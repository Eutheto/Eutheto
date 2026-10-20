use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit,
    analysis::{self, Plan, PlannedConstraint, Predicate},
    budget::{OperationBudget, add, count, within},
    compiler::{BOOL_ID, DecisionVariables, PROVENANCE_ID, reserve_fact},
    identity::{IdentityKind, PlanningIdentities},
    input::AssignmentInput,
    weekends::{self, WeekendPlan, WeekendPopulation},
};
use crate::model::{AssignmentPair, WorkforceRule};
use eutheto_domain_ir::{DomainEntityId, DomainEntityKindId, DomainEntityRef};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, Constraint, ConstraintRecord, Literal, PlanningConstraintId,
    PlanningIrLimitsV1, ProvenanceId, ProvenanceRecord, ProvenanceSourceKind, Variable,
};
use eutheto_types::{EntityId, PersonId, RuleId, ScenarioSettings};
use jiff::civil::Date;
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::collections::BTreeMap;

const HELPER_MESSAGE: &str = "official.workforce.maximum_worked_weekends.helper";
const CONSTRAINT_MESSAGE: &str = "official.workforce.maximum_worked_weekends.constraint";
const MEASURE_BOOL_ALT_ID: &str =
    "official.workforce.bool.0000000000000000000000000000000000000000000000000000000000000001";
const MEASURE_CONSTRAINT_ID: &str = "official.workforce.constraint.0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Shape {
    pub planned_constraints: u64,
    pub variables: u64,
    pub constraints: u64,
    pub provenance: u64,
    pub references: u64,
    pub component_edges: u64,
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}
fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

pub(super) fn is(planned: &PlannedConstraint) -> bool {
    matches!(planned.predicate, Predicate::MaximumWorkedWeekends { .. })
}

fn data<'a>(
    planned: &PlannedConstraint,
    plan: &'a Plan,
) -> Result<(&'a WeekendPlan, PersonId), AssignmentRuleError> {
    let Predicate::MaximumWorkedWeekends { person, definition } = planned.predicate else {
        return Err(invalid());
    };
    Ok((plan.weekends.get(definition).ok_or_else(invalid)?, person))
}

fn entity(kind: &str, id: EntityId) -> Result<DomainEntityRef, AssignmentRuleError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| invalid())?,
        id: DomainEntityId::new(format!("official.workforce.{id}")).map_err(|_| invalid())?,
    })
}

