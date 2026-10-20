use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add, count},
    input::AssignmentInput,
};
use super::{
    HealthDirection, HealthObservation, HealthObservationKind, HealthUnits,
    MAX_HEALTH_AFFECTED_PAIRS, Selected, Summary, Witness, invalid, predicates,
};
use crate::{
    ids::QualificationId,
    model::{
        AssignmentPair, Coverage, PersonSelection, QualificationMatch, Scope, ShiftScope,
        WorkforceEntity,
    },
    temporal::ResolvedShift,
};
use eutheto_types::{EntityId, PersonId};

struct Minimum {
    expression: QualificationMatch,
    required: u16,
    hash: [u8; 32],
}

struct Requirement<'a> {
    owner: EntityId,
    owner_kind: &'static str,
    scope: Option<&'a ShiftScope>,
    lower: u64,
    upper: Option<u64>,
    minima: Vec<Minimum>,
}

// Normalize source sets once per definition, never once per selected person or occurrence.
fn requirement<'a>(
    owner: EntityId,
    owner_kind: &'static str,
    scope: Option<&'a ShiftScope>,
    coverage: &Coverage,
    budget: &mut OperationBudget<'_>,
) -> Result<Requirement<'a>, AssignmentRuleError> {
    budget.step()?;
    budget.reserve(1, 1, 64)?;
    let (lower, upper, minima) = match coverage {
        Coverage::Exact {
            count,
            qualification_minimums,
        } => (
            u64::from(*count),
            Some(u64::from(*count)),
            qualification_minimums,
        ),
        Coverage::AtLeast {
            minimum,
            maximum_count,
            qualification_minimums,
            ..
        } => (
            u64::from(*minimum),
            maximum_count.map(u64::from),
            qualification_minimums,
        ),
    };
    let mut keys = BTreeSet::<(Vec<QualificationId>, Vec<QualificationId>, u16)>::new();
    for minimum in minima {
        budget.step()?;
        let expression = &minimum.qualifications;
        let items = add(
            count(expression.all_qualification_ids.len())?,
            count(expression.any_qualification_ids.len())?,
        )?;
        let bytes = budget.measure(&(expression, minimum.minimum))?;
        budget.reserve(1, items, bytes)?;
        budget.sort_work(expression.all_qualification_ids.len())?;
        budget.sort_work(expression.any_qualification_ids.len())?;
        budget.steps(items)?;
        let mut all = expression.all_qualification_ids.clone();
        let mut any = expression.any_qualification_ids.clone();
        all.sort_unstable();
        all.dedup();
        any.sort_unstable();
        any.dedup();
        keys.insert((all, any, minimum.minimum));
    }
    let mut canonical = Vec::new();
    for (all_qualification_ids, any_qualification_ids, minimum) in keys {
        budget.step()?;
        let mut hash = blake3::Hasher::new();
        serde_json::to_writer(
            &mut hash,
            &(&all_qualification_ids, &any_qualification_ids, minimum),
        )
        .map_err(|_| invalid())?;
        budget.reserve(0, 1, 32)?;
        canonical.push(Minimum {
            expression: QualificationMatch {
                all_qualification_ids,
                any_qualification_ids,
            },
            required: minimum,
            hash: *hash.finalize().as_bytes(),
        });
    }
    Ok(Requirement {
        owner,
        owner_kind,
        scope,
        lower,
        upper,
        minima: canonical,
    })
}

pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut embedded = BTreeMap::new();
    let mut standalone = Vec::new();
    for entity in input.domain.entities.values() {
        budget.step()?;
        match entity {
            WorkforceEntity::ShiftTemplate(value) => {
                embedded.insert(
                    value.id.as_entity_id(),
                    requirement(
                        value.id.as_entity_id(),
                        "shift_template",
                        None,
                        &value.coverage,
                        budget,
                    )?,
                );
            }
            WorkforceEntity::ShiftInstance(value) => {
                embedded.insert(
                    value.id.as_entity_id(),
                    requirement(
                        value.id.as_entity_id(),
                        "shift_instance",
                        None,
                        &value.coverage,
                        budget,
                    )?,
                );
            }
            WorkforceEntity::CoverageRequirement(value) if value.active => {
                standalone.push(requirement(
                    value.id.as_entity_id(),
                    "coverage_requirement",
                    Some(&value.scope),
                    &value.coverage,
                    budget,
                )?);
            }
            _ => {}
        }
    }
    for shift in &input.shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if !predicates::shift_matches(shift, &metadata, scope, budget)? {
            continue;
        }
        let mut population = Vec::new();
        if let Some(people) = selected.by_shift.get(&shift.id) {
            for person_id in people {
                budget.step()?;
                let person = input.person(*person_id).ok_or_else(invalid)?;
                if predicates::person_matches(person, scope, budget)? {
                    budget.reserve(0, 1, 16)?;
                    population.push(*person_id);
                }
            }
        }
        check(
            input,
            selected,
            scope,
            embedded
                .get(&metadata.definition.entity_id())
                .ok_or_else(invalid)?,
            shift,
            &population,
            summary,
            budget,
        )?;
        for requirement in &standalone {
            budget.step()?;
            if predicates::requirement_matches(
                requirement.scope.ok_or_else(invalid)?,
                shift,
                &metadata,
                budget,
            )? {
                check(
                    input,
                    selected,
                    scope,
                    requirement,
                    shift,
                    &population,
                    summary,
                    budget,
                )?;
            }
        }
    }
    Ok(())
}

