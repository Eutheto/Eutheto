use super::MAX_REFERENCE_ITEMS;
use super::common::{Result, invalid, require};
use crate::assignment_rules::budget::OperationBudget;
use crate::ids::{
    AssignmentTypeId, AvailabilityId, CoverageRequirementId, LocationId, QualificationId, ShiftId,
    ShiftTemplateId, TeamId, WorkCalendarId, WorkloadBucketId,
};
use crate::model::{
    AssignmentType, Availability, BaseSchedule, CoverageRequirement, Location, Person,
    PersonSelection, Qualification, Scope, ShiftOrigin, ShiftTemplate, Team, WorkCalendar,
    WorkforceDomainV1, WorkforceEntity, WorkforceScorePolicy, WorkloadBucket,
};
use crate::temporal::{ResolvedShift, ResolvedShiftOrigin, TemporalError, resolve_validated_shift};
use eutheto_types::{EntityId, OperationControl, PersonId, ScenarioSettings};
use jiff::tz::TimeZone;
use std::{cell::RefCell, collections::BTreeSet};

pub const MAX_TEMPLATE_OCCURRENCES: usize = 4096;
pub const MAX_DOCUMENT_OCCURRENCES: usize = 32_768;

pub(super) struct Context<'a> {
    pub domain: &'a WorkforceDomainV1,
    pub settings: &'a ScenarioSettings,
    pub zone: TimeZone,
    pub score_policy: Option<&'a WorkforceScorePolicy>,
    occurrences: BTreeSet<ShiftId>,
    budget: RefCell<OperationBudget<'a>>,
}

macro_rules! entity_lookup {
    ($method:ident, $id:ty, $variant:ident, $record:ty) => {
        pub(super) fn $method(&self, id: $id) -> Result<&$record> {
            match self.domain.entities.get(&id.as_entity_id()) {
                Some(WorkforceEntity::$variant(value)) => Ok(value),
                _ => Err(invalid(
                    stringify!($method),
                    "reference does not resolve to the expected entity kind",
                )),
            }
        }
    };
}

impl<'a> Context<'a> {
    pub(super) fn new(
        domain: &'a WorkforceDomainV1,
        settings: &'a ScenarioSettings,
        control: Option<&'a OperationControl>,
    ) -> Result<Self> {
        control.map_or(Ok(()), OperationControl::check)?;
        require(
            domain.entities.len() <= MAX_REFERENCE_ITEMS
                && domain.rules.len() <= MAX_REFERENCE_ITEMS
                && domain.preferences.len() <= MAX_REFERENCE_ITEMS
                && domain.locked_assignments.len() <= MAX_REFERENCE_ITEMS,
            "domain",
            "too many records",
        )?;
        let mut occurrences = BTreeSet::new();
        let mut dates = BTreeSet::new();
        let mut score_policy = None;
        let mut has_base = false;
        for (id, entity) in &domain.entities {
            control.map_or(Ok(()), OperationControl::check)?;
            require(
                *id == entity.id(),
                "entities.id",
                "owned identity does not match its key",
            )?;
            match entity {
                WorkforceEntity::ScorePolicy(value) => {
                    require(
                        score_policy.is_none(),
                        "entities",
                        "only one score policy is allowed",
                    )?;
                    score_policy = Some(value);
                }
                WorkforceEntity::BaseSchedule(_) => {
                    require(!has_base, "entities", "only one base schedule is allowed")?;
                    has_base = true;
                }
                WorkforceEntity::ShiftTemplate(template) => {
                    require(
                        template.occurrence_identities.len() <= MAX_TEMPLATE_OCCURRENCES,
                        "occurrenceIdentities",
                        "too many template occurrence definitions",
                    )?;
                    for (shift_id, occurrence) in &template.occurrence_identities {
                        control.map_or(Ok(()), OperationControl::check)?;
                        require(
                            *shift_id == occurrence.id,
                            "occurrenceIdentities.id",
                            "owned identity does not match its key",
                        )?;
                        require(
                            occurrences.insert(*shift_id),
                            "occurrenceIdentities",
                            "duplicate shift identity",
                        )?;
                        require(
                            dates.insert((template.id, occurrence.local_start_date)),
                            "occurrenceIdentities",
                            "duplicate template occurrence date",
                        )?;
                    }
                }
                WorkforceEntity::ShiftInstance(shift) => {
                    if let ShiftOrigin::Detached {
                        template_id,
                        occurrence_date,
                    } = shift.origin
                    {
                        require(
                            dates.insert((template_id, occurrence_date)),
                            "origin",
                            "duplicate template occurrence date",
                        )?;
                    }
                }
                _ => {}
            }
            require(
                dates.len() <= MAX_DOCUMENT_OCCURRENCES,
                "occurrenceIdentities",
                "too many document occurrence definitions",
            )?;
        }
        let zone = TimeZone::get(settings.time_zone.as_str())
            .map_err(|_| invalid("settings.timeZone", "unknown scenario time zone"))?;
        Ok(Self {
            domain,
            settings,
            zone,
            score_policy,
            occurrences,
            budget: RefCell::new(OperationBudget::evaluation(control)),
        })
    }

