//! Original-source authority. No compiler candidate graph, constraints or objective terms enter here.

use super::{
    AssignmentRuleError, AssignmentRuleLimit, UnsupportedWorkforceObligation,
    boundary::{DOMAIN_LIMITS, Measured, preflight},
    budget::{MAX_SELECTED_PAIRS, OperationBudget, count, within},
    evaluation::{CHECKED, VIOLATIONS, evaluate_validated_selection},
    identity::{RANK_CATEGORY, RANK_LEVEL, WORKFORCE_PROJECTION_VERSION},
    input::AssignmentInput,
    projection::decode_workforce_assignment,
};
use crate::{
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{AssignmentPair, AvailabilityKind, LockState, WorkforceEntity, WorkforceRule},
};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError};
use eutheto_domain_ir::{
    AcceptedResult, MetricId, MetricValue, NormalizedSolution, OptimizationDirection,
    RequiredRuleBinding, RuleEvaluation, ScoreCategoryId, ScoreLevelId, ScoreLevelValue,
    ScoreVector, VerificationContextV1, VerificationFactId, VerificationReport, VerificationScope,
    VerificationValue,
};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{
    EntityId, OperationControl, REVISION_MAX_V1, RuleId, ScenarioDocument, ScenarioId,
};
use std::collections::BTreeMap;

#[cfg(test)]
#[path = "authority_tests.rs"]
mod tests;

const SCOPE_CONTEXT: &[u8] = b"eutheto/workforce/verification-scope/v1\0";
pub(super) const SELECTED_METRIC: &str = "official.workforce.metric.selected_assignments";
pub(super) const CHECKED_METRIC: &str = "official.workforce.metric.checked_predicates";
pub(super) const VIOLATIONS_METRIC: &str = "official.workforce.metric.violations";

/// Declares every original Required rule and unconditional fact independently of evaluator coverage.
///
/// # Errors
/// Rejects invalid or unsupported source semantics, revisions, cancellation and finite-work limits.
pub fn workforce_verification_scope(
    document: &ScenarioDocument,
    scenario_revision: u64,
    control: Option<&OperationControl>,
) -> Result<VerificationScope, DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    budget.check().map_err(|error| operation_error(&error))?;
    check_revision(scenario_revision)?;
    let input = prepare_source(document, &mut budget)?;
    let obligations = original_obligations(&input, &mut budget)?;
    make_scope(
        document.scenario_id,
        scenario_revision,
        &input.source_document_hash,
        &obligations,
        &mut budget,
    )
}

/// Computes feasibility and the initial minimized assignment rank from original source identities.
///
/// # Errors
/// Rejects malformed true or false decisions, unsupported policies and bounded-operation failures.
pub fn score_workforce_solution(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    control: Option<&OperationControl>,
) -> Result<ScoreVector, DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    Ok(assess(document, solution, &mut budget)?.score)
}

/// Verifies complete original-domain obligations and rejects a supplied score that is not authoritative.
///
/// The planning-model hash is checked syntactically and retained from the caller's context; the
/// generic acceptance reviewer, not this original-domain evaluator, binds it to an actual model.
///
/// # Errors
/// Rejects source/solution/context/score disagreement, unsupported semantics and operation limits.
pub fn verify_workforce_solution(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    context: &VerificationContextV1,
    authoritative_score: &ScoreVector,
    control: Option<&OperationControl>,
) -> Result<VerificationReport, DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    budget.check().map_err(|error| operation_error(&error))?;
    check_rank_shape(authoritative_score)?;
    preflight(context, &mut budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    preflight(authoritative_score, &mut budget, DOMAIN_LIMITS)
        .map_err(|error| operation_error(&error))?;
    let assessment = assess(document, solution, &mut budget)?;
    context.validate().map_err(|_| contract("context"))?;
    budget
        .reserve(0, 1, 128)
        .map_err(|error| operation_error(&error))?;
    authoritative_score
        .validate_current_shape()
        .map_err(|_| contract("score"))?;
    if &assessment.score != authoritative_score {
        return Err(contract("score_mismatch"));
    }
    finish(assessment, solution, context, &mut budget)
}

