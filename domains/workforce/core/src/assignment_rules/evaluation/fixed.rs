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
use eutheto_domain_ir::RuleEvaluation;
use eutheto_types::{EntityId, PersonId, RuleId};

/// Evaluates one authored `FixedAssignment` against the selected original-domain pair set.
///
/// The target scope is checked again here rather than relying on ingress validation. This keeps
/// source authority independent if an already-decoded document reaches the evaluator through a
/// future import path.
pub(super) fn fixed_assignment(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule_id: RuleId,
    scope: &Scope,
    person_id: PersonId,
    shift_id: ShiftId,
    budget: &mut OperationBudget<'_>,
) -> Result<RuleEvaluation, AssignmentRuleError> {
    let pair = AssignmentPair {
        person_id,
        shift_id,
    };
    validate_target_scope(input, rule_id, scope, pair, budget)?;
    let mut summary = Summary::default();
    let witness = Witness::pair(
        pair,
        EntityId::from_uuid(rule_id.as_uuid()),
        "fixed_assignment",
        "fixed_assignment_missing",
    );
    summary.predicate(!selected.contains(pair, budget)?, &witness, budget)?;
    summary.finish(rule_id, budget)
}

/// Evaluates one authored Hard lock independently of any base schedule or compiler candidate.
pub(super) fn hard_lock(
    selected: &Selected<'_>,
    lock_id: RuleId,
    person_id: PersonId,
    shift_id: ShiftId,
    budget: &mut OperationBudget<'_>,
) -> Result<RuleEvaluation, AssignmentRuleError> {
    let pair = AssignmentPair {
        person_id,
        shift_id,
    };
    let mut summary = Summary::default();
    let witness = Witness::pair(
        pair,
        EntityId::from_uuid(lock_id.as_uuid()),
        "hard_lock",
        "hard_lock_missing",
    );
    summary.predicate(!selected.contains(pair, budget)?, &witness, budget)?;
    summary.finish(lock_id, budget)
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
