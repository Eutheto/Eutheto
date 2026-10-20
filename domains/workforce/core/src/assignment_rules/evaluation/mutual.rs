use super::super::{
    AssignmentRuleError, InstantInterval,
    budget::{OperationBudget, add, count},
    input::AssignmentInput,
};
use super::{Selected, Summary, Witness, arithmetic, invalid, predicates};
use crate::{
    ids::ShiftId,
    model::{AssignmentPair, PairingMode, Scope, WorkforceRule},
    temporal::ResolvedShift,
};
use eutheto_types::{EntityId, PersonId, RuleId};

/// Evaluates the authored mutual restriction over the original selected assignments.
///
/// The population is deliberately rebuilt from the selected decision pairs rather than from
/// compiler candidates. A person must be in both `person_ids` and the authored person scope, and
/// every shift in a checked pair must match the authored shift scope.
pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
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
    match mode {
        PairingMode::SameShift => {
            same_shift(input, selected, scope, person_ids, *id, summary, budget)
        }
        PairingMode::OverlappingShifts => {
            overlapping_shifts(input, selected, scope, person_ids, *id, summary, budget)
        }
    }
}

fn same_shift(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    person_ids: &[PersonId],
    rule_id: RuleId,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for (shift_id, people) in &selected.by_shift {
        budget.step()?;
        let shift = input.shift(*shift_id).ok_or_else(invalid)?;
        let metadata = input.metadata(shift)?;
        if !predicates::shift_matches(shift, &metadata, scope, budget)? {
            continue;
        }
        let mut matching = Vec::new();
        for person_id in people {
            if !listed_in_scope(input, *person_id, person_ids, scope, budget)? {
                continue;
            }
            budget.reserve(0, 1, 16)?;
            matching.push(*person_id);
        }
        for (index, first) in matching.iter().enumerate() {
            for second in &matching[index + 1..] {
                let witness = mutual_witness(
                    *first,
                    *shift_id,
                    *second,
                    *shift_id,
                    rule_id,
                    predicates::interval(shift),
                );
                summary.predicate(true, &witness, budget)?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Matching<'a> {
    person: PersonId,
    shift: &'a ResolvedShift,
    interval: InstantInterval,
}

fn overlapping_shifts(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    person_ids: &[PersonId],
    rule_id: RuleId,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut matching = Vec::new();
    let mut same_person_pairs = 0_u64;
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        if !listed_in_scope(input, *person_id, person_ids, scope, budget)? {
            continue;
        }
        let mut person_count = 0_usize;
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !predicates::shift_matches(shift, &metadata, scope, budget)? {
                continue;
            }
            budget.reserve(0, 1, 64)?;
            matching.push(Matching {
                person: *person_id,
                shift,
                interval: predicates::interval(shift),
            });
            person_count = person_count.checked_add(1).ok_or_else(arithmetic)?;
        }
        same_person_pairs = add(same_person_pairs, pair_count(person_count)?)?;
    }
    budget.sort_work(matching.len())?;
    matching.sort_unstable_by_key(|entry| {
        (
            entry.interval.start,
            entry.interval.end,
            entry.person,
            entry.shift.id,
        )
    });

    // Every unordered pair of matching assignments belonging to distinct people is a checked
    // predicate. The overlap sweep below only materializes actual violations, preserving that
    // count without spending quadratic work on known non-overlapping suffixes.
    let checked = pair_count(matching.len())?
        .checked_sub(same_person_pairs)
        .ok_or_else(arithmetic)?;
    summary.checked = add(summary.checked, checked)?;

    for (index, first) in matching.iter().enumerate() {
        for second in &matching[index + 1..] {
            budget.step()?;
            if second.interval.start >= first.interval.end {
                break;
            }
            if first.person == second.person {
                continue;
            }
            let Some(overlap) = first.interval.intersection(second.interval) else {
                continue;
            };
            let witness = mutual_witness(
                first.person,
                first.shift.id,
                second.person,
                second.shift.id,
                rule_id,
                overlap,
            );
            summary.failure(&witness, &witness, None, "direct", budget)?;
        }
    }
    Ok(())
}

fn mutual_witness(
    first_person: PersonId,
    first_shift: ShiftId,
    second_person: PersonId,
    second_shift: ShiftId,
    rule_id: RuleId,
    interval: InstantInterval,
) -> Witness {
    let mut witness = Witness::pair(
        AssignmentPair {
            person_id: first_person,
            shift_id: first_shift,
        },
        EntityId::from_uuid(rule_id.as_uuid()),
        "rule",
        "mutual_assignment_restriction",
    );
    witness.other_person = Some(second_person);
    witness.other_shift = Some(second_shift);
    witness.interval = Some(interval);
    witness
}

fn listed_in_scope(
    input: &AssignmentInput,
    person_id: PersonId,
    person_ids: &[PersonId],
    scope: &Scope,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.steps(count(person_ids.len())?)?;
    if !person_ids.contains(&person_id) {
        return Ok(false);
    }
    let person = input.person(person_id).ok_or_else(invalid)?;
    predicates::person_matches(person, scope, budget)
}

fn pair_count(length: usize) -> Result<u64, AssignmentRuleError> {
    let length = count(length)?;
    length
        .checked_mul(length.saturating_sub(1))
        .ok_or_else(arithmetic)
        .map(|value| value / 2)
}
