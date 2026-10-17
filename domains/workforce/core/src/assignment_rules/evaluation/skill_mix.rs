use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add, count},
    input::AssignmentInput,
};
use super::{Selected, Summary, Witness, invalid, predicates};
use crate::{
    ids::QualificationId,
    model::{QualificationMatch, QualificationMinimum, WorkforceRule},
};
use eutheto_types::EntityId;
use std::collections::BTreeSet;

struct Minimum {
    expression: QualificationMatch,
    required: u16,
    hash: [u8; 32],
}

// Normalize source sets once per rule, independently of compiler planning state.
fn minima(
    values: &[QualificationMinimum],
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Minimum>, AssignmentRuleError> {
    let mut keys = BTreeSet::<(Vec<QualificationId>, Vec<QualificationId>, u16)>::new();
    for minimum in values {
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
    for (all_qualification_ids, any_qualification_ids, required) in keys {
        budget.step()?;
        let mut hash = blake3::Hasher::new();
        serde_json::to_writer(
            &mut hash,
            &(&all_qualification_ids, &any_qualification_ids, required),
        )
        .map_err(|_| invalid())?;
        budget.reserve(0, 1, 32)?;
        canonical.push(Minimum {
            expression: QualificationMatch {
                all_qualification_ids,
                any_qualification_ids,
            },
            required,
            hash: *hash.finalize().as_bytes(),
        });
    }
    Ok(canonical)
}

pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::RequiredSkillMix {
        id,
        scope,
        qualification_minimums,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    let minima = minima(qualification_minimums, budget)?;
    let owner = EntityId::from_uuid(id.as_uuid());
    for shift in &input.shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if !predicates::shift_matches(shift, &metadata, scope, budget)? {
            continue;
        }
        for (rank, minimum) in minima.iter().enumerate() {
            budget.step()?;
            let mut matching = 0;
            if let Some(people) = selected.by_shift.get(&shift.id) {
                for person_id in people {
                    budget.step()?;
                    let person = input.person(*person_id).ok_or_else(invalid)?;
                    if !predicates::person_matches(person, scope, budget)? {
                        continue;
                    }
                    if predicates::qualification_match(
                        person,
                        &minimum.expression,
                        predicates::interval(shift),
                        budget,
                    )? {
                        matching = add(matching, 1)?;
                    }
                }
            }
            let witness = Witness {
                person: None,
                other_person: None,
                shift: Some(shift.id),
                other_shift: None,
                owner,
                owner_kind: "required_skill_mix",
                reason: "required_skill_mix_qualification_minimum",
                minimum_rank: count(rank)?,
                minimum_hash: Some(minimum.hash),
                lower: Some(u64::from(minimum.required)),
                upper: None,
                actual: Some(matching),
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
            let failed = matching < u64::from(minimum.required);
            if failed && summary.collecting_repair() {
                let mut members = Vec::new();
                if let Some(associations) = selected.associations() {
                    for pair in associations {
                        budget.step()?;
                        if pair.shift_id != shift.id {
                            continue;
                        }
                        let person = input.person(pair.person_id).ok_or_else(invalid)?;
                        if predicates::person_matches(person, scope, budget)?
                            && predicates::qualification_match(
                                person,
                                &minimum.expression,
                                predicates::interval(shift),
                                budget,
                            )?
                        {
                            budget.reserve(0, 1, 48)?;
                            members.push(*pair);
                        }
                    }
                }
                summary.predicate_with_members(
                    failed,
                    &witness,
                    Some(members),
                    "required_skill_mix",
                    budget,
                )?;
            } else {
                summary.predicate(failed, &witness, budget)?;
            }
        }
    }
    Ok(())
}
