use super::{
    AssignmentConstructionIssue, AssignmentRuleCompilation, AssignmentRuleError, AssignmentRuleLimit, AssignmentVariable,
    analysis::{Plan, PlannedConstraint, Predicate, predicate_shape, prepare},
    budget::{OperationBudget, add, count, within}, identity::{IdentityKind, PlanningIdentities}, input::AssignmentInput,
};
use crate::model::AssignmentPair;
use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::{DomainEntityId, DomainEntityKindId, DomainEntityRef};
use eutheto_planning_ir::{BoolVariable, BoolVariableId, Constraint, ConstraintRecord, Literal, PlanningConstraintId, PlanningIrLimitsV1, ProvenanceId, ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind};
use eutheto_types::{EntityId, RuleId, ScenarioDocument};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::collections::BTreeMap;

// Only the digest bytes vary between these measurement placeholders and real derived IDs.
// Every identifier is ASCII and every digest has exactly 64 lowercase hexadecimal digits.
const BOOL_ID: &str = "official.workforce.bool.0000000000000000000000000000000000000000000000000000000000000000";
const CONSTRAINT_ID: &str = "official.workforce.constraint.0000000000000000000000000000000000000000000000000000000000000000";
const PROVENANCE_ID: &str = "official.workforce.provenance.0000000000000000000000000000000000000000000000000000000000000000";

/// Compile only the unregistered four-rule mathematical contribution.
///
/// # Errors
/// Returns structural, temporal, cancellation, finite-work and bounded-output failures atomically.
pub fn compile_assignment_rules(document: &ScenarioDocument, context: &CompileContext) -> Result<AssignmentRuleCompilation, AssignmentRuleError> {
    let limits = context.planning_limits;
    let mut budget = OperationBudget::analysis(Some(&context.cancellation), limits);
    let input = AssignmentInput::new(document, &mut budget)?;
    let (analysis, plan) = prepare(document, &input, &mut budget, limits)?;
    // Counts, per-record bounds, references and exact serialized contribution bytes are all
    // reserved before the first IR variable is allocated. The dry pass streams literal lists.
    preflight(&analysis.candidates, &plan, &mut budget, limits)?;
    let mut identities = PlanningIdentities::default();
    let mut variables = Vec::new();
    let mut provenance = Vec::new();
    let mut constraints = Vec::new();
    for pair in analysis.candidates {
        budget.step()?;
        let key = ("assignment", pair.person_id, pair.shift_id);
        let id = BoolVariableId::new(identities.derive(IdentityKind::Boolean, &key, &mut budget)?).map_err(|_| invalid_id())?;
        let fact_id = ProvenanceId::new(identities.derive(IdentityKind::Provenance, &key, &mut budget)?).map_err(|_| invalid_id())?;
        let fact = variable_fact(pair, fact_id.clone())?;
        provenance.push(fact);
        variables.push(AssignmentVariable { pair, variable: BoolVariable { id, provenance: fact_id } });
    }
    let mut parents = BTreeMap::new();
    for (rule, kind) in &plan.parents {
        budget.step()?;
        let id = ProvenanceId::new(identities.derive(IdentityKind::Provenance, &("rule", rule, kind), &mut budget)?).map_err(|_| invalid_id())?;
        budget.reserve(1, 1, 128)?;
        parents.insert(*rule, id.clone());
        provenance.push(rule_fact(*rule, kind, id));
    }
    for planned in &plan.constraints {
        budget.step()?;
        let id = PlanningConstraintId::new(derive_predicate(IdentityKind::Constraint, planned, &plan, &mut identities, &mut budget)?).map_err(|_| invalid_id())?;
        let fact_id = ProvenanceId::new(derive_predicate(IdentityKind::Provenance, planned, &plan, &mut identities, &mut budget)?).map_err(|_| invalid_id())?;
        let parent = parents.get(&planned.rule).ok_or_else(invalid)?.clone();
        let mut literals = Vec::new();
        if !planned.impossible {
            for index in &planned.population {
                budget.step()?;
                let variable = variables.get(*index).ok_or_else(invalid)?;
                literals.push(Literal::positive(variable.variable.id.clone()));
            }
        }
        let body = if planned.impossible { Constraint::bool_or(literals) } else {
            match planned.predicate {
                Predicate::Headcount { lower, upper, .. } => Constraint::cardinality(literals, lower, upper).map_err(|_| invalid())?,
                Predicate::Qualification { definition, minimum, upper, .. } => Constraint::cardinality(literals, u64::from(plan.definitions[definition].minima[minimum].minimum), upper).map_err(|_| invalid())?,
                Predicate::Overlap { .. } => Constraint::at_most_one(literals),
            }
        };
        let (entities, parameters) = predicate_shape(&planned.predicate, &plan)?;
        budget.steps(add(entities, parameters)?)?;
        provenance.push(constraint_fact(planned, &plan, fact_id.clone(), parent)?);
        constraints.push(ConstraintRecord { id, body, enforcement: Vec::new(), provenance: fact_id, tags: Vec::new() });
    }
    variables.sort_unstable_by(|a, b| a.variable.id.cmp(&b.variable.id));
    constraints.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    provenance.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    budget.check()?;
    Ok(AssignmentRuleCompilation {
        source_document_hash: analysis.source_document_hash, variables, constraints, provenance,
        rejections: analysis.rejections, estimate: analysis.estimate, validation: analysis.validation, obligations: analysis.obligations,
    })
}

