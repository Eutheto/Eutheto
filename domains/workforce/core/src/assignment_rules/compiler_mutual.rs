use super::{
    AssignmentInput, AssignmentLock, AssignmentPair, AssignmentRuleError, AssignmentRuleLimit,
    OperationBudget, PairingMode, Plan, Predicate, RuleId, Scope, ValidationIssue,
    ValidationSeverity, WorkforceRule, commit_append, count, interval, invalid, person_scope,
    reserve_append, shift_scope, within,
};
use crate::ids::ShiftId;
use eutheto_domain_api::DomainValidationReport;
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{PersonId, ResourceRef};
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct Event {
    instant: Timestamp,
    // Endpoints sort before starts at the same instant, preserving half-open intervals.
    start: bool,
    candidate: usize,
}

struct MutualRule<'a> {
    id: RuleId,
    scope: &'a Scope,
    person_ids: &'a [PersonId],
    mode: PairingMode,
    maximum_population: u64,
}

pub(super) fn plan(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    by_shift: &BTreeMap<ShiftId, Vec<usize>>,
    plan: &mut Plan,
    rule: &WorkforceRule,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MutualAssignmentRestriction {
        id,
        scope,
        person_ids,
        mode,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    let rule = MutualRule {
        id: *id,
        scope,
        person_ids,
        mode: *mode,
        maximum_population: limits
            .max_refs_per_node
            .min(PlanningIrLimitsV1::DEFAULT.max_refs_per_node),
    };
    let listed = scoped_people(input, rule.scope, rule.person_ids, budget)?;
    if listed.len() < 2 {
        // An active singleton/empty effective rule is a valid compatibility no-op.
        return budget.check();
    }
    match rule.mode {
        PairingMode::SameShift => {
            plan_same_shift(input, candidates, by_shift, plan, &rule, &listed, budget)
        }
        PairingMode::OverlappingShifts => {
            plan_overlapping(input, candidates, plan, &rule, &listed, budget)
        }
    }
}

fn scoped_people(
    input: &AssignmentInput,
    scope: &Scope,
    person_ids: &[PersonId],
    budget: &mut OperationBudget<'_>,
) -> Result<BTreeSet<PersonId>, AssignmentRuleError> {
    let mut listed = BTreeSet::new();
    for person_id in person_ids {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if person_scope(scope, person, budget)? {
            budget.reserve(0, 1, 16)?;
            listed.insert(*person_id);
        }
    }
    Ok(listed)
}

fn plan_same_shift(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    by_shift: &BTreeMap<ShiftId, Vec<usize>>,
    plan: &mut Plan,
    rule: &MutualRule<'_>,
    listed: &BTreeSet<PersonId>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for shift in &input.shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if !shift_scope(rule.scope, shift, &metadata, budget)? {
            continue;
        }
        let Some(indexes) = by_shift.get(&shift.id) else {
            continue;
        };
        let mut population_len = 0usize;
        for index in indexes {
            budget.step()?;
            if listed.contains(&candidates[*index].person_id) {
                population_len = population_len.checked_add(1).ok_or_else(invalid)?;
            }
        }
        if population_len < 2 {
            continue;
        }
        within(
            count(population_len)?,
            rule.maximum_population,
            AssignmentRuleLimit::PerRecord,
        )?;
        let predicate = Predicate::MutualSameShift { shift: shift.id };
        // Reserve the constraint, population membership, and scratch before allocating either
        // the output population or any additional graph node. Dense conflicts fail here.
        let reservation = reserve_append(plan, rule.id, predicate, budget)?;
        let population_items = count(population_len)?;
        budget.reserve(
            0,
            population_items,
            population_items.checked_mul(8).ok_or_else(invalid)?,
        )?;
        let mut population = Vec::with_capacity(population_len);
        for index in indexes {
            budget.step()?;
            if listed.contains(&candidates[*index].person_id) {
                population.push(*index);
            }
        }
        commit_append(
            plan,
            super::PlannedConstraint {
                rule: rule.id,
                predicate,
                population,
                impossible: false,
            },
            reservation,
        );
    }
    budget.check()
}

fn plan_overlapping(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    plan: &mut Plan,
    rule: &MutualRule<'_>,
    listed: &BTreeSet<PersonId>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let events = matching_events(input, candidates, rule.scope, listed, budget)?;
    if events.is_empty() {
        return budget.check();
    }
    let mut active = BTreeSet::<usize>::new();
    let active_work = u64::from(usize::BITS - events.len().leading_zeros())
        .checked_mul(12)
        .ok_or_else(invalid)?;
    let mut position = 0usize;
    while position < events.len() {
        budget.step()?;
        let instant = events[position].instant;
        let mut end = position;
        while end < events.len() && events[end].instant == instant {
            budget.step()?;
            end += 1;
        }
        // All ends at this instant are removed before starts, so touching intervals do not
        // conflict. Starts are then inserted one at a time, making same-shift conflicts visible.
        for event in &events[position..end] {
            if !event.start {
                budget.steps(active_work)?;
                active.remove(&event.candidate);
            }
        }
        for event in &events[position..end] {
            if !event.start {
                continue;
            }
            for other in &active {
                budget.steps(active_work)?;
                let first = candidates[*other];
                let second = candidates[event.candidate];
                if first.person_id == second.person_id {
                    continue;
                }
                let (first_index, second_index, first_pair, second_pair) = if first < second {
                    (*other, event.candidate, first, second)
                } else {
                    (event.candidate, *other, second, first)
                };
                let predicate = Predicate::MutualOverlap {
                    first_person: first_pair.person_id,
                    first_shift: first_pair.shift_id,
                    second_person: second_pair.person_id,
                    second_shift: second_pair.shift_id,
                };
                // Check the bounded constraint limit before allocating the two-member output
                // population. Every pair is emitted independently to preserve same-person work.
                let reservation = reserve_append(plan, rule.id, predicate, budget)?;
                budget.reserve(0, 2, 16)?;
                let population = vec![first_index, second_index];
                commit_append(
                    plan,
                    super::PlannedConstraint {
                        rule: rule.id,
                        predicate,
                        population,
                        impossible: false,
                    },
                    reservation,
                );
            }
            budget.steps(active_work)?;
            active.insert(event.candidate);
        }
        position = end;
    }
    budget.check()
}