/// Build source-owned weekend keys before consulting the candidate graph. Candidate membership is
/// added in a separate pass, so statically pruned source shifts cannot erase a weekend boundary.
pub(super) fn plan(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    rule: &WorkforceRule,
    plan: &mut Plan,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumWorkedWeekends {
        id, scope, maximum, ..
    } = rule
    else {
        return Err(invalid());
    };
    let mut source_weekends = BTreeMap::<crate::ids::ShiftId, Vec<weekends::WorkedWeekend>>::new();
    for shift in &input.shifts {
        budget.step()?;
        let windows = weekends::for_shift(shift, settings, budget)?;
        budget.reserve(1, 1, 64)?;
        source_weekends.insert(shift.id, windows);
    }
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !analysis::support::person_scope(scope, person, budget)? {
            continue;
        }
        let start = candidates.partition_point(|pair| pair.person_id < *person_id);
        let mut grouped = BTreeMap::<Date, WeekendPopulation>::new();
        for (offset, pair) in candidates[start..].iter().enumerate() {
            budget.step()?;
            if pair.person_id != *person_id {
                break;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            let metadata = input.metadata(shift)?;
            if !analysis::support::shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            let Some(windows) = source_weekends.get(&shift.id) else {
                return Err(invalid());
            };
            for weekend in windows {
                budget.step()?;
                budget.reserve(0, 1, 8)?;
                let entry = grouped
                    .entry(weekend.date)
                    .or_insert_with(|| WeekendPopulation {
                        date: weekend.date,
                        population: Vec::new(),
                    });
                entry.population.push(start + offset);
            }
        }
        if grouped.is_empty() {
            continue;
        }
        budget.sort_work(grouped.len())?;
        let weekends = grouped.into_values().collect::<Vec<_>>();
        budget.reserve(1, 1, budget.measure(&weekends.len())?)?;
        let definition = plan.weekends.len();
        plan.weekends.push(WeekendPlan {
            maximum: *maximum,
            weekends,
        });
        analysis::append(
            plan,
            PlannedConstraint {
                rule: *id,
                predicate: Predicate::MaximumWorkedWeekends {
                    person: *person_id,
                    definition,
                },
                population: Vec::new(),
                impossible: false,
            },
            budget,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) fn shape(plan: &Plan, limits: PlanningIrLimitsV1) -> Result<Shape, AssignmentRuleError> {
    let mut shape = Shape::default();
    for planned in &plan.constraints {
        if !is(planned) {
            continue;
        }
        let (data, _) = data(planned, plan)?;
        shape.planned_constraints = add(shape.planned_constraints, 1)?;
        let weekend_count = count(data.weekends.len())?;
        shape.variables = add(shape.variables, weekend_count)?;
        shape.provenance = add(shape.provenance, weekend_count)?;
        shape.references = add(
            shape.references,
            weekend_count.checked_mul(3).ok_or_else(arithmetic)?,
        )?;
        let mut body_constraints = 0_u64;
        let mut body_references = 0_u64;
        let mut body_edges = 0_u64;
        for weekend in &data.weekends {
            let members = count(weekend.population.len())?;
            within(
                3,
                limits
                    .max_refs_per_node
                    .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                AssignmentRuleLimit::PerRecord,
            )?;
            let or_refs = members.checked_add(2).ok_or_else(arithmetic)?;
            within(
                or_refs,
                limits
                    .max_refs_per_node
                    .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                AssignmentRuleLimit::PerRecord,
            )?;
            body_constraints = add(
                body_constraints,
                members.checked_add(1).ok_or_else(arithmetic)?,
            )?;
            body_references = add(
                body_references,
                members
                    .checked_mul(5)
                    .and_then(|value| value.checked_add(members + 4))
                    .ok_or_else(arithmetic)?,
            )?;
            body_edges = add(
                body_edges,
                members
                    .checked_mul(3)
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(arithmetic)?,
            )?;
            shape.provenance = add(
                shape.provenance,
                members.checked_add(1).ok_or_else(arithmetic)?,
            )?;
        }
        let cardinality_refs = weekend_count.checked_add(1).ok_or_else(arithmetic)?;
        within(
            cardinality_refs,
            limits
                .max_refs_per_node
                .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
            AssignmentRuleLimit::PerRecord,
        )?;
        body_constraints = add(body_constraints, 1)?;
        body_references = add(
            body_references,
            weekend_count.checked_add(3).ok_or_else(arithmetic)?,
        )?;
        body_edges = add(body_edges, weekend_count)?;
        shape.constraints = add(shape.constraints, body_constraints)?;
        shape.references = add(shape.references, body_references)?;
        shape.component_edges = add(shape.component_edges, body_edges)?;
        shape.provenance = add(shape.provenance, 1)?;
    }
    within(
        shape.variables,
        limits
            .max_variables
            .min(PlanningIrLimitsV1::DEFAULT.max_variables),
        AssignmentRuleLimit::Variables,
    )?;
    within(
        shape.constraints,
        limits
            .max_constraints
            .min(PlanningIrLimitsV1::DEFAULT.max_constraints),
        AssignmentRuleLimit::Constraints,
    )?;
    within(
        shape.provenance,
        limits
            .max_provenance_records
            .min(PlanningIrLimitsV1::DEFAULT.max_provenance_records),
        AssignmentRuleLimit::ProvenanceRecords,
    )?;
    within(
        shape.references,
        limits
            .max_total_refs
            .min(PlanningIrLimitsV1::DEFAULT.max_total_refs),
        AssignmentRuleLimit::References,
    )?;
    if shape.constraints > 0 {
        within(
            2,
            limits.max_provenance_depth,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    Ok(shape)
}

fn helper_fact(
    rule: RuleId,
    person: PersonId,
    id: ProvenanceId,
    parent: ProvenanceId,
    message: &'static str,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Derived,
        source_id: rule.to_string(),
        entity_refs: vec![entity("person", EntityId::from_uuid(person.as_uuid()))?],
        message_key: message.to_owned(),
        parameters: BTreeMap::new(),
        parent: Some(parent),
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_bool(
    rule: RuleId,
    person: PersonId,
    date: Date,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    output: &mut Vec<Variable>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<BoolVariable, AssignmentRuleError> {
    let key = ("maximum_worked_weekends", rule, person, date);
    let id = BoolVariableId::new(identities.derive(IdentityKind::Boolean, &key, budget)?)
        .map_err(|_| invalid())?;
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("maximum_worked_weekends_variable_fact", rule, person, date),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact = helper_fact(rule, person, fact_id.clone(), parent, HELPER_MESSAGE)?;
    budget.reserve(1, 2, 512)?;
    provenance.push(fact);
    let variable = BoolVariable {
        id,
        provenance: fact_id,
    };
    output.push(Variable::Boolean(variable.clone()));
    Ok(variable)
}

#[allow(clippy::too_many_arguments)]
fn emit_constraint<K: Serialize>(
    rule: RuleId,
    person: PersonId,
    key: &K,
    body: Constraint,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let id = PlanningConstraintId::new(identities.derive(
        IdentityKind::Constraint,
        &("maximum_worked_weekends_constraint", rule, person, key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("maximum_worked_weekends_constraint_fact", rule, person, key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact = helper_fact(rule, person, fact_id.clone(), parent, CONSTRAINT_MESSAGE)?;
    budget.reserve(1, 3, 768)?;
    provenance.push(fact);
    constraints.push(ConstraintRecord {
        id,
        body,
        enforcement: Vec::new(),
        provenance: fact_id,
        tags: Vec::new(),
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    planned: &PlannedConstraint,
    plan: &Plan,
    variables: DecisionVariables<'_>,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    output: &mut Vec<Variable>,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let (data, person) = data(planned, plan)?;
    within(
        u64::from(data.maximum),
        u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
        AssignmentRuleLimit::PerRecord,
    )?;
    let mut weekends = Vec::with_capacity(data.weekends.len());
    for weekend in &data.weekends {
        let boolean = emit_bool(
            planned.rule,
            person,
            weekend.date,
            parent.clone(),
            identities,
            output,
            provenance,
            budget,
        )?;
        for (member, index) in weekend.population.iter().enumerate() {
            let x = variables.boolean(*index).ok_or_else(invalid)?;
            emit_constraint(
                planned.rule,
                person,
                &("member", weekend.date, member),
                Constraint::bool_or(vec![
                    Literal::negative(x.id.clone()),
                    Literal::positive(boolean.id.clone()),
                ]),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
            )?;
        }
        let mut literals = Vec::with_capacity(weekend.population.len() + 1);
        literals.push(Literal::negative(boolean.id.clone()));
        for index in &weekend.population {
            let x = variables.boolean(*index).ok_or_else(invalid)?;
            literals.push(Literal::positive(x.id.clone()));
        }
        emit_constraint(
            planned.rule,
            person,
            &("or", weekend.date),
            Constraint::bool_or(literals),
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
        )?;
        weekends.push(boolean);
    }
    let literals = weekends
        .iter()
        .map(|boolean| Literal::positive(boolean.id.clone()))
        .collect::<Vec<_>>();
    emit_constraint(
        planned.rule,
        person,
        &("maximum", data.maximum),
        Constraint::cardinality(
            literals,
            0,
            u64::from(data.maximum).min(count(weekends.len())?),
        )
        .map_err(|_| invalid())?,
        parent,
        identities,
        constraints,
        provenance,
        budget,
    )?;
    Ok(())
}

// ---- bounded IR preflight -------------------------------------------------

struct MeasuredLiterals(usize);
impl Serialize for MeasuredLiterals {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0))?;
        for index in 0..self.0 {
            sequence.serialize_element(&MeasuredLiteral {
                variable: if index & 1 == 1 {
                    MEASURE_BOOL_ALT_ID
                } else {
                    BOOL_ID
                },
                positive: false,
            })?;
        }
        sequence.end()
    }
}
#[derive(Serialize)]
struct MeasuredLiteral {
    variable: &'static str,
    positive: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredBody {
    BoolOr {
        literals: MeasuredLiterals,
    },
    CardinalityRange {
        literals: MeasuredLiterals,
        min: u64,
        max: u64,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredConstraint {
    id: &'static str,
    body: MeasuredBody,
    enforcement: MeasuredLiterals,
    provenance: &'static str,
    tags: [(); 0],
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredVariable {
    Boolean {
        id: &'static str,
        provenance: &'static str,
    },
}

fn measured_fact(rule: RuleId, person: PersonId) -> Result<ProvenanceRecord, AssignmentRuleError> {
    helper_fact(
        rule,
        person,
        ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
        ProvenanceId::new(PROVENANCE_ID).map_err(|_| invalid())?,
        HELPER_MESSAGE,
    )
}
fn charge_variable(
    rule: RuleId,
    person: PersonId,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    budget.reserve_ir(
        1,
        1,
        budget.measure_ir(&MeasuredVariable::Boolean {
            id: BOOL_ID,
            provenance: PROVENANCE_ID,
        })?,
    )?;
    budget.reserve(1, 2, 512)?;
    reserve_fact(&measured_fact(rule, person)?, budget, limits)
}
fn charge_constraint(
    rule: RuleId,
    person: PersonId,
    body: MeasuredBody,
    body_refs: usize,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let record = MeasuredConstraint {
        id: MEASURE_CONSTRAINT_ID,
        body,
        enforcement: MeasuredLiterals(0),
        provenance: PROVENANCE_ID,
        tags: [],
    };
    budget.reserve(1, 3, 768)?;
    budget.reserve_ir(
        1,
        count(body_refs).and_then(|value| value.checked_add(1).ok_or_else(arithmetic))?,
        budget.measure_ir(&record)?,
    )?;
    budget.reserve(1, 2, 512)?;
    reserve_fact(&measured_fact(rule, person)?, budget, limits)
}

pub(super) fn preflight_contribution(
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    for planned in &plan.constraints {
        if !is(planned) {
            continue;
        }
        let (data, person) = data(planned, plan)?;
        within(
            u64::from(data.maximum),
            u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
            AssignmentRuleLimit::PerRecord,
        )?;
        let weekend_count = data.weekends.len();
        for weekend in &data.weekends {
            charge_variable(planned.rule, person, budget, limits)?;
            for _ in &weekend.population {
                charge_constraint(
                    planned.rule,
                    person,
                    MeasuredBody::BoolOr {
                        literals: MeasuredLiterals(2),
                    },
                    2,
                    budget,
                    limits,
                )?;
            }
            let refs = weekend
                .population
                .len()
                .checked_add(1)
                .ok_or_else(arithmetic)?;
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(refs),
                },
                refs,
                budget,
                limits,
            )?;
        }
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::CardinalityRange {
                literals: MeasuredLiterals(weekend_count),
                min: 0,
                max: u64::from(data.maximum).min(count(weekend_count)?),
            },
            weekend_count,
            budget,
            limits,
        )?;
    }
    budget.check()
}

pub(super) fn helper_variable_bytes(
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<u64, AssignmentRuleError> {
    let mut bytes = 0_u64;
    for planned in &plan.constraints {
        if !is(planned) {
            continue;
        }
        let (data, _) = data(planned, plan)?;
        for _ in &data.weekends {
            budget.step()?;
            bytes = add(
                bytes,
                budget.measure_ir(&MeasuredVariable::Boolean {
                    id: BOOL_ID,
                    provenance: PROVENANCE_ID,
                })?,
            )?;
        }
    }
    Ok(bytes)
}