    pub(super) fn checkpoint(&self) -> Result {
        self.budget.borrow_mut().step()?;
        Ok(())
    }

    pub(super) fn person(&self, id: PersonId) -> Result<&Person> {
        match self.domain.entities.get(&EntityId::from_uuid(id.as_uuid())) {
            Some(WorkforceEntity::Person(person)) => Ok(person),
            _ => Err(invalid(
                "personId",
                "reference does not resolve to a person",
            )),
        }
    }

    pub(super) fn base_schedule(&self, id: EntityId) -> Result<&BaseSchedule> {
        match self.domain.entities.get(&id) {
            Some(WorkforceEntity::BaseSchedule(base)) => Ok(base),
            _ => Err(invalid(
                "baseScheduleId",
                "reference does not resolve to the base schedule",
            )),
        }
    }

    pub(super) fn shift(&self, id: ShiftId) -> Result {
        require(
            matches!(
                self.domain.entities.get(&id.as_entity_id()),
                Some(WorkforceEntity::ShiftInstance(_))
            ) || self.occurrences.contains(&id),
            "shiftId",
            "reference does not resolve to a stored shift or occurrence",
        )
    }
    /// Checks the authored target of an active `FixedAssignment` against its complete scope.
    ///
    /// This resolves the retained instance/occurrence rather than manufacturing metadata from
    /// the rule's identifiers. It is deliberately limited to `FixedAssignment`; other rule scopes
    /// retain their existing population semantics.
    pub(super) fn fixed_target_scope(
        &self,
        person_id: PersonId,
        shift_id: ShiftId,
        scope: &Scope,
    ) -> Result {
        let person = self.person(person_id)?;
        let shift = self.resolved_target_shift(shift_id)?;
        let (assignment_type_id, location_id) = self.target_shift_metadata(shift)?;
        let assignment_type = self.assignment_type(assignment_type_id)?;

        match &scope.people {
            PersonSelection::All {} => {}
            PersonSelection::Selected { person_ids } => {
                if !self.member(person_ids, &person.id)? {
                    return Err(invalid(
                        "scope.people",
                        "fixed assignment person is outside the authored scope",
                    ));
                }
            }
            PersonSelection::Filter { all_tags, any_tags } => {
                for tag in all_tags {
                    if !self.member(&person.tags, tag)? {
                        return Err(invalid(
                            "scope.people",
                            "fixed assignment person is outside the authored scope",
                        ));
                    }
                }
                let mut any = any_tags.is_empty();
                for tag in any_tags {
                    if self.member(&person.tags, tag)? {
                        any = true;
                        break;
                    }
                }
                if !any {
                    return Err(invalid(
                        "scope.people",
                        "fixed assignment person is outside the authored scope",
                    ));
                }
            }
        }
        if let Some(team_ids) = &scope.team_ids {
            let mut matched = false;
            for team_id in team_ids {
                if self.member(&person.team_ids, team_id)? {
                    matched = true;
                    break;
                }
            }
            if !matched {
                return Err(invalid(
                    "scope.teamIds",
                    "fixed assignment person is outside the authored scope",
                ));
            }
        }
        if let Some(ids) = &scope.assignment_type_ids
            && !self.member(ids, &assignment_type.id)?
        {
            return Err(invalid(
                "scope.assignmentTypeIds",
                "fixed assignment shift is outside the authored scope",
            ));
        }
        if let Some(categories) = &scope.categories
            && !self.member(categories, &assignment_type.category)?
        {
            return Err(invalid(
                "scope.categories",
                "fixed assignment shift is outside the authored scope",
            ));
        }
        if let Some(weekdays) = &scope.weekdays {
            let day = crate::temporal::weekday(shift.reporting_date);
            if !self.member(weekdays, &day)? {
                return Err(invalid(
                    "scope.weekdays",
                    "fixed assignment shift is outside the authored scope",
                ));
            }
        }
        if let Some(location_ids) = &scope.location_ids {
            let matched = match location_id {
                Some(location_id) => self.member(location_ids, &location_id)?,
                None => false,
            };
            if !matched {
                return Err(invalid(
                    "scope.locationIds",
                    "fixed assignment shift is outside the authored scope",
                ));
            }
        }
        Ok(())
    }

