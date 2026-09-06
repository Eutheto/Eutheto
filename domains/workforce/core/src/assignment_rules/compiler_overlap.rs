use super::{
    AssignmentInput, AssignmentPair, AssignmentRuleError, OperationBudget, PersonId, Plan,
    PlannedConstraint, Predicate, RuleId, add, append, count, invalid,
};
use jiff::Timestamp;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct Event {
    instant: Timestamp,
    // false sorts before true: half-open intervals end before simultaneous starts.
    start: bool,
    candidate: usize,
}

pub(super) fn plan_cliques(
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    scoped: &[usize],
    plan: &mut Plan,
    rule: RuleId,
    person: PersonId,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    if scoped.len() < 2 {
        return budget.check();
    }
    let members = count(scoped.len())?;
    let event_count = members.checked_mul(2).ok_or_else(invalid)?;
    // Logical bounds for 2N endpoint records and N active tree nodes. The scoped
    // candidate index was separately reserved before each insertion. All scratch
    // reservations remain cumulative even after this person's sweep is dropped.
    budget.reserve(
        0,
        add(event_count, members)?,
        members.checked_mul(128).ok_or_else(invalid)?,
    )?;
    let mut events = Vec::with_capacity(scoped.len().checked_mul(2).ok_or_else(invalid)?);
    for &candidate in scoped {
        budget.step()?;
        let shift = input
            .shift(candidates[candidate].shift_id)
            .ok_or_else(invalid)?;
        for (instant, start) in [
            (shift.interval.starts_at.instant.as_timestamp(), true),
            (shift.interval.ends_at.instant.as_timestamp(), false),
        ] {
            budget.step()?;
            events.push(Event {
                instant,
                start,
                candidate,
            });
        }
    }
    sweep(&mut events, budget, |active, budget| {
        let length = count(active.len())?;
        // Reserve output membership before collect, then charge every copied member.
        budget.reserve(0, length, length.checked_mul(8).ok_or_else(invalid)?)?;
        budget.steps(length)?;
        let population: Vec<_> = active.iter().copied().collect();
        let predicate = if population.len() == 2 {
            let first = candidates[population[0]].shift_id;
            let second = candidates[population[1]].shift_id;
            Predicate::Overlap {
                person,
                first: first.min(second),
                second: first.max(second),
            }
        } else {
            Predicate::OverlapClique { person }
        };
        append(
            plan,
            PlannedConstraint {
                rule,
                predicate,
                population,
                impossible: false,
            },
            budget,
        )
    })
}

fn sweep(
    events: &mut [Event],
    budget: &mut OperationBudget<'_>,
    mut emit: impl FnMut(&BTreeSet<usize>, &mut OperationBudget<'_>) -> Result<(), AssignmentRuleError>,
) -> Result<(), AssignmentRuleError> {
    budget.sort_work(events.len())?;
    events.sort_unstable();
    budget.check()?;
    let mut active = BTreeSet::new();
    let mut new_starts = false;
    let mut position = 0;
    // BTree node searches inspect at most a bounded fanout per tree level.
    let active_work = u64::from(usize::BITS - events.len().leading_zeros())
        .checked_mul(12)
        .ok_or_else(invalid)?;
    while position < events.len() {
        budget.step()?;
        let instant = events[position].instant;
        let mut end = position;
        while end < events.len() && events[end].instant == instant {
            budget.step()?;
            end += 1;
        }
        if !events[position].start {
            // All starts at this instant are still absent. Subsequent ends without
            // new starts only shrink this clique, so never emit those subsets.
            if new_starts && active.len() >= 2 {
                emit(&active, budget)?;
            }
            new_starts = false;
        }
        for event in &events[position..end] {
            budget.steps(active_work)?;
            if event.start {
                active.insert(event.candidate);
                new_starts = true;
            } else {
                active.remove(&event.candidate);
            }
        }
        position = end;
    }
    budget.check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assignment_rules::AssignmentRuleLimit,
        test_support::{fixture, id},
    };
    use eutheto_planning_ir::PlanningIrLimitsV1;
    use eutheto_types::CancellationToken;
    use serde_json::json;

    #[test]
    fn cancellation_inside_endpoint_groups_and_member_emission_is_observed()
    -> Result<(), Box<dyn std::error::Error>> {
        let start: Timestamp = "2026-11-01T00:00:00Z".parse()?;
        let end: Timestamp = "2026-11-01T01:00:00Z".parse()?;
        for during_emission in [false, true] {
            let token = CancellationToken::new();
            let mut budget = OperationBudget::analysis(Some(&token), PlanningIrLimitsV1::DEFAULT);
            let mut events: Vec<_> = (0..128)
                .flat_map(|candidate| {
                    [
                        Event {
                            instant: start,
                            start: true,
                            candidate,
                        },
                        Event {
                            instant: end,
                            start: false,
                            candidate,
                        },
                    ]
                })
                .collect();
            if !during_emission {
                // Sorting costs 256 * 9. Cancellation comes later, while the
                // simultaneous-start group is being inserted into the active set.
                budget.cancel_after_steps(256 * 9 + 256 + 100)?;
            }
            let result = sweep(&mut events, &mut budget, |active, budget| {
                if during_emission {
                    budget.cancel_after_steps(64)?;
                }
                budget.steps(count(active.len())?)
            });
            assert_eq!(result, Err(AssignmentRuleError::Cancelled));
            assert!(token.is_cancelled());
        }
        Ok(())
    }

    #[test]
    fn endpoint_and_active_reservation_cannot_reset_consumed_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let prototype = document
            .domain
            .entities
            .get(&id(8).parse()?)
            .ok_or("shift")?
            .clone();
        for index in 1_000..1_031 {
            let mut shift = prototype.clone();
            shift["id"] = json!(id(index));
            document.domain.entities.insert(id(index).parse()?, shift);
        }
        for field in 0..2 {
            let mut budget = OperationBudget::analysis(None, PlanningIrLimitsV1::DEFAULT);
            let input = AssignmentInput::new(&document, &mut budget)?;
            let person = id(1).parse()?;
            let mut candidates: Vec<_> = input
                .shifts
                .iter()
                .map(|shift| AssignmentPair {
                    person_id: person,
                    shift_id: shift.id,
                })
                .collect();
            candidates.sort_unstable();
            let scoped: Vec<_> = (0..candidates.len()).collect();
            let mut plan = Plan {
                definitions: Vec::new(),
                constraints: Vec::new(),
                parents: std::collections::BTreeMap::default(),
            };
            let (_, remaining_items, remaining_bytes) = budget.remaining_output();
            let members = count(candidates.len())?;
            if field == 0 {
                budget.reserve(0, remaining_items - members * 3 + 1, 0)?;
            } else {
                budget.reserve(0, 0, remaining_bytes - members * 128 + 1)?;
            }
            assert!(matches!(
                plan_cliques(
                    &input,
                    &candidates,
                    &scoped,
                    &mut plan,
                    id(23).parse()?,
                    person,
                    &mut budget
                ),
                Err(AssignmentRuleError::LimitExceeded(
                    AssignmentRuleLimit::References | AssignmentRuleLimit::Bytes
                ))
            ));
            assert!(plan.constraints.is_empty());
        }
        Ok(())
    }
}
