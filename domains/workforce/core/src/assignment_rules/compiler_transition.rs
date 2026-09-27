use super::{
    AssignmentInput, AssignmentLock, AssignmentPair, AssignmentRuleError, DomainValidationReport,
    LockState, OperationBudget, Plan, PlannedConstraint, Predicate, ResourceRef, RuleId,
    ValidationIssue, ValidationSeverity, WorkforceEntity, WorkforceRule, commit_append, count,
    invalid, person_scope, reserve_append, shift_scope,
};
use crate::{ids::LocationId, temporal::ResolvedShift};
use eutheto_types::PersonId;
use jiff::SignedDuration;

#[derive(Clone, Copy)]
struct Member {
    index: usize,
    shift: ResolvedShift,
    location: LocationId,
}

#[derive(Clone, Copy)]
struct LockedMember<'a> {
    lock: &'a AssignmentLock,
    member: Member,
}

pub(super) fn plan(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    plan: &mut Plan,
    rule: &WorkforceRule,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::TransitionTime {
        id,
        scope,
        location_ids,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    if location_ids.len() < 2 {
        return budget.check();
    }
    let mut position = 0;
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        let person_matches = person_scope(scope, person, budget)?;
        let mut members = Vec::new();
        while let Some(pair) = candidates.get(position) {
            budget.step()?;
            if pair.person_id != *person_id {
                break;
            }
            if person_matches && let Some(shift) = input.shift(pair.shift_id) {
                let metadata = input.metadata(shift)?;
                if shift_scope(scope, shift, &metadata, budget)?
                    && let Some(location) = metadata.location_id
                {
                    budget.steps(count(location_ids.len())?)?;
                    if location_ids.contains(&location) {
                        budget.reserve(0, 2, 32)?;
                        members.push(Member {
                            index: position,
                            shift: *shift,
                            location,
                        });
                    }
                }
            }
            position += 1;
        }
        if members.len() < 2 {
            continue;
        }
        budget.sort_work(members.len())?;
        members.sort_unstable_by_key(|member| {
            (
                member.shift.interval.starts_at.instant,
                member.shift.id,
                member.index,
            )
        });
        plan_members(input, &members, *id, *person_id, plan, budget)?;
    }
    budget.check()
}

fn plan_members(
    input: &AssignmentInput,
    members: &[Member],
    rule: RuleId,
    person: PersonId,
    plan: &mut Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for (position, first) in members.iter().enumerate() {
        budget.step()?;
        // A source location can require different durations for each target, so an
        // adequate gap for one target cannot terminate this scan.
        for second in &members[position + 1..] {
            budget.step()?;
            if first.shift.id == second.shift.id || first.location == second.location {
                continue;
            }
            let first_start = first.shift.interval.starts_at.instant;
            let second_start = second.shift.interval.starts_at.instant;
            match first_start.cmp(&second_start) {
                std::cmp::Ordering::Less => {
                    plan_direction(input, *first, *second, rule, person, plan, budget)?;
                }
                std::cmp::Ordering::Greater => {
                    plan_direction(input, *second, *first, rule, person, plan, budget)?;
                }
                std::cmp::Ordering::Equal => {
                    plan_direction(input, *first, *second, rule, person, plan, budget)?;
                    plan_direction(input, *second, *first, rule, person, plan, budget)?;
                }
            }
        }
    }
    Ok(())
}

fn plan_direction(
    input: &AssignmentInput,
    source: Member,
    target: Member,
    rule: RuleId,
    person: PersonId,
    plan: &mut Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let Some(required_minutes) = under_gap(input, source, target, budget)? else {
        return Ok(());
    };
    let predicate = Predicate::TransitionTime {
        person,
        source_location: source.location,
        target_location: target.location,
        source: source.shift.id,
        target: target.shift.id,
        required_minutes,
    };
    // Reserve the constraint and its two-member output before allocating the population. Dense
    // transition matrices therefore fail at the bounded limit without retaining a conflict row.
    let reservation = reserve_append(plan, rule, predicate, budget)?;
    budget.reserve(0, 2, 16)?;
    commit_append(
        plan,
        PlannedConstraint {
            rule,
            predicate,
            population: vec![source.index, target.index],
            impossible: false,
        },
        reservation,
    );
    Ok(())
}