    fn resolved_target_shift(&self, id: ShiftId) -> Result<ResolvedShift> {
        let resolved = resolve_validated_shift(self.domain, self.settings, id, &mut |_| {
            self.checkpoint().map_err(TemporalError::from)
        })
        .map_err(|error| match error {
            TemporalError::InvalidDocument(error) => error,
            TemporalError::Cancelled => eutheto_domain_api::DomainPackError::Cancelled,
            TemporalError::Issue(_) => invalid(
                "shiftId",
                "fixed assignment target shift could not be resolved in the planning horizon",
            ),
        })?;
        resolved.ok_or_else(|| {
            invalid(
                "shiftId",
                "fixed assignment target shift could not be resolved in the planning horizon",
            )
        })
    }

    fn target_shift_metadata(
        &self,
        shift: ResolvedShift,
    ) -> Result<(AssignmentTypeId, Option<LocationId>)> {
        match shift.origin {
            ResolvedShiftOrigin::Generated { template_id, .. } => {
                let Some(WorkforceEntity::ShiftTemplate(template)) =
                    self.domain.entities.get(&template_id.as_entity_id())
                else {
                    return Err(invalid(
                        "shiftId",
                        "fixed assignment target template is not available",
                    ));
                };
                Ok((template.assignment_type_id, template.location_id))
            }
            ResolvedShiftOrigin::Detached { .. } | ResolvedShiftOrigin::Manual => {
                let Some(WorkforceEntity::ShiftInstance(instance)) =
                    self.domain.entities.get(&shift.id.as_entity_id())
                else {
                    return Err(invalid(
                        "shiftId",
                        "fixed assignment target instance is not available",
                    ));
                };
                Ok((instance.assignment_type_id, instance.location_id))
            }
        }
    }

    fn member<T: PartialEq>(&self, values: &[T], value: &T) -> Result<bool> {
        for candidate in values {
            self.checkpoint()?;
            if candidate == value {
                return Ok(true);
            }
        }
        Ok(false)
    }

    entity_lookup!(qualification, QualificationId, Qualification, Qualification);
    entity_lookup!(location, LocationId, Location, Location);
    entity_lookup!(team, TeamId, Team, Team);
    entity_lookup!(bucket, WorkloadBucketId, WorkloadBucket, WorkloadBucket);
    entity_lookup!(calendar, WorkCalendarId, Calendar, WorkCalendar);
    entity_lookup!(
        assignment_type,
        AssignmentTypeId,
        AssignmentType,
        AssignmentType
    );
    entity_lookup!(template, ShiftTemplateId, ShiftTemplate, ShiftTemplate);
    entity_lookup!(availability, AvailabilityId, Availability, Availability);
    entity_lookup!(
        coverage_requirement,
        CoverageRequirementId,
        CoverageRequirement,
        CoverageRequirement
    );
}
