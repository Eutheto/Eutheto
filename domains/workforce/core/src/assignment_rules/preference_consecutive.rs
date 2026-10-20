use super::{
    AssignmentConstructionIssue, AssignmentRuleError, AssignmentRuleLimit, InstantInterval,
    analysis::support,
    budget::{OperationBudget, count, within},
    identity::IdentityKind,
    input::AssignmentInput,
    reporting::ReportingWindows,
    weekends,
};
use crate::{
    ids::AssignmentTypeId,
    model::{
        AssignmentPair, CalendarPeriod, ConsecutiveWorkMode, Scope, Weekday, WorkforceEntity,
        WorkforcePreference,
    },
    temporal::{ResolvedShift, resolve_interval},
};
use eutheto_domain_ir::{DomainEntityId, DomainEntityKindId, DomainEntityRef};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, ComparisonOp, Constraint, ConstraintRecord, InclusiveRange,
    IntDomain, IntVariable, IntVariableId, LinearComparison, LinearExpression, LinearTerm, Literal,
    ObjectiveTerm, ObjectiveTermId, ObjectiveTermKind, PlanningConstraintId, PlanningIrLimitsV1,
    ProvenanceId, ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind, Variable,
};
use eutheto_types::{EntityId, PersonId, RuleId, ScenarioDocument, ScenarioSettings};
use jiff::{
    Span,
    civil::{Date, Time},
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Stable score category for the source and compiled consecutive-work contribution.
pub(super) const CATEGORY: &str = "official.workforce.score.preference.consecutive_work";

/// One source-owned reporting date. Include scoped person/shift pairs even when they are not
/// currently eligible: an opportunity cannot disappear because another rule excludes it.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Day {
    pub date: Date,
    pub pairs: Vec<AssignmentPair>,
}

/// One penalty window. A width of `threshold + 1` means each selected consecutive window is one
/// unit of excess work; a run of length `threshold + n` contributes `n` windows.
#[derive(Clone, Debug)]
pub(super) struct Window {
    pub person: PersonId,
    pub days: Vec<Day>,
}

#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub id: RuleId,
    pub level: usize,
    pub coefficient: i64,
    pub threshold: u32,
    pub weekend: bool,
    pub windows: Vec<Window>,
}

