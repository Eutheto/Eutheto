use super::super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit,
    analysis::{append, person_scope, shift_scope},
    budget::{OperationBudget, add, count, within},
    compiler::{DecisionVariables, reserve_fact},
    identity::{IdentityKind, PlanningIdentities},
    input::AssignmentInput,
};
use super::{Definition, Owner, Plan, PlannedConstraint, Predicate};
use crate::model::{AssignmentPair, ConsecutiveMode, WorkforceRule};
use eutheto_domain_ir::{DomainEntityId, DomainEntityKindId, DomainEntityRef};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, Capability, ComparisonOp, Constraint, ConstraintRecord,
    InclusiveRange, IntDomain, IntVariable, IntVariableId, LinearComparison, LinearExpression,
    LinearTerm, Literal, PlanningConstraintId, PlanningIrLimitsV1, ProvenanceId, ProvenanceRecord,
    ProvenanceSourceKind, Variable,
};
use eutheto_types::{EntityId, PersonId, RuleId};
use jiff::{SignedDuration, Span, civil::Date};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::collections::{BTreeMap, BTreeSet};

const HELPER_MESSAGE: &str = "official.workforce.maximum_consecutive.helper";
const MEASURE_BOOL_ID: &str =
    "official.workforce.bool.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURE_BOOL_ALT_ID: &str =
    "official.workforce.bool.0000000000000000000000000000000000000000000000000000000000000001";
const MEASURE_INT_ID: &str =
    "official.workforce.int.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURE_INT_ALT_ID: &str =
    "official.workforce.int.0000000000000000000000000000000000000000000000000000000000000001";
const MEASURE_CONSTRAINT_ID: &str = "official.workforce.constraint.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURE_PROVENANCE_ID: &str = "official.workforce.provenance.0000000000000000000000000000000000000000000000000000000000000000";

/// Source timing required to compile one `MaximumConsecutive` person/rule plan.
/// Candidate positions remain indexes into the one canonical source candidate vector.
pub(crate) struct ConsecutivePlan {
    pub mode: ConsecutiveMode,
    pub maximum: u32,
    pub break_minutes: Option<u32>,
    /// For Assignments, one entry for every `i < j`, in lexicographic `(i, j)` order.
    pub joins: Vec<bool>,
    /// For `WorkedDays`, sorted distinct reporting dates and their matching candidates.
    pub days: Vec<ConsecutiveDay>,
}

pub(crate) struct ConsecutiveDay {
    pub date: Date,
    pub population: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Shape {
    pub planned_constraints: u64,
    pub variables: u64,
    pub constraints: u64,
    pub provenance: u64,
    pub references: u64,
    pub component_edges: u64,
}

impl Shape {
    pub(crate) fn add(self, other: Self) -> Result<Self, AssignmentRuleError> {
        Ok(Self {
            planned_constraints: add(self.planned_constraints, other.planned_constraints)?,
            variables: add(self.variables, other.variables)?,
            constraints: add(self.constraints, other.constraints)?,
            provenance: add(self.provenance, other.provenance)?,
            references: add(self.references, other.references)?,
            component_edges: add(self.component_edges, other.component_edges)?,
        })
    }
}

pub(crate) fn is(planned: &PlannedConstraint) -> bool {
    matches!(planned.predicate, Predicate::MaximumConsecutive { .. })
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

fn data<'a>(
    planned: &PlannedConstraint,
    plan: &'a Plan,
) -> Result<(&'a ConsecutivePlan, PersonId), AssignmentRuleError> {
    let Predicate::MaximumConsecutive { person, definition } = planned.predicate else {
        return Err(invalid());
    };
    let definition = plan.definitions.get(definition).ok_or_else(invalid)?;
    Ok((definition.consecutive.as_ref().ok_or_else(invalid)?, person))
}

fn owner(person: PersonId) -> Owner {
    Owner {
        kind: "person",
        id: EntityId::from_uuid(person.as_uuid()),
    }
}
pub(crate) fn mode_name(mode: ConsecutiveMode) -> &'static str {
    match mode {
        ConsecutiveMode::WorkedDays {} => "workedDays",
        ConsecutiveMode::Assignments { .. } => "assignments",
    }
}

fn entity(kind: &str, id: EntityId) -> Result<DomainEntityRef, AssignmentRuleError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| invalid())?,
        id: DomainEntityId::new(format!("official.workforce.{id}")).map_err(|_| invalid())?,
    })
}

fn check_maximum(maximum: u32, limits: PlanningIrLimitsV1) -> Result<(), AssignmentRuleError> {
    within(
        u64::from(maximum),
        u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
        AssignmentRuleLimit::PerRecord,
    )
}

