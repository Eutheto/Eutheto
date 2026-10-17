use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add},
    input::AssignmentInput,
    reporting::{ReportingPeriod, ReportingWindows},
};
use super::{Selected, Summary, Witness, invalid, predicates};
use crate::model::{AssignmentPair, WorkforceRule};
use eutheto_types::{PersonId, ScenarioDocument};
use std::collections::BTreeMap;

/// Evaluate `MaximumAssignmentCount` directly from selected source pairs. Candidate pruning,
/// compiler populations and IR constraints are deliberately not consulted here.
pub(super) fn evaluate(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumAssignmentCount {
        scope,
        calendar_id,
        maximum,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    let mut periods = ReportingWindows::new();
    let mut grouped = BTreeMap::<(PersonId, ReportingPeriod), (u64, crate::ids::ShiftId)>::new();
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? {
            continue;
        }
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !predicates::shift_matches(shift, &metadata, scope, budget)? {
                continue;
            }
            let Some(period) =
                periods.resolve(document, input, *calendar_id, shift.reporting_date, budget)?
            else {
                // Custom reporting dates with no owning window are intentionally vacuous.
                continue;
            };
            let key = (*person_id, period);
            if let Some((count, first_shift)) = grouped.get_mut(&key) {
                *count = add(*count, 1)?;
                *first_shift = (*first_shift).min(shift.id);
            } else {
                budget.reserve(1, 2, 32)?;
                grouped.insert(key, (1, shift.id));
            }
        }
    }
    for ((person_id, period), (actual, first_shift)) in grouped {
        budget.step()?;
        let mut witness = Witness::pair(
            AssignmentPair {
                person_id,
                shift_id: first_shift,
            },
            period.calendar_id.as_entity_id(),
            "calendar",
            "maximum_assignment_count",
        );
        witness.lower = Some(0);
        witness.upper = Some(u64::from(*maximum));
        witness.actual = Some(actual);
        witness.interval = Some(super::super::InstantInterval {
            start: period.start,
            end: period.end,
        });
        witness.period = Some((period.start, period.end));
        let failed = actual > u64::from(*maximum);
        let members = if failed && summary.collecting_repair() {
            association_members(
                document,
                input,
                selected,
                person_id,
                scope,
                *calendar_id,
                period,
                &mut periods,
                budget,
            )?
        } else {
            Vec::new()
        };
        let collect_repair = summary.collecting_repair();
        summary.predicate_with_members(
            failed,
            &witness,
            collect_repair.then_some(members),
            "maximum_assignment_count",
            budget,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn association_members(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    person_id: PersonId,
    scope: &crate::model::Scope,
    calendar_id: crate::ids::WorkCalendarId,
    period: ReportingPeriod,
    periods: &mut ReportingWindows,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentPair>, AssignmentRuleError> {
    let mut members = Vec::new();
    if let Some(associations) = selected.associations() {
        for pair in associations {
            budget.step()?;
            if pair.person_id != person_id {
                continue;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            let metadata = input.metadata(shift)?;
            if predicates::shift_matches(shift, &metadata, scope, budget)?
                && periods.resolve(document, input, calendar_id, shift.reporting_date, budget)?
                    == Some(period)
            {
                budget.reserve(0, 1, 48)?;
                members.push(*pair);
            }
        }
    }
    Ok(members)
}
