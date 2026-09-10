use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit, SelectionIssueKind,
    budget::{MAX_SELECTED_PAIRS, OperationBudget, add, count, within},
};
use crate::{
    ids::{AssignmentTypeId, AvailabilityId, LocationId, ShiftId, ShiftTemplateId},
    model::{AssignmentPair, AssignmentType, Coverage, Person, WorkforceDomainV1, WorkforceEntity},
    temporal::{ResolvedShift, ResolvedShiftOrigin, resolve_validated_shifts},
    validation::{validate_document_controlled, validate_value_bounds},
};
use eutheto_types::{EntityId, PersonId, ScenarioDocument};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Structural data only. No scopes, eligibility decisions, candidate graph or solver state.
pub(crate) struct AssignmentInput {
    pub domain: WorkforceDomainV1,
    pub shifts: Vec<ResolvedShift>,
    pub people: Vec<PersonId>,
    pub availability_by_person: BTreeMap<PersonId, Vec<AvailabilityId>>,
    pub source_document_hash: String,
    shift_positions: BTreeMap<ShiftId, usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub(crate) enum ShiftDefinition {
    Template(ShiftTemplateId),
    Instance(ShiftId),
}

impl ShiftDefinition {
    pub fn entity_id(self) -> EntityId {
        match self {
            Self::Template(id) => id.as_entity_id(),
            Self::Instance(id) => id.as_entity_id(),
        }
    }
}

pub(crate) struct ShiftMetadata<'a> {
    pub definition: ShiftDefinition,
    pub assignment_type: &'a AssignmentType,
    pub location_id: Option<LocationId>,
    pub coverage: &'a Coverage,
}

impl AssignmentInput {
    pub fn new(
        document: &ScenarioDocument,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Self, AssignmentRuleError> {
        budget.check()?;
        // Account for decoded source data before allocating its typed representation. The existing
        // ingress remains the authority for structure, recursive depth, references and schema bounds.
        for value in document
            .domain
            .entities
            .values()
            .chain(document.domain.rules.values())
            .chain(document.domain.preferences.values())
            .chain(document.domain.locked_assignments.values())
        {
            validate_value_bounds(value, "document")?;
            charge_items(value, budget)?;
            let bytes = add(16, budget.measure(value)?)?;
            budget.reserve(1, 1, bytes)?;
        }
        let domain = validate_document_controlled(document, budget.control())?;
        budget.check()?;
        let shifts = resolve_validated_shifts(&domain, &document.settings, &mut |event| {
            budget.resolution_step(event)
        })?;
        let mut shift_positions = BTreeMap::new();
        for (position, shift) in shifts.iter().enumerate() {
            budget.step()?;
            budget.reserve(1, 1, 24)?;
            if shift_positions.insert(shift.id, position).is_some() {
                return Err(construction());
            }
        }
        let mut people = Vec::new();
        let mut availability_by_person = BTreeMap::<PersonId, Vec<AvailabilityId>>::new();
        for entity in domain.entities.values() {
            budget.step()?;
            match entity {
                WorkforceEntity::Person(person) => {
                    budget.reserve(0, 1, 16)?;
                    people.push(person.id);
                }
                WorkforceEntity::Availability(availability) => {
                    if !availability_by_person.contains_key(&availability.person_id) {
                        budget.reserve(1, 1, 16)?;
                    }
                    budget.reserve(0, 1, 16)?;
                    availability_by_person
                        .entry(availability.person_id)
                        .or_default()
                        .push(availability.id);
                }
                _ => {}
            }
        }
        budget.sort_work(people.len())?;
        people.sort_unstable();
        for records in availability_by_person.values_mut() {
            budget.sort_work(records.len())?;
            records.sort_unstable();
        }
        budget.reserve(1, 0, 64)?;
        let mut hasher = blake3::Hasher::new();
        serde_json::to_writer(&mut hasher, document).map_err(|_| {
            AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::SourceHash)
        })?;
        budget.check()?;
        let source_document_hash = hasher.finalize().to_hex().to_string();
        Ok(Self {
            domain,
            shifts,
            people,
            availability_by_person,
            source_document_hash,
            shift_positions,
        })
    }

