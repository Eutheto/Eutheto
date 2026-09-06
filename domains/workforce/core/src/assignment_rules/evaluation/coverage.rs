use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add, count},
    input::AssignmentInput,
};
use super::{Selected, Summary, Witness, invalid, predicates};
use crate::{
    ids::QualificationId,
    model::{Coverage, QualificationMatch, Scope, ShiftScope, WorkforceEntity},
    temporal::ResolvedShift,
};
use eutheto_types::{EntityId, PersonId};
use std::collections::{BTreeMap, BTreeSet};

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
                check(input, requirement, shift, &population, summary, budget)?;
            }
        }
    }
    Ok(())
}

fn check(
    input: &AssignmentInput,
    requirement: &Requirement<'_>,
    shift: &ResolvedShift,
    population: &[PersonId],
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    let actual = count(population.len())?;
    let mut witness = Witness {
        person: None,
        shift: shift.id,
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
        rest: None,
    };
    summary.predicate(
        actual < requirement.lower || requirement.upper.is_some_and(|upper| actual > upper),
        witness,
        budget,
    )?;
    for (rank, minimum) in requirement.minima.iter().enumerate() {
        budget.step()?;
        let mut matching = 0;
        for person_id in population {
            budget.step()?;
            if predicates::qualification_match(
                input.person(*person_id).ok_or_else(invalid)?,
                &minimum.expression,
                predicates::interval(shift),
                budget,
            )? {
                matching = add(matching, 1)?;
            }
        }
        witness.reason = "coverage_qualification_minimum";
        witness.minimum_rank = count(rank)?;
        witness.minimum_hash = Some(minimum.hash);
        witness.lower = Some(u64::from(minimum.required));
        witness.upper = None;
        witness.actual = Some(matching);
        summary.predicate(matching < u64::from(minimum.required), witness, budget)?;
    }
    Ok(())
}