fn matching_events(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    scope: &Scope,
    listed: &BTreeSet<PersonId>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Event>, AssignmentRuleError> {
    // Resolve each shift scope once, then count matching candidate pairs before allocating the
    // event scratch. The second pass fills the already-reserved event buffer.
    let mut matching_shifts = BTreeSet::new();
    for shift in &input.shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if shift_scope(scope, shift, &metadata, budget)? {
            budget.reserve(0, 1, 8)?;
            matching_shifts.insert(shift.id);
        }
    }
    let mut matching_candidates = 0usize;
    for pair in candidates {
        budget.step()?;
        if listed.contains(&pair.person_id) && matching_shifts.contains(&pair.shift_id) {
            matching_candidates = matching_candidates.checked_add(1).ok_or_else(invalid)?;
        }
    }
    if matching_candidates < 2 {
        return Ok(Vec::new());
    }
    let event_count = matching_candidates.checked_mul(2).ok_or_else(invalid)?;
    let scratch_items = count(event_count)?
        .checked_add(count(matching_candidates)?)
        .ok_or_else(invalid)?;
    let candidate_bytes = count(matching_candidates)?
        .checked_mul(8)
        .ok_or_else(invalid)?;
    let scratch_bytes = count(event_count)?
        .checked_mul(128)
        .and_then(|value| value.checked_add(candidate_bytes))
        .ok_or_else(invalid)?;
    // This is cumulative logical scratch; no per-conflict matrix is retained.
    budget.reserve(0, scratch_items, scratch_bytes)?;
    let mut events = Vec::with_capacity(event_count);
    for (candidate, pair) in candidates.iter().enumerate() {
        budget.step()?;
        if !listed.contains(&pair.person_id) || !matching_shifts.contains(&pair.shift_id) {
            continue;
        }
        let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
        events.push(Event {
            instant: shift.interval.starts_at.instant.as_timestamp(),
            start: true,
            candidate,
        });
        events.push(Event {
            instant: shift.interval.ends_at.instant.as_timestamp(),
            start: false,
            candidate,
        });
    }
    budget.sort_work(events.len())?;
    events.sort_unstable();
    budget.check()?;
    Ok(events)
}

pub(super) fn locked_findings(
    input: &AssignmentInput,
    locks: &[&AssignmentLock],
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for rule in input.domain.rules.values() {
        budget.step()?;
        let WorkforceRule::MutualAssignmentRestriction {
            id,
            active: true,
            scope,
            person_ids,
            mode,
            ..
        } = rule
        else {
            continue;
        };
        let listed = scoped_people(input, scope, person_ids, budget)?;
        if listed.len() < 2 {
            continue;
        }
        let mut scoped = Vec::new();
        for lock in locks {
            budget.step()?;
            if !listed.contains(&lock.person_id) {
                continue;
            }
            let person = input.person(lock.person_id).ok_or_else(invalid)?;
            let Some(shift) = input.shift(lock.shift_id) else {
                continue;
            };
            let metadata = input.metadata(shift)?;
            if !person_scope(scope, person, budget)?
                || !shift_scope(scope, shift, &metadata, budget)?
            {
                continue;
            }
            budget.reserve(0, 1, 16)?;
            scoped.push(*lock);
        }
        for (position, first) in scoped.iter().enumerate() {
            budget.step()?;
            let Some(first_shift) = input.shift(first.shift_id) else {
                continue;
            };
            for second in &scoped[position + 1..] {
                budget.step()?;
                if first.person_id == second.person_id {
                    continue;
                }
                let same_shift = first.shift_id == second.shift_id;
                let overlaps = same_shift
                    || input
                        .shift(second.shift_id)
                        .and_then(|shift| interval(first_shift).intersection(interval(shift)))
                        .is_some();
                if !overlaps {
                    continue;
                }
                if *mode == PairingMode::SameShift && !same_shift {
                    continue;
                }
                budget.reserve(1, 7, 1024)?;
                let (first_lock, second_lock) = if first.id <= second.id {
                    (first, second)
                } else {
                    (second, first)
                };
                let mode_name = match mode {
                    PairingMode::SameShift => "SameShift",
                    PairingMode::OverlappingShifts => "OverlappingShifts",
                };
                report.issues.push(ValidationIssue {
                    code: "official.workforce.hard_lock_mutual_conflict".to_owned(),
                    severity: ValidationSeverity::Error,
                    message: format!(
                        "Hard locks {} and {} conflict under active MutualAssignmentRestriction rule {} in {mode_name} mode: distinct listed people are locked on {} and {}. This is not a solver feasibility result.",
                        first_lock.id,
                        second_lock.id,
                        id,
                        first_lock.shift_id,
                        second_lock.shift_id,
                    ),
                    field_path: Some(format!(
                        "domain.lockedAssignments.{}",
                        first_lock.id
                    )),
                    resource: Some(ResourceRef::Assignment(first_lock.id)),
                });
            }
        }
    }
    budget.check()
}