fn under_gap(
    input: &AssignmentInput,
    source: Member,
    target: Member,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<u32>, AssignmentRuleError> {
    let Some(WorkforceEntity::Location(location)) =
        input.domain.entities.get(&source.location.as_entity_id())
    else {
        return Err(invalid());
    };
    for transition in &location.transitions {
        budget.step()?;
        if transition.location_id != target.location {
            continue;
        }
        let elapsed = source
            .shift
            .interval
            .ends_at
            .instant
            .as_timestamp()
            .duration_until(target.shift.interval.starts_at.instant.as_timestamp());
        let required = SignedDuration::from_secs(i64::from(transition.minutes) * 60);
        return Ok((elapsed < required).then_some(transition.minutes));
    }
    Ok(None)
}

pub(super) fn locked_findings(
    input: &AssignmentInput,
    locks: &[&AssignmentLock],
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for rule in input.domain.rules.values() {
        budget.step()?;
        let WorkforceRule::TransitionTime {
            id,
            active: true,
            scope,
            location_ids,
            ..
        } = rule
        else {
            continue;
        };
        if location_ids.len() < 2 {
            continue;
        }
        let mut lock_position = 0;
        for person_id in &input.people {
            budget.step()?;
            let person = input.person(*person_id).ok_or_else(invalid)?;
            let person_matches = person_scope(scope, person, budget)?;
            let mut members = Vec::new();
            while let Some(lock) = locks.get(lock_position) {
                budget.step()?;
                if lock.person_id < *person_id {
                    lock_position += 1;
                    continue;
                }
                if lock.person_id > *person_id {
                    break;
                }
                lock_position += 1;
                if !person_matches || !matches!(lock.state, LockState::Hard {}) {
                    continue;
                }
                let Some(shift) = input.shift(lock.shift_id) else {
                    continue;
                };
                let metadata = input.metadata(shift)?;
                let Some(location) = metadata.location_id else {
                    continue;
                };
                if !shift_scope(scope, shift, &metadata, budget)? {
                    continue;
                }
                budget.steps(count(location_ids.len())?)?;
                if !location_ids.contains(&location) {
                    continue;
                }
                budget.reserve(0, 3, 64)?;
                members.push(LockedMember {
                    lock,
                    member: Member {
                        index: 0,
                        shift: *shift,
                        location,
                    },
                });
            }
            if members.len() < 2 {
                continue;
            }
            budget.sort_work(members.len())?;
            members.sort_unstable_by_key(|member| {
                (
                    member.member.shift.interval.starts_at.instant,
                    member.member.shift.id,
                    member.lock.id,
                )
            });
            locked_members(input, &members, *id, report, budget)?;
        }
    }
    budget.check()
}

fn locked_members(
    input: &AssignmentInput,
    members: &[LockedMember<'_>],
    rule: RuleId,
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for (position, first) in members.iter().enumerate() {
        budget.step()?;
        for second in &members[position + 1..] {
            budget.step()?;
            if first.member.shift.id == second.member.shift.id
                || first.member.location == second.member.location
            {
                continue;
            }
            let first_start = first.member.shift.interval.starts_at.instant;
            let second_start = second.member.shift.interval.starts_at.instant;
            match first_start.cmp(&second_start) {
                std::cmp::Ordering::Less => {
                    locked_direction(input, *first, *second, rule, report, budget)?;
                }
                std::cmp::Ordering::Greater => {
                    locked_direction(input, *second, *first, rule, report, budget)?;
                }
                std::cmp::Ordering::Equal => {
                    locked_direction(input, *first, *second, rule, report, budget)?;
                    locked_direction(input, *second, *first, rule, report, budget)?;
                }
            }
        }
    }
    Ok(())
}

fn locked_direction(
    input: &AssignmentInput,
    source: LockedMember<'_>,
    target: LockedMember<'_>,
    rule: RuleId,
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let Some(required_minutes) = under_gap(input, source.member, target.member, budget)? else {
        return Ok(());
    };
    budget.step()?;
    budget.reserve(1, 7, 1024)?;
    let elapsed = source
        .member
        .shift
        .interval
        .ends_at
        .instant
        .as_timestamp()
        .duration_until(
            target
                .member
                .shift
                .interval
                .starts_at
                .instant
                .as_timestamp(),
        );
    report.issues.push(ValidationIssue {
        code: "official.workforce.hard_lock_transition_conflict".to_owned(),
        severity: ValidationSeverity::Error,
        message: format!(
            "Source Hard lock {} and target Hard lock {} conflict under active TransitionTime rule {}: travel from location {} to {} requires {} minutes, while elapsed time is {} seconds and {} subsecond nanoseconds. This is not a solver feasibility result.",
            source.lock.id,
            target.lock.id,
            rule,
            source.member.location,
            target.member.location,
            required_minutes,
            elapsed.as_secs(),
            elapsed.subsec_nanos(),
        ),
        field_path: Some(format!("domain.lockedAssignments.{}", source.lock.id)),
        resource: Some(ResourceRef::Assignment(source.lock.id)),
    });
    Ok(())
}
