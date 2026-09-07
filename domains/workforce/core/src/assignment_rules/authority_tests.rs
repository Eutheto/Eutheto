use super::{assess, finish, operation_error};
use crate::assignment_rules::{
    AssignmentRuleError, AssignmentRuleLimit, MAX_WORK_STEPS, budget::OperationBudget,
    workforce_verification_scope,
};
use eutheto_domain_ir::{NormalizedSolution, VerificationContextV1, blake3_hex};
use eutheto_types::{CancellationToken, ScenarioDocument};
use std::error::Error;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn fixture() -> Result<(ScenarioDocument, NormalizedSolution)> {
    let mut document = crate::test_support::fixture()?;
    document
        .domain
        .entities
        .remove(&crate::test_support::id(6).parse()?);
    document.domain.locked_assignments.clear();
    let solution = NormalizedSolution {
        schema_version: 1,
        pack_id: document.domain_pack.id.clone(),
        scenario_id: document.scenario_id,
        scenario_revision: 1,
        projection_version: 1,
        solution_id: crate::test_support::id(700).parse()?,
        assignments: Vec::new(),
    };
    Ok((document, solution))
}

#[test]
fn cancellation_after_entry_aborts_input_traversal_and_report_finalization() -> Result<()> {
    let (document, solution) = fixture()?;
    let token = CancellationToken::new();
    let mut budget = OperationBudget::evaluation(Some(&token));
    budget.cancel_after_steps(10)?;
    assert!(matches!(
        assess(&document, &solution, &mut budget),
        Err(eutheto_domain_api::DomainPackError::Cancelled)
    ));
    assert!(token.is_cancelled());

    let token = CancellationToken::new();
    let mut budget = OperationBudget::evaluation(Some(&token));
    let assessment = assess(&document, &solution, &mut budget)?;
    let scope = workforce_verification_scope(&document, 1, None)?;
    let context = VerificationContextV1::new(
        document.scenario_id,
        1,
        blake3_hex(&serde_json::to_vec(&document)?),
        "0".repeat(64),
        solution.canonical_hash()?,
        scope.checksum,
    )?;
    budget.cancel_after_steps(1)?;
    assert_eq!(
        finish(assessment, &solution, &context, &mut budget),
        Err(eutheto_domain_api::DomainPackError::Cancelled)
    );
    assert!(token.is_cancelled());
    Ok(())
}

#[test]
fn source_authority_never_refills_work_or_retained_scratch() -> Result<()> {
    let (document, solution) = fixture()?;
    let mut work = OperationBudget::evaluation(None);
    work.steps(MAX_WORK_STEPS)?;
    assert!(matches!(assess(&document, &solution, &mut work), Err(error)
        if error == operation_error(&AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps))));
    let mut scratch = OperationBudget::evaluation(None);
    scratch.reserve(0, 0, 16 * 1024 * 1024)?;
    assert!(
        matches!(assess(&document, &solution, &mut scratch), Err(error)
        if error == operation_error(&AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::Bytes)))
    );
    Ok(())
}