/// Public checksums establish consistency, not truth. Share re-evaluates the complete source.
pub(super) fn revalidate_workforce_accepted(
    document: &ScenarioDocument,
    accepted: &AcceptedResult,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    budget.check().map_err(|error| operation_error(&error))?;
    check_rank_shape(&accepted.verification.score)?;
    let whole =
        preflight(accepted, budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    let report = preflight(&accepted.verification, budget, DOMAIN_LIMITS)
        .map_err(|error| operation_error(&error))?;
    let solution_size = preflight(&accepted.solution, budget, DOMAIN_LIMITS)
        .map_err(|error| operation_error(&error))?;
    let input = prepare_source(document, budget)?;
    whole
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))?;
    report
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(
            0,
            1,
            solution_size
                .bytes
                .checked_add(128)
                .ok_or_else(|| contract("arithmetic"))?,
        )
        .map_err(|error| operation_error(&error))?;
    accepted
        .validate()
        .map_err(|_| contract("accepted_result"))?;
    let assessment = assess_prepared(document, &accepted.solution, input, solution_size, budget)?;
    let supplied = &accepted.verification;
    budget
        .reserve(1, 6, 512)
        .map_err(|error| operation_error(&error))?;
    let context = VerificationContextV1::new(
        supplied.scenario_id,
        supplied.evaluated_revision,
        supplied.document_hash.clone(),
        supplied.planning_model_hash.clone(),
        supplied.normalized_solution_hash.clone(),
        supplied.verification_scope_checksum.clone(),
    )
    .map_err(|_| contract("context"))?;
    let fresh = finish(assessment, &accepted.solution, &context, budget)?;
    if !fresh.accepted || fresh.score.feasibility != 0 || fresh != *supplied {
        return Err(contract("accepted_result_mismatch"));
    }
    Ok(())
}

pub(crate) fn validate_input_bounds<T: serde::Serialize>(
    value: &T,
    control: Option<&OperationControl>,
) -> Result<(), DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    preflight(value, &mut budget, ContractJsonLimits::DEFAULT)
        .map(|_| ())
        .map_err(|error| operation_error(&error))
}

fn check_rank_shape(score: &ScoreVector) -> Result<(), DomainPackError> {
    // The generic shape validator builds an identity set before checking its level ceiling.
    if score.levels.len() != 1 || score.levels[0].category_breakdown.len() > 1 {
        return Err(contract("score_shape"));
    }
    Ok(())
}

pub(super) fn prepare_source(
    document: &ScenarioDocument,
    budget: &mut OperationBudget<'_>,
) -> Result<AssignmentInput, DomainPackError> {
    let measured = preflight(document, budget, ContractJsonLimits::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    measured
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))?;
    AssignmentInput::new(document, budget).map_err(|error| operation_error(&error))
}

