use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, count},
    input::AssignmentInput,
    weekends,
};
use super::{InstantInterval, Selected, Summary, Witness, invalid, predicates};
use crate::model::{AssignmentPair, Scope, WorkforceRule};
use crate::temporal::ResolvedShift;
use eutheto_types::{EntityId, PersonId, RuleId, ScenarioSettings};
use jiff::civil::Date;
use std::collections::BTreeMap;

/// Evaluates `MaximumWorkedWeekends` from selected source-owned shifts. Candidate pruning and
/// compiler populations are deliberately not consulted here.
pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    settings: &ScenarioSettings,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumWorkedWeekends {
        id, scope, maximum, ..
    } = rule
    else {
        return Err(invalid());
    };
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? {
            continue;
        }
        let mut matching = Vec::new();
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if predicates::shift_matches(shift, &metadata, scope, budget)? {
                budget.reserve(0, 1, 32)?;
                matching.push(*shift);
            }
        }
        evaluate_person(
            input, selected, scope, *person_id, *id, *maximum, &matching, settings, summary, budget,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn evaluate_person(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    person_id: PersonId,
    rule_id: RuleId,
    maximum: u32,
    shifts: &[&ResolvedShift],
    settings: &ScenarioSettings,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut grouped = BTreeMap::<Date, (InstantInterval, &ResolvedShift)>::new();
    for shift in shifts {
        for weekend in weekends::for_shift(shift, settings, budget)? {
            budget.step()?;
            grouped
                .entry(weekend.date)
                .or_insert_with(|| (weekend.interval, *shift));
        }
    }
    let actual = count(grouped.len())?;
    if actual == 0 {
        return Ok(());
    }
    let index = usize::try_from(u64::from(maximum)).unwrap_or(usize::MAX);
    let (_, (interval, shift)) = grouped
        .iter()
        .nth(index.min(grouped.len().saturating_sub(1)))
        .ok_or_else(invalid)?;
    let mut witness = Witness::pair(
        AssignmentPair {
            person_id,
            shift_id: shift.id,
        },
        EntityId::from_uuid(rule_id.as_uuid()),
        "rule",
        "maximum_worked_weekends",
    );
    witness.lower = Some(0);
    witness.upper = Some(u64::from(maximum));
    witness.actual = Some(actual);
    witness.interval = Some(*interval);
    witness.period = Some((interval.start, interval.end));
    let failed = actual > u64::from(maximum);
    let mut members = Vec::new();
    if failed
        && summary.collecting_repair()
        && let Some(associations) = selected.associations()
    {
        for pair in associations {
            budget.step()?;
            if pair.person_id != person_id {
                continue;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            let metadata = input.metadata(shift)?;
            if predicates::shift_matches(shift, &metadata, scope, budget)?
                && !weekends::for_shift(shift, settings, budget)?.is_empty()
            {
                budget.reserve(0, 1, 48)?;
                members.push(*pair);
            }
        }
    }
    let collect_repair = summary.collecting_repair();
    summary.predicate_with_members(
        failed,
        &witness,
        collect_repair.then_some(members),
        "maximum_worked_weekends",
        budget,
    )
}