fn derive_predicate(kind: IdentityKind, planned: &PlannedConstraint, plan: &Plan, identities: &mut PlanningIdentities, budget: &mut OperationBudget<'_>) -> Result<String, AssignmentRuleError> {
    match planned.predicate {
        Predicate::Headcount { definition, shift, lower, upper, .. } => identities.derive(kind, &("headcount", planned.rule, plan.definitions[definition].owner, shift, lower, upper), budget),
        Predicate::Qualification { definition, minimum, shift, .. } => identities.derive(kind, &("qualification_minimum", planned.rule, plan.definitions[definition].owner, shift, &plan.definitions[definition].minima[minimum].identity), budget),
        Predicate::Overlap { person, first, second } => identities.derive(kind, &("no_overlap", planned.rule, person, first, second), budget),
    }
}

fn entity(kind: &str, id: EntityId) -> Result<DomainEntityRef, AssignmentRuleError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}")).map_err(|_| invalid_id())?,
        id: DomainEntityId::new(format!("official.workforce.{id}")).map_err(|_| invalid_id())?,
    })
}

fn variable_fact(pair: AssignmentPair, id: ProvenanceId) -> Result<ProvenanceRecord, AssignmentRuleError> {
    let mut entity_refs = vec![entity("person", EntityId::from_uuid(pair.person_id.as_uuid()))?, entity("shift", pair.shift_id.as_entity_id())?];
    entity_refs.sort_unstable();
    Ok(ProvenanceRecord {
        id, source_kind: ProvenanceSourceKind::Fact, source_id: pair.person_id.to_string(),
        entity_refs, message_key: "official.workforce.assignment_option".to_owned(), parameters: BTreeMap::new(), parent: None,
    })
}

fn rule_fact(rule: RuleId, kind: &str, id: ProvenanceId) -> ProvenanceRecord {
    ProvenanceRecord {
        id, source_kind: ProvenanceSourceKind::RequiredRule, source_id: rule.to_string(), entity_refs: Vec::new(),
        message_key: format!("official.workforce.{kind}"), parameters: BTreeMap::new(), parent: None,
    }
}

fn constraint_fact(planned: &PlannedConstraint, plan: &Plan, id: ProvenanceId, parent: ProvenanceId) -> Result<ProvenanceRecord, AssignmentRuleError> {
    let mut parameters = BTreeMap::new();
    let (mut entity_refs, message_key) = match planned.predicate {
        Predicate::Headcount { definition, shift, lower, upper, authored_upper } => {
            let owner = plan.definitions[definition].owner;
            parameters.insert("minimum".to_owned(), integer(lower)?);
            parameters.insert("normalized_maximum".to_owned(), integer(upper)?);
            parameters.insert("has_authored_maximum".to_owned(), ProvenanceParameter::Boolean(authored_upper.is_some()));
            if let Some(value) = authored_upper { parameters.insert("authored_maximum".to_owned(), integer(value)?); }
            (vec![entity(owner.kind, owner.id)?, entity("shift", shift.as_entity_id())?], "official.workforce.coverage_headcount")
        }
        Predicate::Qualification { definition, minimum, shift, .. } => {
            let definition = &plan.definitions[definition];
            let key = &definition.minima[minimum];
            parameters.insert("minimum".to_owned(), ProvenanceParameter::Integer(i64::from(key.minimum)));
            for (prefix, qualifications) in [("all", &key.all), ("any", &key.any)] {
                for qualification in qualifications {
                    parameters.insert(format!("{prefix}.{qualification}"), ProvenanceParameter::Entity(entity("qualification", qualification.as_entity_id())?));
                }
            }
            (vec![entity(definition.owner.kind, definition.owner.id)?, entity("shift", shift.as_entity_id())?], "official.workforce.coverage_qualification_minimum")
        }
        Predicate::Overlap { person, first, second } => (vec![entity("person", EntityId::from_uuid(person.as_uuid()))?, entity("shift", first.as_entity_id())?, entity("shift", second.as_entity_id())?], "official.workforce.incompatible_overlap"),
    };
    entity_refs.sort_unstable();
    Ok(ProvenanceRecord { id, source_kind: ProvenanceSourceKind::Derived, source_id: planned.rule.to_string(), entity_refs, message_key: message_key.to_owned(), parameters, parent: Some(parent) })
}

fn integer(value: u64) -> Result<ProvenanceParameter, AssignmentRuleError> {
    Ok(ProvenanceParameter::Integer(i64::try_from(value).map_err(|_| invalid())?))
}

#[derive(Serialize)]
struct MeasuredVariable { id: &'static str, provenance: &'static str }