#[derive(Clone, Debug)]
pub(super) struct DayShape {
    pub date: Date,
    pub population: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct WindowShape {
    pub plan: usize,
    pub person: PersonId,
    pub days: Vec<DayShape>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Shape {
    pub windows: Vec<WindowShape>,
    pub term_count: u64,
    pub variables: u64,
    pub constraints: u64,
    pub provenance: u64,
    pub references: u64,
    pub component_edges: u64,
}

#[derive(Clone, Debug)]
pub(super) struct Ids {
    pub fact: ProvenanceId,
    pub day_variables: Vec<BoolVariableId>,
    pub window_variable: BoolVariableId,
    pub window_integer: IntVariableId,
    pub member_constraints: Vec<PlanningConstraintId>,
    pub day_constraints: Vec<PlanningConstraintId>,
    pub window_constraints: Vec<PlanningConstraintId>,
    pub window_or_constraint: PlanningConstraintId,
    pub window_channels: [PlanningConstraintId; 2],
    pub term: ObjectiveTermId,
}

pub(super) struct Emission {
    pub variables: Vec<Variable>,
    pub constraints: Vec<ConstraintRecord>,
    pub term: ObjectiveTerm,
    pub fact: ProvenanceRecord,
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

/// Reporting-day mode and the source-owned weekend mode are both supported. Weekend mode uses the
/// authored weekday set only as a positive local Sat/Sun overlap filter; other weekdays do not get
/// reinterpreted as another weekend definition.
pub(super) fn supports(mode: &ConsecutiveWorkMode) -> bool {
    matches!(
        mode,
        ConsecutiveWorkMode::ReportingDays {} | ConsecutiveWorkMode::Weekends { .. }
    )
}
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn plan(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    preference: &WorkforcePreference,
    id: RuleId,
    level: usize,
    coefficient: i64,
    scope: &Scope,
    mode: &ConsecutiveWorkMode,
    assignment_type_ids: &[AssignmentTypeId],
    threshold: u32,
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<Plan>, AssignmentRuleError> {
    if !supports(mode) {
        return Ok(None);
    }
    if !matches!(preference, WorkforcePreference::ConsecutiveWork { .. }) {
        return Err(invalid());
    }
    within(
        u64::from(threshold),
        u64::try_from(limits.max_abs_value).map_err(|_| arithmetic())?,
        AssignmentRuleLimit::PerRecord,
    )?;
    if let ConsecutiveWorkMode::Weekends { calendar_id, .. } = mode {
        let Some(WorkforceEntity::Calendar(calendar)) =
            input.domain.entities.get(&calendar_id.as_entity_id())
        else {
            return Err(invalid());
        };
        if !matches!(&calendar.period, CalendarPeriod::Week { .. }) {
            return Err(invalid());
        }
    }

    // Group source opportunities by reporting date, not by selected or statically eligible
    // assignments. A missing source date is a real break; no neighboring shift is inferred.
    let mut grouped = BTreeMap::<PersonId, BTreeMap<Date, Vec<AssignmentPair>>>::new();
    let mut reporting = ReportingWindows::new();
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !support::person_scope(scope, person, budget)? {
            continue;
        }
        for shift in &input.shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !support::shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            budget.steps(count(assignment_type_ids.len())?)?;
            if !assignment_type_ids.contains(&metadata.assignment_type.id) {
                continue;
            }
            let pair = AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            };
            match mode {
                ConsecutiveWorkMode::ReportingDays {} => {
                    insert_pair(&mut grouped, *person_id, shift.reporting_date, pair, budget)?;
                }
                ConsecutiveWorkMode::Weekends {
                    calendar_id,
                    weekdays,
                } => {
                    let source_weekends = weekends::for_shift(shift, &document.settings, budget)?;
                    for weekend in source_weekends {
                        budget.step()?;
                        if reporting
                            .resolve(document, input, *calendar_id, weekend.date, budget)?
                            .is_none()
                            || !weekend_matches_weekdays(
                                &weekend,
                                shift,
                                weekdays,
                                &document.settings,
                                budget,
                            )?
                        {
                            continue;
                        }
                        insert_pair(&mut grouped, *person_id, weekend.date, pair, budget)?;
                    }
                }
            }
        }
    }

    let width = usize::try_from(threshold)
        .ok()
        .and_then(|value| value.checked_add(1));
    let Some(width) = width else {
        return Ok(Some(Plan {
            id,
            level,
            coefficient,
            threshold,
            weekend: matches!(mode, ConsecutiveWorkMode::Weekends { .. }),
            windows: Vec::new(),
        }));
    };
    let mut windows = Vec::new();
    for (person, days) in grouped {
        budget.step()?;
        let dates = days.keys().copied().collect::<Vec<_>>();
        let mut run_start = 0_usize;
        let step_days = match mode {
            ConsecutiveWorkMode::ReportingDays {} => 1,
            ConsecutiveWorkMode::Weekends { .. } => 7,
        };
        for index in 0..=dates.len() {
            budget.step()?;
            let contiguous = index < dates.len()
                && (index == run_start
                    || dates
                        .get(index - 1)
                        .and_then(|date| date.checked_add(Span::new().days(step_days)).ok())
                        .is_some_and(|date| date == dates[index]));
            if contiguous {
                continue;
            }
            let run_end = index;
            if run_end.saturating_sub(run_start) >= width {
                for start in run_start..=run_end - width {
                    budget.step()?;
                    let mut window_days = Vec::with_capacity(width);
                    for date in &dates[start..start + width] {
                        budget.step()?;
                        let pairs = days.get(date).ok_or_else(invalid)?;
                        budget.reserve(0, 1, 32)?;
                        window_days.push(Day {
                            date: *date,
                            pairs: pairs.clone(),
                        });
                    }
                    budget.reserve(1, 1, budget.measure(&window_days)?)?;
                    windows.push(Window {
                        person,
                        days: window_days,
                    });
                }
            }
            if index < dates.len() {
                run_start = index;
            }
        }
    }
    budget.sort_work(windows.len())?;
    windows.sort_unstable_by_key(|window| (window.person, window.days[0].date));
    Ok(Some(Plan {
        id,
        level,
        coefficient,
        threshold,
        weekend: matches!(mode, ConsecutiveWorkMode::Weekends { .. }),
        windows,
    }))
}

fn insert_pair(
    grouped: &mut BTreeMap<PersonId, BTreeMap<Date, Vec<AssignmentPair>>>,
    person: PersonId,
    date: Date,
    pair: AssignmentPair,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let person_days = grouped.entry(person).or_default();
    if let Some(pairs) = person_days.get_mut(&date) {
        budget.reserve(0, 1, 32)?;
        pairs.push(pair);
    } else {
        budget.reserve(0, 2, 64)?;
        person_days.insert(date, vec![pair]);
    }
    Ok(())
}

fn weekend_matches_weekdays(
    weekend: &weekends::WorkedWeekend,
    shift: &ResolvedShift,
    weekdays: &[Weekday],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let mut date = weekend.date;
    for expected in [Weekday::Saturday, Weekday::Sunday] {
        budget.steps(count(weekdays.len())?)?;
        if weekdays.contains(&expected) {
            let end = date
                .checked_add(Span::new().days(1))
                .map_err(|_| arithmetic())?;
            let resolved = resolve_interval(
                date.to_datetime(Time::MIN),
                end.to_datetime(Time::MIN),
                settings,
                shift.id.as_entity_id(),
            )?;
            if (InstantInterval {
                start: shift.interval.starts_at.instant,
                end: shift.interval.ends_at.instant,
            })
            .intersection(InstantInterval {
                start: resolved.starts_at.instant,
                end: resolved.ends_at.instant,
            })
            .is_some()
            {
                return Ok(true);
            }
        }
        date = date
            .checked_add(Span::new().days(1))
            .map_err(|_| arithmetic())?;
    }
    Ok(false)
}

#[allow(clippy::too_many_lines)]
pub(super) fn shape(
    plans: &[Plan],
    candidates: &[AssignmentPair],
    limits: PlanningIrLimitsV1,
    budget: &mut OperationBudget<'_>,
) -> Result<Shape, AssignmentRuleError> {
    let mut shape = Shape::default();
    for (plan_index, plan) in plans.iter().enumerate() {
        budget.step()?;
        for window in &plan.windows {
            budget.step()?;
            let mut days = Vec::with_capacity(window.days.len());
            let mut constraints = 0_u64;
            let mut references = 0_u64;
            let mut edges = 0_u64;
            let mut valid = true;
            for day in &window.days {
                budget.step()?;
                let mut population = Vec::with_capacity(day.pairs.len());
                for pair in &day.pairs {
                    budget.step()?;
                    if let Ok(index) = candidates.binary_search(pair) {
                        budget.reserve(0, 1, 8)?;
                        population.push(index);
                    }
                }
                if population.is_empty() {
                    valid = false;
                }
                days.push(DayShape {
                    date: day.date,
                    population,
                });
            }
            if !valid {
                continue;
            }
            for day in &days {
                let members = count(day.population.len())?;
                let day_or_refs = members.checked_add(1).ok_or_else(arithmetic)?;
                within(
                    day_or_refs,
                    limits
                        .max_refs_per_node
                        .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                    AssignmentRuleLimit::PerRecord,
                )?;
                constraints = constraints
                    .checked_add(members.checked_add(2).ok_or_else(arithmetic)?)
                    .ok_or_else(arithmetic)?;
                references = references
                    .checked_add(
                        members
                            .checked_mul(2)
                            .and_then(|value| value.checked_add(day_or_refs))
                            .ok_or_else(arithmetic)?,
                    )
                    .ok_or_else(arithmetic)?;
                edges = edges
                    .checked_add(
                        members
                            .checked_add(day_or_refs.saturating_sub(1))
                            .ok_or_else(arithmetic)?,
                    )
                    .ok_or_else(arithmetic)?;
            }
            let width = count(days.len())?;
            let window_refs = width.checked_add(1).ok_or_else(arithmetic)?;
            within(
                2,
                limits
                    .max_refs_per_node
                    .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                AssignmentRuleLimit::PerRecord,
            )?;
            within(
                window_refs,
                limits
                    .max_refs_per_node
                    .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
                AssignmentRuleLimit::PerRecord,
            )?;
            constraints = constraints.checked_add(3).ok_or_else(arithmetic)?;
            references = references
                .checked_add(width.checked_mul(2).ok_or_else(arithmetic)?)
                .and_then(|value| value.checked_add(window_refs))
                .and_then(|value| value.checked_add(6))
                .ok_or_else(arithmetic)?;
            edges = edges
                .checked_add(width)
                .and_then(|value| value.checked_add(window_refs.saturating_sub(1)))
                .and_then(|value| value.checked_add(2))
                .ok_or_else(arithmetic)?;
            shape.variables = shape
                .variables
                .checked_add(width.checked_add(2).ok_or_else(arithmetic)?)
                .ok_or_else(arithmetic)?;
            shape.constraints = shape
                .constraints
                .checked_add(constraints)
                .ok_or_else(arithmetic)?;
            shape.provenance = shape.provenance.checked_add(1).ok_or_else(arithmetic)?;
            shape.references = shape
                .references
                .checked_add(references)
                .ok_or_else(arithmetic)?;
            shape.component_edges = shape
                .component_edges
                .checked_add(edges)
                .ok_or_else(arithmetic)?;
            shape.term_count = shape.term_count.checked_add(1).ok_or_else(arithmetic)?;
            shape.windows.push(WindowShape {
                plan: plan_index,
                person: window.person,
                days,
            });
        }
    }
    Ok(shape)
}

pub(super) fn score_with_outcomes(
    input: &AssignmentInput,
    plan: &Plan,
    selected: &[AssignmentPair],
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
    mut outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<i64, AssignmentRuleError> {
    let mut windows = 0_i64;
    for window in &plan.windows {
        budget.step()?;
        let mut active = true;
        for day in &window.days {
            budget.step()?;
            let mut selected_day = false;
            for pair in &day.pairs {
                budget.steps(selection_search_steps(selected.len()))?;
                if selected.binary_search(pair).is_ok()
                    && super::preference::statically_feasible(input, *pair, settings, budget)?
                {
                    selected_day = true;
                    break;
                }
            }
            if !selected_day {
                active = false;
                break;
            }
        }
        if active {
            windows = windows.checked_add(1).ok_or_else(arithmetic)?;
        }
        if let Some(outcomes) = outcomes.as_deref_mut() {
            use super::alternative_outcomes::{Family, Key};
            outcomes.counts(
                Key::person(Family::Consecutive, plan.id, window.person),
                0,
                i128::from(active),
                i128::from(active),
                if active {
                    i128::from(plan.coefficient)
                } else {
                    0
                },
                budget,
            )?;
        }
    }
    windows.checked_mul(plan.coefficient).ok_or_else(arithmetic)
}

fn selection_search_steps(length: usize) -> u64 {
    let length = u64::try_from(length).unwrap_or(u64::MAX);
    u64::from(u64::BITS - length.saturating_add(1).leading_zeros())
}

fn entity(kind: &str, id: EntityId) -> Result<DomainEntityRef, AssignmentRuleError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| invalid())?,
        id: DomainEntityId::new(format!("official.workforce.{id}")).map_err(|_| invalid())?,
    })
}