struct Assessment {
    document_hash: String,
    solution_size: Measured,
    obligations: Vec<(RuleId, &'static str)>,
    evaluations: Vec<RuleEvaluation>,
    score: ScoreVector,
    selected: i64,
    checked: i64,
}

fn assess(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    budget: &mut OperationBudget<'_>,
) -> Result<Assessment, DomainPackError> {
    let solution_size =
        preflight(solution, budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    let input = prepare_source(document, budget)?;
    assess_prepared(document, solution, input, solution_size, budget)
}

fn assess_prepared(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    input: AssignmentInput,
    solution_size: Measured,
    budget: &mut OperationBudget<'_>,
) -> Result<Assessment, DomainPackError> {
    solution.validate().map_err(|_| contract("solution"))?;
    if solution.pack_id.as_str() != WORKFORCE_PACK_ID
        || solution.scenario_id != document.scenario_id
        || solution.projection_version != WORKFORCE_PROJECTION_VERSION
        || solution.solution_id.as_uuid().get_version_num() != 7
    {
        return Err(contract("solution_binding"));
    }
    within(
        count(solution.assignments.len()).map_err(|error| operation_error(&error))?,
        PlanningIrLimitsV1::DEFAULT.max_projections,
        AssignmentRuleLimit::Records,
    )
    .map_err(|error| operation_error(&error))?;
    let obligations = original_obligations(&input, budget)?;
    let mut selected = Vec::new();
    for assignment in &solution.assignments {
        budget.step().map_err(|error| operation_error(&error))?;
        let (pair, enabled) = decode_workforce_assignment(assignment)?;
        validate_pair(&input, pair)?;
        if enabled {
            within(
                count(selected.len() + 1).map_err(|error| operation_error(&error))?,
                MAX_SELECTED_PAIRS,
                AssignmentRuleLimit::SelectedPairs,
            )
            .map_err(|error| operation_error(&error))?;
            budget
                .reserve(1, 1, 32)
                .map_err(|error| operation_error(&error))?;
            selected.push(pair);
        }
    }
    // Canonical assignment IDs and exact pair decoding establish uniqueness for both values.
    let (evaluations, partition) =
        evaluate_validated_selection(&input, &selected, &document.settings, budget)
            .map_err(|error| operation_error(&error))?;
    if !partition.remaining.is_empty()
        || evaluations.len() != obligations.len()
        || evaluations
            .iter()
            .zip(&obligations)
            .any(|(evaluation, (id, _))| evaluation.rule_id != *id)
    {
        return Err(contract("incomplete_evaluation"));
    }
    let (violations, checked) = evaluation_counts(&evaluations, budget)?;
    let rank = original_rank(&input, &selected, budget)?;
    let score = rank_score(violations, rank, budget)?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(Assessment {
        document_hash: input.source_document_hash,
        solution_size,
        obligations,
        evaluations,
        score,
        selected: i64::try_from(selected.len()).map_err(|_| contract("arithmetic"))?,
        checked,
    })
}

pub(super) fn validate_pair(
    input: &AssignmentInput,
    pair: AssignmentPair,
) -> Result<(), DomainPackError> {
    if input.person(pair.person_id).is_none() {
        return Err(contract(
            if input
                .domain
                .entities
                .contains_key(&EntityId::from_uuid(pair.person_id.as_uuid()))
            {
                "person_kind"
            } else {
                "person_missing"
            },
        ));
    }
    if input.shift(pair.shift_id).is_none() {
        return Err(contract("shift_missing"));
    }
    Ok(())
}

pub(super) fn original_obligations(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(RuleId, &'static str)>, DomainPackError> {
    let mut obligations = Vec::new();
    let mut policy = false;
    for entity in input.domain.entities.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let obligation = match entity {
            WorkforceEntity::Person(person) => {
                Some((RuleId::from_uuid(person.id.as_uuid()), "person_activity"))
            }
            WorkforceEntity::Availability(record)
                if record.availability_kind == AvailabilityKind::ApprovedTimeOff =>
            {
                Some((
                    RuleId::from_uuid(record.id.as_entity_id().as_uuid()),
                    "approved_leave",
                ))
            }
            WorkforceEntity::ScorePolicy(_) => {
                policy = true;
                None
            }
            _ => None,
        };
        if let Some(obligation) = obligation {
            budget
                .reserve(1, 2, 48)
                .map_err(|error| operation_error(&error))?;
            obligations.push(obligation);
        }
    }
    if !policy {
        return Err(operation_error(&AssignmentRuleError::MissingScorePolicy));
    }
    let mut unsupported: Option<UnsupportedWorkforceObligation> = None;
    for rule in input.domain.rules.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let (id, active, _) = rule.header();
        if !active {
            continue;
        }
        if matches!(
            rule,
            WorkforceRule::Eligibility { .. }
                | WorkforceRule::Availability { .. }
                | WorkforceRule::Coverage { .. }
                | WorkforceRule::NoOverlap { .. }
                | WorkforceRule::MinimumRest(_)
        ) {
            budget
                .reserve(1, 2, 48)
                .map_err(|error| operation_error(&error))?;
            obligations.push((id, "required_rule"));
        } else {
            let value = UnsupportedWorkforceObligation::RequiredRule(id);
            unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
        }
    }
    for preference in input.domain.preferences.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let (id, active, ..) = preference.header();
        if active {
            let value = UnsupportedWorkforceObligation::Preference(id);
            unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
        }
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let value = match lock.state {
            LockState::Hard {} => UnsupportedWorkforceObligation::HardLock(lock.id),
            LockState::Soft { .. } => UnsupportedWorkforceObligation::SoftLock(lock.id),
            LockState::Unlocked {} => continue,
        };
        unsupported = Some(unsupported.map_or(value, |prior| prior.min(value)));
    }
    if let Some(value) = unsupported {
        return Err(operation_error(
            &AssignmentRuleError::UnsupportedObligation(value),
        ));
    }
    budget
        .sort_work(obligations.len())
        .map_err(|error| operation_error(&error))?;
    obligations.sort_unstable_by_key(|(id, _)| *id);
    if obligations.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(contract("duplicate_obligation"));
    }
    Ok(obligations)
}

