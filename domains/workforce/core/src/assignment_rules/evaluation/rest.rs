use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add, count},
    input::AssignmentInput,
};
use super::{
    HealthDirection, HealthObservation, HealthObservationKind, HealthUnits, RestWitness, Selected,
    Summary, Witness, arithmetic, invalid, predicates,
};
use crate::model::{AssignmentPair, MinimumRestRule};
use jiff::SignedDuration;

#[allow(clippy::too_many_lines)]
pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &MinimumRestRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    // u32 minutes fit exactly in signed seconds. Comparing durations avoids endpoint overflow.
    let required = SignedDuration::from_secs(i64::from(rule.minimum_minutes) * 60);
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, &rule.scope, budget)?
            || !predicates::person_matches(person, &rule.after_scope, budget)?
            || !predicates::person_matches(person, &rule.before_scope, budget)?
        {
            continue;
        }
        // These borrowed role lists retain Selected's chronological ordering. Evaluate all
        // invariant scope leaves here, never again for each pair in the conflict window.
        let mut sources = Vec::new();
        let mut targets = Vec::new();
        for shift in shifts {
            budget.step()?;
            let metadata = input.metadata(shift)?;
            if !predicates::shift_matches(shift, &metadata, &rule.scope, budget)? {
                continue;
            }
            let source = predicates::shift_matches(shift, &metadata, &rule.after_scope, budget)?;
            let target = predicates::shift_matches(shift, &metadata, &rule.before_scope, budget)?;
            if source {
                budget.reserve(0, 1, 16)?;
                sources.push((*shift, target));
            }
            if target {
                budget.reserve(0, 1, 16)?;
                targets.push(*shift);
            }
        }
        let mut first_target = 0;
        for (source, also_target) in sources {
            budget.step()?;
            // Sources are chronological too, so this cursor only advances. Equal starts stay
            // in the suffix regardless of UUID order; only the identical shift is excluded.
            while let Some(target) = targets.get(first_target) {
                budget.step()?;
                if target.interval.starts_at.instant >= source.interval.starts_at.instant {
                    break;
                }
                first_target += 1;
            }
            let applicable = count(targets.len() - first_target)?
                .checked_sub(u64::from(also_target))
                .ok_or_else(arithmetic)?;
            summary.checked = add(summary.checked, applicable)?;
            for target in &targets[first_target..] {
                budget.step()?;
                if source.id == target.id {
                    continue;
                }
                let actual = source
                    .interval
                    .ends_at
                    .instant
                    .as_timestamp()
                    .duration_until(target.interval.starts_at.instant.as_timestamp());
                let actual_nanos = i128::from(actual.as_secs())
                    .checked_mul(1_000_000_000)
                    .and_then(|value| value.checked_add(i128::from(actual.subsec_nanos())))
                    .ok_or_else(arithmetic)?;
                let required_nanos = i128::from(rule.minimum_minutes)
                    .checked_mul(60_000_000_000)
                    .ok_or_else(arithmetic)?;
                let failed = actual < required;
                if summary.collecting_health() {
                    summary.record_health(
                        HealthObservation {
                            kind: HealthObservationKind::MinimumRest,
                            rule_id: None,
                            coverage_owner: None,
                            person_id: Some(*person_id),
                            shift_id: Some(source.id),
                            other_shift_id: Some(target.id),
                            period: None,
                            units: HealthUnits::Minutes,
                            direction: HealthDirection::Minimum,
                            threshold: Some(required_nanos),
                            observed: Some(actual_nanos),
                            satisfied: Some(!failed),
                            minimum_rank: None,
                            affected_pairs: vec![
                                AssignmentPair {
                                    person_id: *person_id,
                                    shift_id: source.id,
                                },
                                AssignmentPair {
                                    person_id: *person_id,
                                    shift_id: target.id,
                                },
                            ],
                            affected_pair_count: 2,
                            affected_pairs_truncated: false,
                        },
                        budget,
                    )?;
                }
                // Every later target is safe for this source. Count that suffix above in
                // checked arithmetic rather than walking quadratic numbers of passing pairs.
                if failed {
                    let mut witness = Witness::pair(
                        AssignmentPair {
                            person_id: *person_id,
                            shift_id: source.id,
                        },
                        source.id.as_entity_id(),
                        "shift",
                        "minimum_rest",
                    );
                    witness.other_shift = Some(target.id);
                    witness.rest = Some(RestWitness {
                        minimum_minutes: rule.minimum_minutes,
                        actual,
                    });
                    summary.failure(&witness, &witness, None, "direct", budget)?;
                } else {
                    break;
                }
            }
        }
    }
    Ok(())
}