/// Plan only matching source candidates. A rule/person with no possible violation is omitted
/// entirely, so vacuous rules allocate neither helper variables nor helper provenance.
#[allow(clippy::too_many_lines)]
pub(crate) fn plan(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    plan: &mut Plan,
    rule: &WorkforceRule,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumConsecutive {
        id,
        scope,
        mode,
        maximum,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !person_scope(scope, person, budget)? {
            continue;
        }
        let start = candidates.partition_point(|pair| pair.person_id < *person_id);
        let mut matching = Vec::new();
        for (offset, pair) in candidates[start..].iter().enumerate() {
            budget.step()?;
            if pair.person_id != *person_id {
                break;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            if shift_scope(scope, shift, &input.metadata(shift)?, budget)? {
                budget.reserve(0, 1, 8)?;
                matching.push(start + offset);
            }
        }
        if matching.is_empty() {
            continue;
        }
        let mut consecutive = ConsecutivePlan {
            mode: *mode,
            maximum: *maximum,
            break_minutes: match mode {
                ConsecutiveMode::WorkedDays {} => None,
                ConsecutiveMode::Assignments { break_minutes } => Some(*break_minutes),
            },
            joins: Vec::new(),
            days: Vec::new(),
        };
        match mode {
            ConsecutiveMode::WorkedDays {} => {
                let mut grouped = BTreeMap::<Date, Vec<usize>>::new();
                for index in &matching {
                    budget.step()?;
                    let pair = candidates[*index];
                    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
                    budget.reserve(0, 1, 8)?;
                    grouped
                        .entry(shift.reporting_date)
                        .or_default()
                        .push(*index);
                }
                budget.sort_work(grouped.len())?;
                for (date, population) in grouped {
                    budget.reserve(0, 1, 16)?;
                    consecutive.days.push(ConsecutiveDay { date, population });
                }
                let distinct = count(consecutive.days.len())?;
                if *maximum > 0
                    && (u64::from(*maximum) >= distinct
                        || window_count(&consecutive.days, *maximum)? == 0)
                {
                    continue;
                }
                if *maximum > 0 {
                    check_worked_shape(&consecutive.days, *maximum, limits)?;
                }
            }
            ConsecutiveMode::Assignments { .. } => {
                budget.sort_work(matching.len())?;
                matching.sort_unstable_by(|left, right| {
                    let left_pair = candidates[*left];
                    let right_pair = candidates[*right];
                    let left_key = input
                        .shift(left_pair.shift_id)
                        .map(|shift| (shift.interval.starts_at.instant, left_pair.shift_id));
                    let right_key = input
                        .shift(right_pair.shift_id)
                        .map(|shift| (shift.interval.starts_at.instant, right_pair.shift_id));
                    left_key.cmp(&right_key)
                });
                if *maximum > 0 {
                    if u64::try_from(matching.len()).map_err(|_| arithmetic())?
                        <= u64::from(*maximum)
                    {
                        continue;
                    }
                    check_assignment_shape(matching.len(), *maximum, limits)?;
                    let pair_count = pair_count(matching.len())?;
                    budget.reserve(0, pair_count, pair_count)?;
                    let break_duration = SignedDuration::from_mins(i64::from(
                        consecutive.break_minutes.ok_or_else(invalid)?,
                    ));
                    for i in 0..matching.len() {
                        let previous = input
                            .shift(candidates[matching[i]].shift_id)
                            .ok_or_else(invalid)?;
                        for j in (i + 1)..matching.len() {
                            budget.step()?;
                            let next = input
                                .shift(candidates[matching[j]].shift_id)
                                .ok_or_else(invalid)?;
                            let gap = previous
                                .interval
                                .ends_at
                                .instant
                                .as_timestamp()
                                .duration_until(next.interval.starts_at.instant.as_timestamp());
                            consecutive.joins.push(gap <= break_duration);
                        }
                    }
                }
            }
        }
        if *maximum == 0 {
            check_assignment_shape(matching.len(), 0, limits)?;
        }
        let definition = plan.definitions.len();
        budget.reserve(1, 3, 256)?;
        plan.definitions.push(Definition {
            owner: owner(*person_id),
            minima: Vec::new(),
            source_rows: Vec::new(),
            consecutive: Some(consecutive),
        });
        append(
            plan,
            PlannedConstraint {
                rule: *id,
                predicate: Predicate::MaximumConsecutive {
                    person: *person_id,
                    definition,
                },
                population: matching,
                impossible: false,
            },
            budget,
        )?;
    }
    Ok(())
}

fn pair_count(n: usize) -> Result<u64, AssignmentRuleError> {
    let n = count(n)?;
    n.checked_mul(n.saturating_sub(1))
        .and_then(|value| value.checked_div(2))
        .ok_or_else(arithmetic)
}

fn q_count(n: usize) -> Result<u64, AssignmentRuleError> {
    if n < 3 {
        return Ok(0);
    }
    let left = count(n - 1)?;
    let right = count(n - 2)?;
    left.checked_mul(right)
        .and_then(|value| value.checked_div(2))
        .ok_or_else(arithmetic)
}

fn add_constraint_shape(
    shape: &mut Shape,
    body: u64,
    enforcement: u64,
) -> Result<(), AssignmentRuleError> {
    shape.constraints = add(shape.constraints, 1)?;
    shape.provenance = add(shape.provenance, 1)?;
    shape.references = add(
        shape.references,
        body.checked_add(enforcement)
            .and_then(|v| v.checked_add(3))
            .ok_or_else(arithmetic)?,
    )?;
    shape.component_edges = add(
        shape.component_edges,
        body.checked_add(enforcement).ok_or_else(arithmetic)?,
    )?;
    Ok(())
}

fn add_variable_shape(shape: &mut Shape) -> Result<(), AssignmentRuleError> {
    shape.variables = add(shape.variables, 1)?;
    shape.provenance = add(shape.provenance, 1)?;
    // Declaration -> fact; fact -> person + authored parent.
    shape.references = add(shape.references, 3)?;
    Ok(())
}

fn bounded_shape(shape: Shape, limits: PlanningIrLimitsV1) -> Result<Shape, AssignmentRuleError> {
    let defaults = PlanningIrLimitsV1::DEFAULT;
    within(
        shape.variables,
        limits.max_variables.min(defaults.max_variables),
        AssignmentRuleLimit::Variables,
    )?;
    within(
        shape.constraints,
        limits.max_constraints.min(defaults.max_constraints),
        AssignmentRuleLimit::Constraints,
    )?;
    within(
        shape.provenance,
        limits
            .max_provenance_records
            .min(defaults.max_provenance_records),
        AssignmentRuleLimit::ProvenanceRecords,
    )?;
    within(
        shape.references,
        limits.max_total_refs.min(defaults.max_total_refs),
        AssignmentRuleLimit::References,
    )?;
    within(
        3,
        limits.max_refs_per_node.min(defaults.max_refs_per_node),
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        2,
        limits
            .max_enforcement_literals
            .min(defaults.max_enforcement_literals),
        AssignmentRuleLimit::PerRecord,
    )?;
    within(1, limits.max_domain_ranges, AssignmentRuleLimit::PerRecord)?;
    within(
        shape.component_edges,
        limits.max_component_edges.min(defaults.max_component_edges),
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok(shape)
}

fn assignment_shape(n: usize) -> Result<Shape, AssignmentRuleError> {
    let n = count(n)?;
    let n_minus_one = n.saturating_sub(1);
    let n_minus_two = n.saturating_sub(2);
    let q = if n >= 3 {
        n_minus_one
            .checked_mul(n_minus_two)
            .and_then(|value| value.checked_div(2))
            .ok_or_else(arithmetic)?
    } else {
        0
    };
    let pairs = n
        .checked_mul(n_minus_one)
        .and_then(|value| value.checked_div(2))
        .ok_or_else(arithmetic)?;
    let variables = n
        .checked_add(n_minus_one)
        .and_then(|value| value.checked_add(q))
        .ok_or_else(arithmetic)?;
    let mut constraints = n.checked_add(1).ok_or_else(arithmetic)?;
    let mut references = variables.checked_mul(3).ok_or_else(arithmetic)?;
    let mut component_edges = 0_u64;
    references = references
        .checked_add(n.checked_mul(5).ok_or_else(arithmetic)?)
        .and_then(|value| value.checked_add(5))
        .ok_or_else(arithmetic)?;
    component_edges = component_edges
        .checked_add(n.checked_mul(2).ok_or_else(arithmetic)?)
        .and_then(|value| value.checked_add(2))
        .ok_or_else(arithmetic)?;
    if n >= 2 {
        let prefix_constraints = 2_u64
            .checked_add(n_minus_two.checked_mul(3).ok_or_else(arithmetic)?)
            .and_then(|value| value.checked_add(n_minus_one))
            .ok_or_else(arithmetic)?;
        constraints = constraints
            .checked_add(prefix_constraints)
            .ok_or_else(arithmetic)?;
        references = add(
            add(
                add(references, 10)?,
                n_minus_two.checked_mul(16).ok_or_else(arithmetic)?,
            )?,
            n_minus_one.checked_mul(6).ok_or_else(arithmetic)?,
        )?;
        component_edges = add(
            add(
                add(component_edges, 4)?,
                n_minus_two.checked_mul(7).ok_or_else(arithmetic)?,
            )?,
            n_minus_one.checked_mul(3).ok_or_else(arithmetic)?,
        )?;
    }
    constraints = constraints
        .checked_add(q.checked_mul(3).ok_or_else(arithmetic)?)
        .and_then(|value| value.checked_add(pairs))
        .ok_or_else(arithmetic)?;
    references = add(
        add(references, q.checked_mul(16).ok_or_else(arithmetic)?)?,
        pairs.checked_mul(7).ok_or_else(arithmetic)?,
    )?;
    component_edges = add(
        add(component_edges, q.checked_mul(7).ok_or_else(arithmetic)?)?,
        pairs.checked_mul(4).ok_or_else(arithmetic)?,
    )?;
    let shape = Shape {
        planned_constraints: 1,
        variables,
        constraints,
        provenance: variables.checked_add(constraints).ok_or_else(arithmetic)?,
        references,
        component_edges,
    };
    bounded_shape(shape, PlanningIrLimitsV1::DEFAULT)
}

pub(super) fn check_assignment_shape(
    n: usize,
    maximum: u32,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    if maximum == 0 {
        let mut shape = Shape {
            planned_constraints: 1,
            ..Shape::default()
        };
        for _ in 0..count(n)? {
            add_constraint_shape(&mut shape, 1, 0)?;
        }
        bounded_shape(shape, limits)?;
        return Ok(());
    }
    check_maximum(maximum, limits)?;
    let shape = assignment_shape(n)?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    within(
        shape.variables,
        limits.max_variables.min(defaults.max_variables),
        AssignmentRuleLimit::Variables,
    )?;
    within(
        shape.constraints,
        limits.max_constraints.min(defaults.max_constraints),
        AssignmentRuleLimit::Constraints,
    )?;
    within(
        shape.provenance,
        limits
            .max_provenance_records
            .min(defaults.max_provenance_records),
        AssignmentRuleLimit::ProvenanceRecords,
    )?;
    within(
        shape.references,
        limits.max_total_refs.min(defaults.max_total_refs),
        AssignmentRuleLimit::References,
    )?;
    within(
        3,
        limits.max_refs_per_node.min(defaults.max_refs_per_node),
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        2,
        limits
            .max_enforcement_literals
            .min(defaults.max_enforcement_literals),
        AssignmentRuleLimit::PerRecord,
    )?;
    within(1, limits.max_domain_ranges, AssignmentRuleLimit::PerRecord)?;
    within(
        shape.component_edges,
        limits.max_component_edges.min(defaults.max_component_edges),
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok(())
}

fn check_worked_shape(
    days: &[ConsecutiveDay],
    maximum: u32,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    check_maximum(maximum, limits)?;
    for day in days {
        within(
            count(day.population.len())?
                .checked_add(1)
                .ok_or_else(arithmetic)?,
            limits
                .max_refs_per_node
                .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    within(
        u64::from(maximum).checked_add(1).ok_or_else(arithmetic)?,
        limits
            .max_refs_per_node
            .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
        AssignmentRuleLimit::PerRecord,
    )?;
    let shape = worked_shape(days, maximum)?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    within(
        shape.variables,
        limits.max_variables.min(defaults.max_variables),
        AssignmentRuleLimit::Variables,
    )?;
    within(
        shape.constraints,
        limits.max_constraints.min(defaults.max_constraints),
        AssignmentRuleLimit::Constraints,
    )?;
    within(
        shape.references,
        limits.max_total_refs.min(defaults.max_total_refs),
        AssignmentRuleLimit::References,
    )?;
    within(
        shape.component_edges,
        limits.max_component_edges.min(defaults.max_component_edges),
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok(())
}

fn window_count(days: &[ConsecutiveDay], maximum: u32) -> Result<u64, AssignmentRuleError> {
    let width = usize::try_from(maximum)
        .ok()
        .and_then(|value| value.checked_add(1));
    let Some(width) = width else {
        return Ok(0);
    };
    if width > days.len() {
        return Ok(0);
    }
    let mut result = 0_u64;
    let mut run = 0_usize;
    let mut previous: Option<Date> = None;
    for day in days {
        let adjacent = previous
            .and_then(|date| date.checked_add(Span::new().days(1)).ok())
            .is_some_and(|date| date == day.date);
        run = if adjacent { run + 1 } else { 1 };
        if run >= width {
            result = result.checked_add(1).ok_or_else(arithmetic)?;
        }
        previous = Some(day.date);
    }
    Ok(result)
}

fn worked_shape(days: &[ConsecutiveDay], maximum: u32) -> Result<Shape, AssignmentRuleError> {
    let mut shape = Shape {
        planned_constraints: 1,
        ..Shape::default()
    };
    for day in days {
        add_variable_shape(&mut shape)?;
        let members = count(day.population.len())?;
        for _ in 0..members {
            add_constraint_shape(&mut shape, 2, 0)?;
        }
        add_constraint_shape(
            &mut shape,
            members.checked_add(1).ok_or_else(arithmetic)?,
            0,
        )?;
    }
    let windows = window_count(days, maximum)?;
    let width = u64::from(maximum).checked_add(1).ok_or_else(arithmetic)?;
    for _ in 0..windows {
        add_constraint_shape(&mut shape, width, 0)?;
    }
    bounded_shape(shape, PlanningIrLimitsV1::DEFAULT)
}

pub(crate) fn shape_constraint(
    planned: &PlannedConstraint,
    plan: &Plan,
    limits: PlanningIrLimitsV1,
) -> Result<Shape, AssignmentRuleError> {
    let (data, _) = data(planned, plan)?;
    let shape = if data.maximum == 0 {
        let mut shape = Shape {
            planned_constraints: 1,
            ..Shape::default()
        };
        for _ in &planned.population {
            add_constraint_shape(&mut shape, 1, 0)?;
        }
        shape
    } else {
        check_maximum(data.maximum, limits)?;
        match data.mode {
            ConsecutiveMode::WorkedDays {} => {
                for day in &data.days {
                    within(
                        count(day.population.len())?
                            .checked_add(1)
                            .ok_or_else(arithmetic)?,
                        limits
                            .max_refs_per_node
                            .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                        AssignmentRuleLimit::PerRecord,
                    )?;
                }
                within(
                    u64::from(data.maximum)
                        .checked_add(1)
                        .ok_or_else(arithmetic)?,
                    limits
                        .max_refs_per_node
                        .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                    AssignmentRuleLimit::PerRecord,
                )?;
                worked_shape(&data.days, data.maximum)?
            }
            ConsecutiveMode::Assignments { .. } => assignment_shape(planned.population.len())?,
        }
    };
    bounded_shape(shape, limits)
}

pub(crate) fn capabilities(
    planned: &PlannedConstraint,
    plan: &Plan,
    capabilities: &mut BTreeSet<Capability>,
) -> Result<(), AssignmentRuleError> {
    let (data, _) = data(planned, plan)?;
    if data.maximum == 0 {
        capabilities.insert(Capability::BoolOr);
    } else {
        match data.mode {
            ConsecutiveMode::WorkedDays {} => {
                capabilities.insert(Capability::BoolOr);
                capabilities.insert(Capability::CardinalityRange);
            }
            ConsecutiveMode::Assignments { .. } => {
                capabilities.insert(Capability::BoolOr);
                capabilities.insert(Capability::LinearComparison);
            }
        }
    }
    Ok(())
}

pub(crate) fn shape(plan: &Plan, limits: PlanningIrLimitsV1) -> Result<Shape, AssignmentRuleError> {
    let mut result = Shape::default();
    for planned in &plan.constraints {
        if is(planned) {
            result = result.add(shape_constraint(planned, plan, limits)?)?;
        }
    }
    Ok(result)
}

fn pair_offset(i: usize, n: usize) -> usize {
    i * (2 * n - i - 1) / 2
}

fn join_at(joins: &[bool], i: usize, j: usize, n: usize) -> Result<bool, AssignmentRuleError> {
    if i >= j || j >= n {
        return Err(invalid());
    }
    let offset = pair_offset(i, n)
        .checked_add(j - i - 1)
        .ok_or_else(arithmetic)?;
    joins.get(offset).copied().ok_or_else(invalid)
}

fn int_id(value: &str) -> Result<IntVariableId, AssignmentRuleError> {
    IntVariableId::new(value.to_owned()).map_err(|_| invalid())
}

fn helper_fact(
    rule: RuleId,
    person: PersonId,
    id: ProvenanceId,
    parent: ProvenanceId,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Derived,
        source_id: rule.to_string(),
        entity_refs: vec![entity("person", EntityId::from_uuid(person.as_uuid()))?],
        message_key: HELPER_MESSAGE.to_owned(),
        parameters: BTreeMap::new(),
        parent: Some(parent),
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_bool<K: Serialize>(
    rule: RuleId,
    person: PersonId,
    key: &K,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    variables: &mut Vec<Variable>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<BoolVariable, AssignmentRuleError> {
    let identity_key = ("maximum_consecutive", rule, person, key);
    let id =
        BoolVariableId::new(identities.derive(IdentityKind::Boolean, &identity_key, budget)?)
            .map_err(|_| invalid())?;
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("maximum_consecutive_variable_fact", rule, person, key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact = helper_fact(rule, person, fact_id.clone(), parent)?;
    budget.reserve(1, 2, 512)?;
    provenance.push(fact);
    let variable = BoolVariable {
        id: id.clone(),
        provenance: fact_id,
    };
    variables.push(Variable::Boolean(variable.clone()));
    Ok(variable)
}

#[allow(clippy::too_many_arguments)]
fn emit_int<K: Serialize>(
    rule: RuleId,
    person: PersonId,
    key: &K,
    maximum: u32,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    variables: &mut Vec<Variable>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<IntVariable, AssignmentRuleError> {
    budget.step()?;
    within(
        i64::from(maximum).unsigned_abs(),
        u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
        AssignmentRuleLimit::PerRecord,
    )?;
    let identity_key = ("maximum_consecutive", rule, person, key);
    let id = IntVariableId::new(identities.derive(IdentityKind::Integer, &identity_key, budget)?)
        .map_err(|_| invalid())?;
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("maximum_consecutive_variable_fact", rule, person, key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact = helper_fact(rule, person, fact_id.clone(), parent)?;
    budget.reserve(1, 2, 512)?;
    provenance.push(fact);
    let variable = IntVariable {
        id: id.clone(),
        domain: IntDomain {
            inclusive_ranges: vec![InclusiveRange {
                start: 0,
                end: i64::from(maximum),
            }],
        },
        provenance: fact_id,
    };
    variables.push(Variable::Integer(variable.clone()));
    Ok(variable)
}

fn linear(
    terms: Vec<LinearTerm>,
    op: ComparisonOp,
    rhs: i64,
    limits: PlanningIrLimitsV1,
) -> Result<Constraint, AssignmentRuleError> {
    let coefficient_limit = limits
        .max_abs_coefficient
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_coefficient);
    let value_limit = limits
        .max_abs_value
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_value);
    if terms
        .iter()
        .any(|term| term.coefficient.abs() > coefficient_limit)
        || rhs.abs() > value_limit
    {
        return Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord,
        ));
    }
    let expression = LinearExpression::new(terms, 0).map_err(|_| invalid())?;
    Ok(Constraint::LinearComparison(LinearComparison {
        expression,
        op,
        rhs,
    }))
}

#[allow(clippy::too_many_arguments)]
fn emit_constraint<K: Serialize>(
    rule: RuleId,
    person: PersonId,
    key: &K,
    body: Constraint,
    enforcement: Vec<Literal>,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    within(
        count(enforcement.len())?,
        limits
            .max_enforcement_literals
            .min(PlanningIrLimitsV1::DEFAULT.max_enforcement_literals),
        AssignmentRuleLimit::PerRecord,
    )?;
    let id = PlanningConstraintId::new(identities.derive(
        IdentityKind::Constraint,
        &("maximum_consecutive_constraint", rule, person, key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("maximum_consecutive_constraint_fact", rule, person, key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let fact = helper_fact(rule, person, fact_id.clone(), parent)?;
    budget.reserve(1, 3, 768)?;
    provenance.push(fact);
    constraints.push(ConstraintRecord {
        id,
        body,
        enforcement,
        provenance: fact_id,
        tags: Vec::new(),
    });
    Ok(())
}

fn x_literal(variable: &BoolVariable) -> Literal {
    Literal::positive(variable.id.clone())
}

fn not_literal(variable: &BoolVariable) -> Literal {
    Literal::negative(variable.id.clone())
}

// Keep the exact predecessor channel and guarded run transitions together for audit.
#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
fn emit_assignments(
    planned: &PlannedConstraint,
    data: &ConsecutivePlan,
    person: PersonId,
    variables: DecisionVariables<'_>,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    output: &mut Vec<Variable>,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let n = planned.population.len();
    if data.maximum == 0 {
        for (position, index) in planned.population.iter().enumerate() {
            let x = variables.boolean(*index).ok_or_else(invalid)?;
            emit_constraint(
                planned.rule,
                person,
                &("forbid", position),
                Constraint::bool_or(vec![not_literal(x)]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
        return Ok(());
    }
    let mut x = Vec::with_capacity(n);
    for index in &planned.population {
        x.push(variables.boolean(*index).ok_or_else(invalid)?);
    }
    let mut runs = Vec::with_capacity(n);
    for (j, _) in x.iter().enumerate() {
        runs.push(emit_int(
            planned.rule,
            person,
            &("run", j),
            data.maximum,
            parent.clone(),
            identities,
            output,
            provenance,
            budget,
            limits,
        )?);
    }
    for (j, run) in runs.iter().enumerate() {
        emit_constraint(
            planned.rule,
            person,
            &("absent", j),
            linear(
                vec![LinearTerm {
                    variable: run.id.clone(),
                    coefficient: 1,
                }],
                ComparisonOp::Equal,
                0,
                limits,
            )?,
            vec![not_literal(x[j])],
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
            limits,
        )?;
    }
    emit_constraint(
        planned.rule,
        person,
        &("first", 0_usize),
        linear(
            vec![LinearTerm {
                variable: runs[0].id.clone(),
                coefficient: 1,
            }],
            ComparisonOp::Equal,
            1,
            limits,
        )?,
        vec![x_literal(x[0])],
        parent.clone(),
        identities,
        constraints,
        provenance,
        budget,
        limits,
    )?;
    let mut prefixes = Vec::with_capacity(n.saturating_sub(1));
    for j in 1..n {
        let prefix = emit_bool(
            planned.rule,
            person,
            &("prefix", j),
            parent.clone(),
            identities,
            output,
            provenance,
            budget,
        )?;
        if j == 1 {
            emit_constraint(
                planned.rule,
                person,
                &("prefix_not", j),
                Constraint::bool_or(vec![not_literal(&prefix), not_literal(x[0])]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
            emit_constraint(
                planned.rule,
                person,
                &("prefix_yes", j),
                Constraint::bool_or(vec![x_literal(x[0]), x_literal(&prefix)]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        } else {
            let previous = &prefixes[j - 2];
            emit_constraint(
                planned.rule,
                person,
                &("prefix_prev", j),
                Constraint::bool_or(vec![not_literal(&prefix), x_literal(previous)]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
            emit_constraint(
                planned.rule,
                person,
                &("prefix_gap", j),
                Constraint::bool_or(vec![not_literal(&prefix), not_literal(x[j - 1])]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
            emit_constraint(
                planned.rule,
                person,
                &("prefix_back", j),
                Constraint::bool_or(vec![
                    not_literal(previous),
                    x_literal(x[j - 1]),
                    x_literal(&prefix),
                ]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
        prefixes.push(prefix);
    }
    for j in 1..n {
        emit_constraint(
            planned.rule,
            person,
            &("prefix_run", j),
            linear(
                vec![LinearTerm {
                    variable: runs[j].id.clone(),
                    coefficient: 1,
                }],
                ComparisonOp::Equal,
                1,
                limits,
            )?,
            vec![x_literal(x[j]), x_literal(&prefixes[j - 1])],
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
            limits,
        )?;
    }
    for i in 0..n {
        for j in (i + 2)..n {
            let q = emit_bool(
                planned.rule,
                person,
                &("q", i, j),
                parent.clone(),
                identities,
                output,
                provenance,
                budget,
            )?;
            let previous = if j == i + 2 {
                x[i].clone()
            } else {
                let previous_id = identities.derive(
                    IdentityKind::Boolean,
                    &("maximum_consecutive", planned.rule, person, ("q", i, j - 1)),
                    budget,
                )?;
                BoolVariable {
                    id: BoolVariableId::new(previous_id).map_err(|_| invalid())?,
                    provenance: q.provenance.clone(),
                }
            };
            emit_constraint(
                planned.rule,
                person,
                &("q_prev", i, j),
                Constraint::bool_or(vec![not_literal(&q), x_literal(&previous)]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
            emit_constraint(
                planned.rule,
                person,
                &("q_gap", i, j),
                Constraint::bool_or(vec![not_literal(&q), not_literal(x[j - 1])]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
            emit_constraint(
                planned.rule,
                person,
                &("q_back", i, j),
                Constraint::bool_or(vec![
                    not_literal(&previous),
                    x_literal(x[j - 1]),
                    x_literal(&q),
                ]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
    }
    for j in 1..n {
        for i in 0..j {
            let predecessor = if i + 1 == j {
                x[i].clone()
            } else {
                let q_id = identities.derive(
                    IdentityKind::Boolean,
                    &("maximum_consecutive", planned.rule, person, ("q", i, j)),
                    budget,
                )?;
                BoolVariable {
                    id: BoolVariableId::new(q_id).map_err(|_| invalid())?,
                    provenance: x[i].provenance.clone(),
                }
            };
            let body = if join_at(&data.joins, i, j, n)? {
                linear(
                    vec![
                        LinearTerm {
                            variable: runs[j].id.clone(),
                            coefficient: 1,
                        },
                        LinearTerm {
                            variable: runs[i].id.clone(),
                            coefficient: -1,
                        },
                    ],
                    ComparisonOp::Equal,
                    1,
                    limits,
                )?
            } else {
                linear(
                    vec![LinearTerm {
                        variable: runs[j].id.clone(),
                        coefficient: 1,
                    }],
                    ComparisonOp::Equal,
                    1,
                    limits,
                )?
            };
            emit_constraint(
                planned.rule,
                person,
                &("transition", i, j),
                body,
                vec![x_literal(x[j]), x_literal(&predecessor)],
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
    }
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
fn emit_worked_days(
    planned: &PlannedConstraint,
    data: &ConsecutivePlan,
    person: PersonId,
    variables: DecisionVariables<'_>,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    output: &mut Vec<Variable>,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    if data.maximum == 0 {
        for (position, index) in planned.population.iter().enumerate() {
            let x = variables.boolean(*index).ok_or_else(invalid)?;
            emit_constraint(
                planned.rule,
                person,
                &("forbid", position),
                Constraint::bool_or(vec![not_literal(x)]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
        return Ok(());
    }
    let mut days = Vec::with_capacity(data.days.len());
    for day in &data.days {
        let boolean = emit_bool(
            planned.rule,
            person,
            &("day", day.date),
            parent.clone(),
            identities,
            output,
            provenance,
            budget,
        )?;
        for (member, index) in day.population.iter().enumerate() {
            let x = variables.boolean(*index).ok_or_else(invalid)?;
            emit_constraint(
                planned.rule,
                person,
                &("day_member", day.date, member),
                Constraint::bool_or(vec![not_literal(x), x_literal(&boolean)]),
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
        let mut literals = Vec::with_capacity(day.population.len() + 1);
        literals.push(not_literal(&boolean));
        for index in &day.population {
            literals.push(x_literal(variables.boolean(*index).ok_or_else(invalid)?));
        }
        emit_constraint(
            planned.rule,
            person,
            &("day_or", day.date),
            Constraint::bool_or(literals),
            Vec::new(),
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
            limits,
        )?;
        days.push(boolean);
    }
    let width = usize::try_from(data.maximum)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(arithmetic)?;
    let mut run = 0_usize;
    let mut previous: Option<Date> = None;
    for (end, day) in data.days.iter().enumerate() {
        let adjacent = previous
            .and_then(|date| date.checked_add(Span::new().days(1)).ok())
            .is_some_and(|date| date == day.date);
        if adjacent {
            run += 1;
        } else {
            run = 1;
        }
        if run >= width {
            let literals = days[(end + 1 - width)..=end]
                .iter()
                .map(x_literal)
                .collect();
            emit_constraint(
                planned.rule,
                person,
                &("window", end + 1 - width, end),
                Constraint::cardinality(literals, 0, u64::from(data.maximum))
                    .map_err(|_| invalid())?,
                Vec::new(),
                parent.clone(),
                identities,
                constraints,
                provenance,
                budget,
                limits,
            )?;
        }
        previous = Some(day.date);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit(
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
    match data.mode {
        ConsecutiveMode::WorkedDays {} => emit_worked_days(
            planned,
            data,
            person,
            variables,
            parent,
            identities,
            output,
            constraints,
            provenance,
            budget,
            limits,
        ),
        ConsecutiveMode::Assignments { .. } => emit_assignments(
            planned,
            data,
            person,
            variables,
            parent,
            identities,
            output,
            constraints,
            provenance,
            budget,
            limits,
        ),
    }
}

// ---- bounded IR preflight -------------------------------------------------
// Negative literals serialize as `false`, one byte longer than `true`. Measure that
// upper bound for every borrowed template so mixed-polarity clauses never undercharge.

struct MeasuredLiterals(usize, bool);

impl Serialize for MeasuredLiterals {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0))?;
        for index in 0..self.0 {
            sequence.serialize_element(&MeasuredLiteral {
                variable: if self.1 && index & 1 == 1 {
                    MEASURE_BOOL_ALT_ID
                } else {
                    MEASURE_BOOL_ID
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
#[serde(rename_all = "camelCase")]
struct MeasuredRange {
    start: i64,
    end: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredDomain {
    inclusive_ranges: [MeasuredRange; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredInteger {
    id: &'static str,
    domain: MeasuredDomain,
    provenance: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredVariable {
    Boolean {
        id: &'static str,
        provenance: &'static str,
    },
    Integer(MeasuredInteger),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredExpression {
    terms: MeasuredTerms,
    constant: i64,
}

struct MeasuredTerms {
    count: usize,
    two: bool,
}
impl Serialize for MeasuredTerms {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.count))?;
        for index in 0..self.count {
            sequence.serialize_element(&LinearTerm {
                variable: if self.two && index & 1 == 1 {
                    int_id(MEASURE_INT_ALT_ID).map_err(|_| serde::ser::Error::custom("id"))?
                } else {
                    int_id(MEASURE_INT_ID).map_err(|_| serde::ser::Error::custom("id"))?
                },
                coefficient: if self.two && index == 1 { -1 } else { 1 },
            })?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredComparison {
    expression: MeasuredExpression,
    op: ComparisonOp,
    rhs: i64,
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
    LinearComparison(MeasuredComparison),
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

fn measured_fact(rule: RuleId, person: PersonId) -> Result<ProvenanceRecord, AssignmentRuleError> {
    helper_fact(
        rule,
        person,
        ProvenanceId::new(MEASURE_PROVENANCE_ID).map_err(|_| invalid())?,
        ProvenanceId::new(MEASURE_PROVENANCE_ID).map_err(|_| invalid())?,
    )
}

fn charge_fact(
    rule: RuleId,
    person: PersonId,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    budget.reserve(1, 2, 512)?;
    reserve_fact(&measured_fact(rule, person)?, budget, limits)
}

fn charge_variable(
    rule: RuleId,
    person: PersonId,
    variable: &MeasuredVariable,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let bytes = budget.measure_ir(variable)?;
    budget.reserve_ir(1, 1, bytes)?;
    charge_fact(rule, person, budget, limits)
}

fn measured_constraint(body: MeasuredBody, enforcement: usize) -> MeasuredConstraint {
    MeasuredConstraint {
        id: MEASURE_CONSTRAINT_ID,
        body,
        enforcement: MeasuredLiterals(enforcement, false),
        provenance: MEASURE_PROVENANCE_ID,
        tags: [],
    }
}

fn charge_constraint(
    rule: RuleId,
    person: PersonId,
    body: MeasuredBody,
    body_refs: u64,
    enforcement: usize,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let record = measured_constraint(body, enforcement);
    let refs = body_refs
        .checked_add(count(enforcement)?)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(arithmetic)?;
    budget.reserve(1, 3, 768)?;
    let bytes = budget.measure_ir(&record)?;
    budget.reserve_ir(1, refs, bytes)?;
    let fact = measured_fact(rule, person)?;
    reserve_fact(&fact, budget, limits)
}

fn measured_bool_variable() -> MeasuredVariable {
    MeasuredVariable::Boolean {
        id: MEASURE_BOOL_ID,
        provenance: MEASURE_PROVENANCE_ID,
    }
}

fn measured_int_variable(maximum: u32) -> MeasuredVariable {
    MeasuredVariable::Integer(MeasuredInteger {
        id: MEASURE_INT_ID,
        domain: MeasuredDomain {
            inclusive_ranges: [MeasuredRange {
                start: 0,
                end: i64::from(maximum),
            }],
        },
        provenance: MEASURE_PROVENANCE_ID,
    })
}

fn measured_linear(body_terms: usize, rhs: i64, _enforcement: usize) -> MeasuredBody {
    MeasuredBody::LinearComparison(MeasuredComparison {
        expression: MeasuredExpression {
            terms: MeasuredTerms {
                count: body_terms,
                two: body_terms == 2,
            },
            constant: 0,
        },
        op: ComparisonOp::Equal,
        rhs,
    })
}

#[allow(clippy::too_many_lines)]
fn preflight_assignments(
    planned: &PlannedConstraint,
    data: &ConsecutivePlan,
    person: PersonId,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let n = planned.population.len();
    if data.maximum == 0 {
        for _ in 0..n {
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(1, false),
                },
                1,
                0,
                budget,
                limits,
            )?;
        }
        return Ok(());
    }
    for _ in 0..n {
        charge_variable(
            planned.rule,
            person,
            &measured_int_variable(data.maximum),
            budget,
            limits,
        )?;
    }
    for _ in 0..n.saturating_sub(1) {
        charge_variable(
            planned.rule,
            person,
            &measured_bool_variable(),
            budget,
            limits,
        )?;
    }
    for _ in 0..q_count(n)? {
        charge_variable(
            planned.rule,
            person,
            &measured_bool_variable(),
            budget,
            limits,
        )?;
    }
    for _ in 0..n {
        charge_constraint(
            planned.rule,
            person,
            measured_linear(1, 0, 1),
            1,
            1,
            budget,
            limits,
        )?;
    }
    charge_constraint(
        planned.rule,
        person,
        measured_linear(1, 1, 1),
        1,
        1,
        budget,
        limits,
    )?;
    if n >= 2 {
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::BoolOr {
                literals: MeasuredLiterals(2, true),
            },
            2,
            0,
            budget,
            limits,
        )?;
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::BoolOr {
                literals: MeasuredLiterals(2, true),
            },
            2,
            0,
            budget,
            limits,
        )?;
        for _ in 2..n {
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(2, true),
                },
                2,
                0,
                budget,
                limits,
            )?;
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(2, true),
                },
                2,
                0,
                budget,
                limits,
            )?;
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(3, true),
                },
                3,
                0,
                budget,
                limits,
            )?;
        }
        for _ in 1..n {
            charge_constraint(
                planned.rule,
                person,
                measured_linear(1, 1, 2),
                1,
                2,
                budget,
                limits,
            )?;
        }
    }
    for _ in 0..q_count(n)? {
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::BoolOr {
                literals: MeasuredLiterals(2, true),
            },
            2,
            0,
            budget,
            limits,
        )?;
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::BoolOr {
                literals: MeasuredLiterals(2, true),
            },
            2,
            0,
            budget,
            limits,
        )?;
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::BoolOr {
                literals: MeasuredLiterals(3, true),
            },
            3,
            0,
            budget,
            limits,
        )?;
    }
    for _ in 0..pair_count(n)? {
        charge_constraint(
            planned.rule,
            person,
            measured_linear(2, 1, 2),
            2,
            2,
            budget,
            limits,
        )?;
    }
    Ok(())
}

fn preflight_worked_days(
    planned: &PlannedConstraint,
    data: &ConsecutivePlan,
    person: PersonId,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    if data.maximum == 0 {
        for _ in &planned.population {
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(1, false),
                },
                1,
                0,
                budget,
                limits,
            )?;
        }
        return Ok(());
    }
    for day in &data.days {
        charge_variable(
            planned.rule,
            person,
            &measured_bool_variable(),
            budget,
            limits,
        )?;
        for _ in &day.population {
            charge_constraint(
                planned.rule,
                person,
                MeasuredBody::BoolOr {
                    literals: MeasuredLiterals(2, true),
                },
                2,
                0,
                budget,
                limits,
            )?;
        }
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::BoolOr {
                literals: MeasuredLiterals(day.population.len() + 1, true),
            },
            count(day.population.len())?
                .checked_add(1)
                .ok_or_else(arithmetic)?,
            0,
            budget,
            limits,
        )?;
    }
    let windows = window_count(&data.days, data.maximum)?;
    for _ in 0..windows {
        charge_constraint(
            planned.rule,
            person,
            MeasuredBody::CardinalityRange {
                literals: MeasuredLiterals(
                    usize::try_from(data.maximum).map_err(|_| arithmetic())? + 1,
                    true,
                ),
                min: 0,
                max: u64::from(data.maximum),
            },
            u64::from(data.maximum) + 1,
            0,
            budget,
            limits,
        )?;
    }
    Ok(())
}

/// Charge all consecutive helper declarations/records exactly once for contribution and complete
/// compilation. Records are measured as borrowed one-record templates; no quadratic graph is
/// retained by the measurement pass.
pub(crate) fn preflight_contribution(
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    for planned in &plan.constraints {
        if !is(planned) {
            continue;
        }
        let (data, person) = data(planned, plan)?;
        budget.step()?;
        match data.mode {
            ConsecutiveMode::WorkedDays {} => {
                preflight_worked_days(planned, data, person, budget, limits)?;
            }
            ConsecutiveMode::Assignments { .. } => {
                preflight_assignments(planned, data, person, budget, limits)?;
            }
        }
    }
    budget.check()
}

/// Complete-model root serialization has an enum wrapper for every helper variable. Contribution
/// preflight already charges each declaration; this pass returns only the additional wrapper bytes
/// which the complete root's empty-array measurement cannot see.
pub(crate) fn helper_variable_bytes(
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<u64, AssignmentRuleError> {
    let mut bytes = 0_u64;
    for planned in &plan.constraints {
        if !is(planned) {
            continue;
        }
        let (data, _) = data(planned, plan)?;
        let mut charge = |variable: MeasuredVariable| -> Result<(), AssignmentRuleError> {
            budget.step()?;
            bytes = add(bytes, budget.measure_ir(&variable)?)?;
            Ok(())
        };
        if data.maximum == 0 {
            continue;
        }
        match data.mode {
            ConsecutiveMode::WorkedDays {} => {
                for _ in &data.days {
                    charge(measured_bool_variable())?;
                }
            }
            ConsecutiveMode::Assignments { .. } => {
                for _ in &planned.population {
                    charge(measured_int_variable(data.maximum))?;
                }
                for _ in 0..planned.population.len().saturating_sub(1) {
                    charge(measured_bool_variable())?;
                }
                for _ in 0..q_count(planned.population.len())? {
                    charge(measured_bool_variable())?;
                }
            }
        }
    }
    Ok(bytes)
}