fn make_scope(
    scenario_id: ScenarioId,
    revision: u64,
    document_hash: &str,
    obligations: &[(RuleId, &'static str)],
    budget: &mut OperationBudget<'_>,
) -> Result<VerificationScope, DomainPackError> {
    check_revision(revision)?;
    let mut bindings = Vec::new();
    for &(rule_id, kind) in obligations {
        budget.step().map_err(|error| operation_error(&error))?;
        budget
            .reserve(1, 2, 96)
            .map_err(|error| operation_error(&error))?;
        let mut hash = blake3::Hasher::new();
        hash.update(SCOPE_CONTEXT);
        hash.update(document_hash.as_bytes());
        hash.update(&[0]);
        hash.update(kind.as_bytes());
        hash.update(&[0]);
        hash.update(rule_id.as_uuid().as_bytes());
        bindings.push(RequiredRuleBinding {
            rule_id,
            semantic_hash: hash.finalize().to_hex().to_string(),
        });
    }
    let measured =
        preflight(&bindings, budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    measured
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))?;
    // Fixed UUID/version/checksum header and the constructor's final serialized-size pass.
    budget
        .reserve(
            1,
            8,
            measured
                .bytes
                .checked_add(4096)
                .ok_or_else(|| contract("arithmetic"))?,
        )
        .map_err(|error| operation_error(&error))?;
    let scope =
        VerificationScope::new(scenario_id, revision, bindings).map_err(|_| contract("scope"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(scope)
}

pub(super) fn original_rank(
    input: &AssignmentInput,
    selected: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<i64, DomainPackError> {
    budget.check().map_err(|error| operation_error(&error))?;
    if selected.is_empty() {
        return Ok(0);
    }
    let shift_count = count(input.shifts.len()).map_err(|error| operation_error(&error))?;
    budget
        .reserve(
            0,
            shift_count,
            shift_count
                .checked_mul(16)
                .ok_or_else(|| contract("arithmetic"))?,
        )
        .map_err(|error| operation_error(&error))?;
    budget
        .steps(shift_count)
        .map_err(|error| operation_error(&error))?;
    let mut shifts: Vec<_> = input.shifts.iter().map(|shift| shift.id).collect();
    budget
        .sort_work(shifts.len())
        .map_err(|error| operation_error(&error))?;
    shifts.sort_unstable();
    let mut total = 0_i64;
    for pair in selected {
        budget
            .steps(
                u64::from(usize::BITS - input.people.len().leading_zeros())
                    + u64::from(usize::BITS - shifts.len().leading_zeros()),
            )
            .map_err(|error| operation_error(&error))?;
        let person = u64::try_from(
            input
                .people
                .binary_search(&pair.person_id)
                .map_err(|_| contract("person_missing"))?,
        )
        .map_err(|_| contract("arithmetic"))?;
        let shift = u64::try_from(
            shifts
                .binary_search(&pair.shift_id)
                .map_err(|_| contract("shift_missing"))?,
        )
        .map_err(|_| contract("arithmetic"))?;
        let rank = person
            .checked_mul(shift_count)
            .and_then(|value| value.checked_add(shift))
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| contract("arithmetic"))?;
        let rank = i64::try_from(rank).map_err(|_| contract("arithmetic"))?;
        if rank > PlanningIrLimitsV1::DEFAULT.max_abs_coefficient {
            return Err(contract("rank_limit"));
        }
        total = total
            .checked_add(rank)
            .ok_or_else(|| contract("arithmetic"))?;
        if total > PlanningIrLimitsV1::DEFAULT.max_abs_value {
            return Err(contract("rank_limit"));
        }
    }
    Ok(total)
}

fn rank_score(
    feasibility: i64,
    rank: i64,
    budget: &mut OperationBudget<'_>,
) -> Result<ScoreVector, DomainPackError> {
    budget
        .reserve(1, 3, 512)
        .map_err(|error| operation_error(&error))?;
    let mut category_breakdown = BTreeMap::new();
    if rank != 0 {
        category_breakdown.insert(
            ScoreCategoryId::new(RANK_CATEGORY).map_err(|_| contract("score_identity"))?,
            rank,
        );
    }
    let score = ScoreVector {
        feasibility,
        levels: vec![ScoreLevelValue {
            level_id: ScoreLevelId::new(RANK_LEVEL).map_err(|_| contract("score_identity"))?,
            value: rank,
            direction: OptimizationDirection::Minimize,
            category_breakdown,
        }],
    };
    score
        .validate_current_shape()
        .map_err(|_| contract("score"))?;
    Ok(score)
}

fn finish(
    assessment: Assessment,
    solution: &NormalizedSolution,
    context: &VerificationContextV1,
    budget: &mut OperationBudget<'_>,
) -> Result<VerificationReport, DomainPackError> {
    let scope = make_scope(
        solution.scenario_id,
        solution.scenario_revision,
        &assessment.document_hash,
        &assessment.obligations,
        budget,
    )?;
    budget
        .reserve(0, 0, assessment.solution_size.bytes)
        .map_err(|error| operation_error(&error))?;
    let solution_hash = solution
        .canonical_hash()
        .map_err(|_| contract("solution_hash"))?;
    if context.scenario_id != solution.scenario_id
        || context.evaluated_revision != solution.scenario_revision
        || context.document_hash != assessment.document_hash
        || context.normalized_solution_hash != solution_hash
        || context.verification_scope_checksum != scope.checksum
    {
        return Err(contract("context_binding"));
    }
    budget
        .reserve(3, 6, 512)
        .map_err(|error| operation_error(&error))?;
    let metrics = [
        (SELECTED_METRIC, assessment.selected),
        (CHECKED_METRIC, assessment.checked),
        (VIOLATIONS_METRIC, assessment.score.feasibility),
    ]
    .into_iter()
    .map(|(id, value)| {
        Ok((
            MetricId::new(id).map_err(|_| contract("metric_identity"))?,
            MetricValue::Integer(value),
        ))
    })
    .collect::<Result<BTreeMap<_, _>, DomainPackError>>()?;
    let measured = preflight(
        &(
            context,
            &assessment.evaluations,
            &assessment.score,
            &metrics,
        ),
        budget,
        DOMAIN_LIMITS,
    )
    .map_err(|error| operation_error(&error))?;
    measured
        .reserve_json(budget, 1)
        .map_err(|error| operation_error(&error))?;
    budget
        .reserve(
            1,
            16,
            measured
                .bytes
                .checked_add(4096)
                .ok_or_else(|| contract("arithmetic"))?,
        )
        .map_err(|error| operation_error(&error))?;
    let report = VerificationReport::new(
        context,
        assessment.evaluations,
        assessment.score,
        Vec::new(),
        metrics,
    )
    .map_err(|_| contract("report"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(report)
}

pub(super) fn evaluation_counts(
    evaluations: &[RuleEvaluation],
    budget: &mut OperationBudget<'_>,
) -> Result<(i64, i64), DomainPackError> {
    budget
        .reserve(0, 2, 128)
        .map_err(|error| operation_error(&error))?;
    let violations_id =
        VerificationFactId::new(VIOLATIONS).map_err(|_| contract("fact_identity"))?;
    let checked_id = VerificationFactId::new(CHECKED).map_err(|_| contract("fact_identity"))?;
    let mut violations = 0_i64;
    let mut checked = 0_i64;
    for evaluation in evaluations {
        budget.step().map_err(|error| operation_error(&error))?;
        let count = integer_fact(evaluation, &violations_id)?;
        if evaluation.satisfied != (count == 0) {
            return Err(contract("inconsistent_evaluation"));
        }
        violations = violations
            .checked_add(count)
            .ok_or_else(|| contract("arithmetic"))?;
        checked = checked
            .checked_add(integer_fact(evaluation, &checked_id)?)
            .ok_or_else(|| contract("arithmetic"))?;
    }
    Ok((violations, checked))
}

fn integer_fact(
    evaluation: &RuleEvaluation,
    id: &VerificationFactId,
) -> Result<i64, DomainPackError> {
    match evaluation.observed.get(id) {
        Some(VerificationValue::Integer(value)) if *value >= 0 => Ok(*value),
        _ => Err(contract("evaluation_fact")),
    }
}
fn check_revision(value: u64) -> Result<(), DomainPackError> {
    if value > REVISION_MAX_V1 {
        Err(contract("revision"))
    } else {
        Ok(())
    }
}
pub(crate) fn operation_error(error: &AssignmentRuleError) -> DomainPackError {
    match error {
        AssignmentRuleError::Cancelled => DomainPackError::Cancelled,
        AssignmentRuleError::BudgetExpired => DomainPackError::BudgetExpired,
        _ => DomainPackError::Contract(error.code().to_owned()),
    }
}
pub(super) fn contract(code: &'static str) -> DomainPackError {
    DomainPackError::Contract(format!("official.workforce.authority.{code}"))
}