#[derive(Serialize)]
struct MeasuredLiteral { variable: &'static str, positive: bool }

// A borrowed-size serializer deliberately has no Vec of placeholder literals. Measurement
// cannot create a large graph only to discover it exceeds the caller's byte quota.
struct MeasuredLiterals(usize);
impl Serialize for MeasuredLiterals {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0))?;
        for _ in 0..self.0 { seq.serialize_element(&MeasuredLiteral { variable: BOOL_ID, positive: true })?; }
        seq.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredBody {
    BoolOr { literals: MeasuredLiterals },
    AtMostOne { literals: MeasuredLiterals },
    CardinalityRange { literals: MeasuredLiterals, min: u64, max: u64 },
}

#[derive(Serialize)]
struct MeasuredConstraint {
    id: &'static str,
    body: MeasuredBody,
    enforcement: [(); 0],
    provenance: &'static str,
    tags: [(); 0],
}

pub(super) fn preflight(pairs: &[AssignmentPair], plan: &Plan, budget: &mut OperationBudget<'_>, limits: PlanningIrLimitsV1) -> Result<(), AssignmentRuleError> {
    if !pairs.is_empty() || !plan.constraints.is_empty() {
        within(count(PROVENANCE_ID.len())?, limits.max_id_bytes.min(PlanningIrLimitsV1::DEFAULT.max_id_bytes), AssignmentRuleLimit::PerRecord)?;
    }
    let placeholder = ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid_id())?;
    for pair in pairs {
        budget.step()?;
        let variable = MeasuredVariable { id: BOOL_ID, provenance: PROVENANCE_ID };
        budget.reserve_ir(1, 1, budget.measure_ir(&variable)?)?;
        // Temporary small provenance payloads are charged separately before materialization.
        budget.reserve(1, 2, 1024)?;
        let fact = variable_fact(*pair, placeholder.clone())?;
        reserve_fact(&fact, budget, limits)?;
    }
    for (rule, kind) in &plan.parents {
        budget.step()?;
        budget.reserve(1, 0, 512)?;
        reserve_fact(&rule_fact(*rule, kind, placeholder.clone()), budget, limits)?;
    }
    for planned in &plan.constraints {
        budget.step()?;
        let literals = MeasuredLiterals(if planned.impossible { 0 } else { planned.population.len() });
        budget.steps(count(literals.0)?)?;
        let literal_count = count(literals.0)?;
        let body = if planned.impossible { MeasuredBody::BoolOr { literals } } else {
            match planned.predicate {
                Predicate::Headcount { lower, upper, .. } => MeasuredBody::CardinalityRange { literals, min: lower, max: upper },
                Predicate::Qualification { definition, minimum, upper, .. } => MeasuredBody::CardinalityRange { literals, min: u64::from(plan.definitions[definition].minima[minimum].minimum), max: upper },
                Predicate::Overlap { .. } => MeasuredBody::AtMostOne { literals },
            }
        };
        let record = MeasuredConstraint { id: CONSTRAINT_ID, body, enforcement: [], provenance: PROVENANCE_ID, tags: [] };
        budget.reserve_ir(1, add(literal_count, 1)?, budget.measure_ir(&record)?)?;
        let (entities, parameters) = predicate_shape(&planned.predicate, plan)?;
        let temporary_bytes = add(1024, add(entities, parameters)?.checked_mul(512).ok_or_else(invalid)?)?;
        budget.reserve(1, add(entities, parameters)?, temporary_bytes)?;
        budget.steps(add(entities, parameters)?)?;
        reserve_fact(&constraint_fact(planned, plan, placeholder.clone(), placeholder.clone())?, budget, limits)?;
    }
    budget.check()
}

fn reserve_fact(record: &ProvenanceRecord, budget: &mut OperationBudget<'_>, limits: PlanningIrLimitsV1) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    for text in [&record.source_id, &record.message_key] {
        within(count(text.len())?, limits.max_metadata_text_bytes.min(defaults.max_metadata_text_bytes), AssignmentRuleLimit::PerRecord)?;
    }
    for (key, parameter) in &record.parameters {
        budget.step()?;
        within(count(key.len())?, limits.max_parameter_text_bytes.min(defaults.max_parameter_text_bytes), AssignmentRuleLimit::PerRecord)?;
        if let ProvenanceParameter::Text(value) = parameter {
            within(count(value.len())?, limits.max_parameter_text_bytes.min(defaults.max_parameter_text_bytes), AssignmentRuleLimit::PerRecord)?;
        }
        if let ProvenanceParameter::Integer(value) = parameter {
            if *value > limits.max_abs_value.min(defaults.max_abs_value) { return Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord)); }
        }
    }
    let refs = add(count(record.entity_refs.len() + record.parameters.len())?, u64::from(record.parent.is_some()))?;
    budget.reserve_ir(1, refs, budget.measure_ir(record)?)
}

fn invalid() -> AssignmentRuleError { AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord) }
fn invalid_id() -> AssignmentRuleError { AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidIdentifier) }
