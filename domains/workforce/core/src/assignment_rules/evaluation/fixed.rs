use super::super::{
    AssignmentConstructionIssue, AssignmentRuleError, budget::OperationBudget,
    input::AssignmentInput,
};
use super::{Selected, Summary, Witness, predicates};
use crate::{
    ids::ShiftId,
    model::{AssignmentPair, Scope},
};
use eutheto_domain_api::DomainPackError;
use eutheto_types::{EntityId, PersonId, RuleId};

/// Evaluates one authored `FixedAssignment` against the selected original-domain pair set.
///
/// The target scope is checked again here rather than relying on ingress validation. This keeps
/// source authority independent if an already-decoded document reaches the evaluator through a
/// future import path.
#[allow(clippy::too_many_arguments)]
pub(super) fn fixed_assignment(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule_id: RuleId,
    scope: &Scope,
    pair: AssignmentPair,
    assigned: bool,
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    validate_target_scope(input, rule_id, scope, pair, budget)?;
    let mut summary = if collect_repair {
        Summary::repair()
    } else {
        Summary::default()
    };
    let witness = Witness::pair(
        pair,
        EntityId::from_uuid(rule_id.as_uuid()),
        "fixed_assignment",
        if assigned {
            "fixed_assignment_missing"
        } else {
            "fixed_assignment_excluded_selected"
        },
    );
    summary.predicate(
        selected.contains(pair, budget)? != assigned,
        &witness,
        budget,
    )?;
    Ok(summary)
}

/// Evaluates one authored Hard lock independently of any base schedule or compiler candidate.
pub(super) fn hard_lock(
    selected: &Selected<'_>,
    lock_id: RuleId,
    person_id: PersonId,
    shift_id: ShiftId,
    collect_repair: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<Summary, AssignmentRuleError> {
    let pair = AssignmentPair {
        person_id,
        shift_id,
    };
    let mut summary = if collect_repair {
        Summary::repair()
    } else {
        Summary::default()
    };
    let witness = Witness::pair(
        pair,
        EntityId::from_uuid(lock_id.as_uuid()),
        "hard_lock",
        "hard_lock_missing",
    );
    summary.predicate(!selected.contains(pair, budget)?, &witness, budget)?;
    Ok(summary)
}

fn validate_target_scope(
    input: &AssignmentInput,
    rule_id: RuleId,
    scope: &Scope,
    pair: AssignmentPair,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let person = input.person(pair.person_id).ok_or_else(invalid)?;
    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
    let metadata = input.metadata(shift)?;
    let matches = predicates::person_matches(person, scope, budget)?
        && predicates::shift_matches(shift, &metadata, scope, budget)?;
    if matches {
        Ok(())
    } else {
        Err(AssignmentRuleError::InvalidDocument(
            DomainPackError::InvalidPayload {
                path: format!("domain.rules.{rule_id}.scope"),
                message: "active FixedAssignment target does not match its scope".to_owned(),
            },
        ))
    }
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}