/// Evaluates every embedded and active standalone source owner with no authored-rule filter.
/// Health uses this pass so source coverage remains visible even when a Coverage rule is absent.
pub(super) fn evaluate_all(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let scope = Scope {
        people: PersonSelection::All {},
        team_ids: None,
        assignment_type_ids: None,
        categories: None,
        weekdays: None,
        location_ids: None,
    };
    evaluate(input, selected, &scope, summary, budget)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn check(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    requirement: &Requirement<'_>,
    shift: &ResolvedShift,
    population: &[PersonId],
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let actual = count(population.len())?;
    let failed =
        actual < requirement.lower || requirement.upper.is_some_and(|upper| actual > upper);
    let mut witness = Witness {
        person: None,
        other_person: None,
        shift: Some(shift.id),
        other_shift: None,
        owner: requirement.owner,
        owner_kind: requirement.owner_kind,
        reason: "coverage_headcount",
        minimum_rank: 0,
        minimum_hash: None,
        lower: Some(requirement.lower),
        upper: requirement.upper,
        actual: Some(actual),
        interval: None,
        period: None,
        consecutive_start_date: None,
        consecutive_end_date: None,
        rest: None,
        transition: None,
        hours: None,
        quota: None,
        quota_count: None,
    };
    if summary.collecting_health() {
        record_headcount_health(requirement, shift, population, actual, summary, budget)?;
    }
    if failed && summary.collecting_repair() {
        summary.predicate_with_members(
            failed,
            &witness,
            Some(associated_pairs(
                input, selected, scope, shift, None, budget,
            )?),
            "coverage_headcount",
            budget,
        )?;
    } else {
        summary.predicate(failed, &witness, budget)?;
    }
    for (rank, minimum) in requirement.minima.iter().enumerate() {
        budget.step()?;
        let mut matching = 0;
        let mut matching_pairs = Vec::new();
        for person_id in population {
            budget.step()?;
            if predicates::qualification_match(
                input.person(*person_id).ok_or_else(invalid)?,
                &minimum.expression,
                predicates::interval(shift),
                budget,
            )? {
                matching = add(matching, 1)?;
                if summary.collecting_health() && matching_pairs.len() < MAX_HEALTH_AFFECTED_PAIRS {
                    matching_pairs.push(AssignmentPair {
                        person_id: *person_id,
                        shift_id: shift.id,
                    });
                }
            }
        }
        witness.reason = "coverage_qualification_minimum";
        witness.minimum_rank = count(rank)?;
        witness.minimum_hash = Some(minimum.hash);
        witness.lower = Some(u64::from(minimum.required));
        witness.upper = None;
        witness.actual = Some(matching);
        if summary.collecting_health() {
            let required = u64::from(minimum.required);
            summary.record_health(
                HealthObservation {
                    kind: HealthObservationKind::CoverageQualification,
                    rule_id: None,
                    coverage_owner: Some((requirement.owner, requirement.owner_kind)),
                    person_id: None,
                    shift_id: Some(shift.id),
                    other_shift_id: None,
                    period: Some((
                        shift.interval.starts_at.instant,
                        shift.interval.ends_at.instant,
                    )),
                    units: HealthUnits::People,
                    direction: HealthDirection::Minimum,
                    threshold: Some(i128::from(required)),
                    observed: Some(i128::from(matching)),
                    satisfied: Some(matching >= required),
                    minimum_rank: Some(count(rank)?),
                    affected_pairs: matching_pairs,
                    affected_pair_count: matching,
                    affected_pairs_truncated: matching
                        > u64::try_from(MAX_HEALTH_AFFECTED_PAIRS).map_err(|_| invalid())?,
                },
                budget,
            )?;
        }
        let failed = matching < u64::from(minimum.required);
        if failed && summary.collecting_repair() {
            summary.predicate_with_members(
                failed,
                &witness,
                Some(associated_pairs(
                    input,
                    selected,
                    scope,
                    shift,
                    Some(minimum),
                    budget,
                )?),
                "coverage_qualification_minimum",
                budget,
            )?;
        } else {
            summary.predicate(failed, &witness, budget)?;
        }
    }
    Ok(())
}

fn associated_pairs(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    shift: &ResolvedShift,
    minimum: Option<&Minimum>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentPair>, AssignmentRuleError> {
    let mut members = Vec::new();
    if let Some(associations) = selected.associations() {
        for pair in associations {
            budget.step()?;
            if pair.shift_id != shift.id {
                continue;
            }
            let person = input.person(pair.person_id).ok_or_else(invalid)?;
            if !predicates::person_matches(person, scope, budget)? {
                continue;
            }
            if let Some(minimum) = minimum
                && !predicates::qualification_match(
                    person,
                    &minimum.expression,
                    predicates::interval(shift),
                    budget,
                )?
            {
                continue;
            }
            budget.reserve(0, 1, 48)?;
            members.push(*pair);
        }
    }
    Ok(members)
}

fn record_headcount_health(
    requirement: &Requirement<'_>,
    shift: &ResolvedShift,
    population: &[PersonId],
    actual: u64,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let exact = requirement.upper == Some(requirement.lower);
    let observation = HealthObservation {
        kind: HealthObservationKind::CoverageHeadcount,
        rule_id: None,
        coverage_owner: Some((requirement.owner, requirement.owner_kind)),
        person_id: None,
        shift_id: Some(shift.id),
        other_shift_id: None,
        period: Some((
            shift.interval.starts_at.instant,
            shift.interval.ends_at.instant,
        )),
        units: HealthUnits::People,
        direction: if exact {
            HealthDirection::Exact
        } else {
            HealthDirection::Minimum
        },
        threshold: Some(i128::from(requirement.lower)),
        observed: Some(i128::from(actual)),
        satisfied: Some(if exact {
            actual == requirement.lower
        } else {
            actual >= requirement.lower
        }),
        minimum_rank: None,
        affected_pairs: population
            .iter()
            .take(MAX_HEALTH_AFFECTED_PAIRS)
            .map(|person_id| AssignmentPair {
                person_id: *person_id,
                shift_id: shift.id,
            })
            .collect(),
        affected_pair_count: actual,
        affected_pairs_truncated: population.len() > MAX_HEALTH_AFFECTED_PAIRS,
    };
    let maximum = requirement
        .upper
        .filter(|upper| *upper != requirement.lower)
        .map(|upper| HealthObservation {
            direction: HealthDirection::Maximum,
            threshold: Some(i128::from(upper)),
            satisfied: Some(actual <= upper),
            ..observation.clone()
        });
    summary.record_health(observation, budget)?;
    if let Some(maximum) = maximum {
        summary.record_health(maximum, budget)?;
    }
    Ok(())
}
