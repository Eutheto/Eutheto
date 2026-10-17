use std::collections::BTreeMap;

use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add, count},
    input::AssignmentInput,
};
use super::{
    HealthDirection, HealthObservation, HealthObservationKind, HealthUnits, Selected, Summary,
    TransitionWitness, Witness, invalid, predicates,
};
use crate::{
    ids::LocationId,
    model::{AssignmentPair, Scope, WorkforceEntity, WorkforceRule},
    temporal::ResolvedShift,
};
use eutheto_types::{EntityId, Rfc3339Timestamp};
use jiff::SignedDuration;

struct Matching<'a> {
    shift: &'a ResolvedShift,
    location: LocationId,
}

/// Evaluates authored directed travel requirements over the original selected assignments.
///
/// Only pairs belonging to the same person, matching the complete authored scope, and using
/// distinct named locations are predicates. A directed edge is required for a pair to be checked;
/// it is never inferred from the reverse edge. Equal-start pairs are reached from both source
/// assignments, so each configured direction is evaluated independently.
pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
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

    budget.steps(count(location_ids.len())?)?;
    if location_ids.len() < 2 {
        return Ok(());
    }
    let edges = directed_edges(input, location_ids, budget)?;
    if edges.is_empty() {
        return Ok(());
    }

    let owner = EntityId::from_uuid(id.as_uuid());
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? {
            continue;
        }
        let matching = matching_shifts(input, shifts, scope, location_ids, budget)?;
        if matching.len() < 2 {
            continue;
        }
        evaluate_person(*person_id, &matching, &edges, owner, summary, budget)?;
    }
    Ok(())
}

fn directed_edges(
    input: &AssignmentInput,
    location_ids: &[LocationId],
    budget: &mut OperationBudget<'_>,
) -> Result<BTreeMap<LocationId, Vec<(LocationId, u32)>>, AssignmentRuleError> {
    let mut edges: BTreeMap<LocationId, Vec<(LocationId, u32)>> = BTreeMap::new();
    for source in location_ids {
        budget.step()?;
        let Some(WorkforceEntity::Location(location)) =
            input.domain.entities.get(&(*source).as_entity_id())
        else {
            return Err(invalid());
        };
        for transition in &location.transitions {
            budget.step()?;
            budget.steps(count(location_ids.len())?)?;
            if *source == transition.location_id || !location_ids.contains(&transition.location_id)
            {
                continue;
            }
            if !edges.contains_key(source) {
                budget.reserve(0, 1, 32)?;
            }
            budget.reserve(0, 1, 32)?;
            edges
                .entry(*source)
                .or_default()
                .push((transition.location_id, transition.minutes));
        }
    }
    Ok(edges)
}

fn matching_shifts<'a>(
    input: &AssignmentInput,
    shifts: &[&'a ResolvedShift],
    scope: &Scope,
    location_ids: &[LocationId],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Matching<'a>>, AssignmentRuleError> {
    let mut matching = Vec::new();
    for shift in shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if !predicates::shift_matches(shift, &metadata, scope, budget)? {
            continue;
        }
        let Some(location) = metadata.location_id else {
            continue;
        };
        budget.steps(count(location_ids.len())?)?;
        if !location_ids.contains(&location) {
            continue;
        }
        budget.reserve(0, 1, 32)?;
        matching.push(Matching { shift, location });
    }
    Ok(matching)
}

fn evaluate_person(
    person_id: eutheto_types::PersonId,
    matching: &[Matching<'_>],
    edges: &BTreeMap<LocationId, Vec<(LocationId, u32)>>,
    owner: EntityId,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut by_location = BTreeMap::<LocationId, Vec<&Matching<'_>>>::new();
    for entry in matching {
        budget.step()?;
        if !by_location.contains_key(&entry.location) {
            budget.reserve(0, 1, 32)?;
        }
        budget.reserve(0, 1, 16)?;
        by_location.entry(entry.location).or_default().push(entry);
    }

    for source in matching {
        budget.step()?;
        let Some(outgoing) = edges.get(&source.location) else {
            continue;
        };
        for &(target_location, required_minutes) in outgoing {
            budget.step()?;
            let Some(targets) = by_location.get(&target_location) else {
                continue;
            };
            let source_start = source.shift.interval.starts_at.instant;
            let first_target = first_at_or_after(targets, source_start, budget)?;
            let applicable = count(targets.len() - first_target)?;
            summary.checked = add(summary.checked, applicable)?;
            let required = SignedDuration::from_secs(i64::from(required_minutes) * 60);
            let required_nanos = i128::from(required_minutes)
                .checked_mul(60_000_000_000)
                .ok_or_else(invalid)?;
            for target in &targets[first_target..] {
                budget.step()?;
                let actual = source
                    .shift
                    .interval
                    .ends_at
                    .instant
                    .as_timestamp()
                    .duration_until(target.shift.interval.starts_at.instant.as_timestamp());
                let actual_nanos = i128::from(actual.as_secs())
                    .checked_mul(1_000_000_000)
                    .and_then(|value| value.checked_add(i128::from(actual.subsec_nanos())))
                    .ok_or_else(invalid)?;
                let failed = actual < required;
                if summary.collecting_health() {
                    summary.record_health(
                        HealthObservation {
                            kind: HealthObservationKind::TransitionTime,
                            rule_id: None,
                            coverage_owner: None,
                            person_id: Some(person_id),
                            shift_id: Some(source.shift.id),
                            other_shift_id: Some(target.shift.id),
                            period: None,
                            units: HealthUnits::Minutes,
                            direction: HealthDirection::Minimum,
                            threshold: Some(required_nanos),
                            observed: Some(actual_nanos),
                            satisfied: Some(!failed),
                            minimum_rank: None,
                            affected_pairs: vec![
                                AssignmentPair {
                                    person_id,
                                    shift_id: source.shift.id,
                                },
                                AssignmentPair {
                                    person_id,
                                    shift_id: target.shift.id,
                                },
                            ],
                            affected_pair_count: 2,
                            affected_pairs_truncated: false,
                        },
                        budget,
                    )?;
                }
                if failed {
                    let mut witness = Witness::pair(
                        AssignmentPair {
                            person_id,
                            shift_id: source.shift.id,
                        },
                        owner,
                        "rule",
                        "transition_time",
                    );
                    witness.other_shift = Some(target.shift.id);
                    witness.transition = Some(TransitionWitness {
                        source_location: source.location,
                        target_location,
                        required_minutes,
                        actual,
                    });
                    summary.failure(&witness, &witness, None, "direct", budget)?;
                } else if !summary.collecting_health() {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn first_at_or_after(
    targets: &[&Matching<'_>],
    start: Rfc3339Timestamp,
    budget: &mut OperationBudget<'_>,
) -> Result<usize, AssignmentRuleError> {
    let mut low = 0;
    let mut high = targets.len();
    while low < high {
        budget.step()?;
        let middle = low + (high - low) / 2;
        if targets[middle].shift.interval.starts_at.instant < start {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(low)
}