fn derive_bool(
    identities: &mut super::identity::PlanningIdentities,
    key: &impl Serialize,
    budget: &mut OperationBudget<'_>,
) -> Result<BoolVariableId, AssignmentRuleError> {
    BoolVariableId::new(identities.derive(IdentityKind::Boolean, key, budget)?)
        .map_err(|_| invalid())
}

fn derive_constraint(
    identities: &mut super::identity::PlanningIdentities,
    key: &impl Serialize,
    budget: &mut OperationBudget<'_>,
) -> Result<PlanningConstraintId, AssignmentRuleError> {
    PlanningConstraintId::new(identities.derive(IdentityKind::Constraint, key, budget)?)
        .map_err(|_| invalid())
}

pub(super) fn ids(
    plan: &Plan,
    shape: &WindowShape,
    identities: &mut super::identity::PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<Ids, AssignmentRuleError> {
    let key = (
        "consecutive_work",
        plan.id,
        shape.person,
        shape.days[0].date,
        plan.threshold,
    );
    let fact = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &("consecutive_work_fact", key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let mut day_variables = Vec::with_capacity(shape.days.len());
    let mut day_constraints = Vec::with_capacity(shape.days.len());
    let mut window_constraints = Vec::with_capacity(shape.days.len());
    let mut member_constraints = Vec::new();
    for (index, day) in shape.days.iter().enumerate() {
        budget.step()?;
        day_variables.push(derive_bool(
            identities,
            &("consecutive_work_day", key, day.date),
            budget,
        )?);
        day_constraints.push(derive_constraint(
            identities,
            &("consecutive_work_day_or", key, day.date),
            budget,
        )?);
        for member in 0..day.population.len() {
            budget.step()?;
            member_constraints.push(derive_constraint(
                identities,
                &("consecutive_work_member", key, index, member),
                budget,
            )?);
        }
        window_constraints.push(derive_constraint(
            identities,
            &("consecutive_work_window_day", key, day.date),
            budget,
        )?);
    }
    let window_variable = derive_bool(identities, &("consecutive_work_window", key), budget)?;
    let window_integer = IntVariableId::new(identities.derive(
        IdentityKind::Integer,
        &("consecutive_work_window_value", key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    let window_or_constraint =
        derive_constraint(identities, &("consecutive_work_window_or", key), budget)?;
    let window_channels = [
        derive_constraint(
            identities,
            &("consecutive_work_window_channel", key, true),
            budget,
        )?,
        derive_constraint(
            identities,
            &("consecutive_work_window_channel", key, false),
            budget,
        )?,
    ];
    let term = ObjectiveTermId::new(identities.derive(
        IdentityKind::ObjectiveTerm,
        &("consecutive_work_term", key),
        budget,
    )?)
    .map_err(|_| invalid())?;
    Ok(Ids {
        fact,
        day_variables,
        window_variable,
        window_integer,
        member_constraints,
        day_constraints,
        window_constraints,
        window_or_constraint,
        window_channels,
        term,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn emit(
    plan: &Plan,
    shape: &WindowShape,
    ids: Ids,
    candidate_booleans: &[BoolVariableId],
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<Emission, AssignmentRuleError> {
    budget.steps(
        count(shape.days.len())?
            .checked_add(
                shape
                    .days
                    .iter()
                    .map(|day| day.population.len())
                    .try_fold(0_u64, |sum, len| {
                        sum.checked_add(count(len)?).ok_or_else(arithmetic)
                    })?,
            )
            .and_then(|value| value.checked_add(8))
            .ok_or_else(arithmetic)?,
    )?;
    let fact = fact(plan, shape, ids.fact.clone(), parent, budget)?;
    let mut variables = Vec::with_capacity(shape.days.len() + 2);
    for variable in &ids.day_variables {
        variables.push(Variable::Boolean(BoolVariable {
            id: variable.clone(),
            provenance: ids.fact.clone(),
        }));
    }
    variables.push(Variable::Boolean(BoolVariable {
        id: ids.window_variable.clone(),
        provenance: ids.fact.clone(),
    }));
    variables.push(Variable::Integer(IntVariable {
        id: ids.window_integer.clone(),
        domain: IntDomain {
            inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
        },
        provenance: ids.fact.clone(),
    }));
    let mut constraints = Vec::new();
    let mut member_offset = 0_usize;
    for (day_index, day) in shape.days.iter().enumerate() {
        let day_variable = ids.day_variables.get(day_index).ok_or_else(invalid)?;
        for candidate in &day.population {
            let candidate = candidate_booleans.get(*candidate).ok_or_else(invalid)?;
            let constraint_id = ids
                .member_constraints
                .get(member_offset)
                .ok_or_else(invalid)?
                .clone();
            member_offset = member_offset.checked_add(1).ok_or_else(arithmetic)?;
            constraints.push(ConstraintRecord {
                id: constraint_id,
                body: Constraint::bool_or(vec![
                    Literal::negative(candidate.clone()),
                    Literal::positive(day_variable.clone()),
                ]),
                enforcement: Vec::new(),
                provenance: ids.fact.clone(),
                tags: Vec::new(),
            });
        }
        let mut day_literals = Vec::with_capacity(day.population.len() + 1);
        day_literals.push(Literal::negative(day_variable.clone()));
        for candidate in &day.population {
            day_literals.push(Literal::positive(
                candidate_booleans
                    .get(*candidate)
                    .ok_or_else(invalid)?
                    .clone(),
            ));
        }
        constraints.push(ConstraintRecord {
            id: ids
                .day_constraints
                .get(day_index)
                .ok_or_else(invalid)?
                .clone(),
            body: Constraint::bool_or(day_literals),
            enforcement: Vec::new(),
            provenance: ids.fact.clone(),
            tags: Vec::new(),
        });
        let window_variable = ids.window_variable.clone();
        constraints.push(ConstraintRecord {
            id: ids
                .window_constraints
                .get(day_index)
                .ok_or_else(invalid)?
                .clone(),
            body: Constraint::bool_or(vec![
                Literal::negative(window_variable.clone()),
                Literal::positive(day_variable.clone()),
            ]),
            enforcement: Vec::new(),
            provenance: ids.fact.clone(),
            tags: Vec::new(),
        });
    }
    let mut window_literals = Vec::with_capacity(shape.days.len() + 1);
    window_literals.push(Literal::positive(ids.window_variable.clone()));
    window_literals.extend(ids.day_variables.iter().cloned().map(Literal::negative));
    constraints.push(ConstraintRecord {
        id: ids.window_or_constraint.clone(),
        body: Constraint::bool_or(window_literals),
        enforcement: Vec::new(),
        provenance: ids.fact.clone(),
        tags: Vec::new(),
    });
    for (id, selected) in ids.window_channels.into_iter().zip([true, false]) {
        constraints.push(ConstraintRecord {
            id,
            body: Constraint::LinearComparison(LinearComparison {
                expression: LinearExpression {
                    constant: 0,
                    terms: vec![LinearTerm {
                        variable: ids.window_integer.clone(),
                        coefficient: 1,
                    }],
                },
                op: ComparisonOp::Equal,
                rhs: i64::from(selected),
            }),
            enforcement: vec![Literal {
                variable: ids.window_variable.clone(),
                positive: selected,
            }],
            provenance: ids.fact.clone(),
            tags: Vec::new(),
        });
    }
    let term = ObjectiveTerm {
        id: ids.term,
        expression: LinearExpression {
            constant: 0,
            terms: vec![LinearTerm {
                variable: ids.window_integer,
                coefficient: plan.coefficient,
            }],
        },
        kind: ObjectiveTermKind::Penalty,
        category: eutheto_domain_ir::ScoreCategoryId::new(CATEGORY).map_err(|_| invalid())?,
        provenance: fact.id.clone(),
    };
    Ok(Emission {
        variables,
        constraints,
        term,
        fact,
    })
}

fn fact(
    plan: &Plan,
    shape: &WindowShape,
    id: ProvenanceId,
    parent: ProvenanceId,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    budget.reserve(0, 6, 640)?;
    let end_days = if plan.weekend { 2 } else { 1 };
    let end = shape
        .days
        .last()
        .ok_or_else(invalid)?
        .date
        .checked_add(Span::new().days(end_days))
        .map_err(|_| arithmetic())?;
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Preference,
        source_id: plan.id.to_string(),
        entity_refs: vec![entity(
            "person",
            EntityId::from_uuid(shape.person.as_uuid()),
        )?],
        message_key: "official.workforce.preference.consecutive_work".to_owned(),
        parameters: BTreeMap::from([
            (
                "coefficient".to_owned(),
                ProvenanceParameter::Integer(plan.coefficient),
            ),
            (
                "threshold".to_owned(),
                ProvenanceParameter::Integer(i64::from(plan.threshold)),
            ),
            (
                "window_start".to_owned(),
                ProvenanceParameter::Text(shape.days[0].date.to_string()),
            ),
            (
                "window_end".to_owned(),
                ProvenanceParameter::Text(end.to_string()),
            ),
        ]),
        parent: Some(parent),
    })
}