    pub fn person(&self, id: PersonId) -> Option<&Person> {
        match self.domain.entities.get(&EntityId::from_uuid(id.as_uuid())) {
            Some(WorkforceEntity::Person(person)) => Some(person),
            _ => None,
        }
    }

    pub fn shift(&self, id: ShiftId) -> Option<&ResolvedShift> {
        self.shift_positions
            .get(&id)
            .and_then(|position| self.shifts.get(*position))
    }

    pub fn assignment_type(
        &self,
        id: AssignmentTypeId,
    ) -> Result<&AssignmentType, AssignmentRuleError> {
        match self.domain.entities.get(&id.as_entity_id()) {
            Some(WorkforceEntity::AssignmentType(value)) => Ok(value),
            _ => Err(construction()),
        }
    }

    pub fn metadata(
        &self,
        shift: &ResolvedShift,
    ) -> Result<ShiftMetadata<'_>, AssignmentRuleError> {
        let (definition, assignment_type_id, location_id, coverage) = match shift.origin {
            ResolvedShiftOrigin::Generated { template_id, .. } => {
                let Some(WorkforceEntity::ShiftTemplate(template)) =
                    self.domain.entities.get(&template_id.as_entity_id())
                else {
                    return Err(construction());
                };
                (
                    ShiftDefinition::Template(template_id),
                    template.assignment_type_id,
                    template.location_id,
                    &template.coverage,
                )
            }
            ResolvedShiftOrigin::Detached { .. } | ResolvedShiftOrigin::Manual => {
                let Some(WorkforceEntity::ShiftInstance(instance)) =
                    self.domain.entities.get(&shift.id.as_entity_id())
                else {
                    return Err(construction());
                };
                (
                    ShiftDefinition::Instance(shift.id),
                    instance.assignment_type_id,
                    instance.location_id,
                    &instance.coverage,
                )
            }
        };
        Ok(ShiftMetadata {
            definition,
            assignment_type: self.assignment_type(assignment_type_id)?,
            location_id,
            coverage,
        })
    }

    /// Structural validation only: identified but ineligible/unavailable pairs are deliberately kept.
    pub fn validate_selection(
        &self,
        pairs: &[AssignmentPair],
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.check()?;
        within(
            count(pairs.len())?,
            MAX_SELECTED_PAIRS,
            AssignmentRuleLimit::SelectedPairs,
        )?;
        let mut seen = BTreeSet::new();
        for pair in pairs {
            budget.step()?;
            let person_key = EntityId::from_uuid(pair.person_id.as_uuid());
            let kind = match self.domain.entities.get(&person_key) {
                None => Some(SelectionIssueKind::MissingPerson),
                Some(WorkforceEntity::Person(_)) => None,
                Some(_) => Some(SelectionIssueKind::WrongKindPerson),
            };
            if let Some(kind) = kind {
                return Err(AssignmentRuleError::InvalidSelection { pair: *pair, kind });
            }
            if self.shift(pair.shift_id).is_none() {
                return Err(AssignmentRuleError::InvalidSelection {
                    pair: *pair,
                    kind: SelectionIssueKind::UnresolvedShift,
                });
            }
            if seen.contains(pair) {
                return Err(AssignmentRuleError::InvalidSelection {
                    pair: *pair,
                    kind: SelectionIssueKind::DuplicatePair,
                });
            }
            budget.reserve(1, 2, 32)?;
            seen.insert(*pair);
        }
        Ok(())
    }
}

fn charge_items(
    value: &Value,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve(0, 1, 0)?;
    match value {
        Value::Array(values) => {
            for value in values {
                charge_items(value, budget)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                charge_items(value, budget)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn construction() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}
