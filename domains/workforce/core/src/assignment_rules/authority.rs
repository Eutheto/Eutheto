//! Original-source authority. No compiler candidate graph, constraints or objective terms enter here.

use super::{
    AssignmentRuleError, AssignmentRuleLimit, UnsupportedWorkforceObligation,
    boundary::{DOMAIN_LIMITS, Measured, preflight},
    budget::{MAX_SELECTED_PAIRS, OperationBudget, count, within},
    evaluation::{CHECKED, VIOLATIONS, evaluate_validated_selection},
    identity::{RANK_CATEGORY, RANK_LEVEL, WORKFORCE_PROJECTION_VERSION},
    input::AssignmentInput,
    preference::{self, PreferencePlan, PreferenceScore},
    preference_consecutive, profile,
    projection::decode_workforce_assignment,
};
use crate::{
    assignment_rules::AssignmentRuleEvaluation,
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{AssignmentPair, AvailabilityKind, LockState, WorkforceEntity, WorkforceRule},
};
use eutheto_domain_api::{ContractJsonLimits, DiagnosticVerificationScope, DomainPackError};
use eutheto_domain_ir::{
    AcceptedResult, MetricId, MetricValue, NormalizedSolution, OptimizationDirection,
    RequiredRuleBinding, RuleEvaluation, RunObjectiveContext, ScoreCategoryId, ScoreLevelId,
    ScoreLevelValue, ScoreVector, VerificationContextV1, VerificationFactId, VerificationReport,
    VerificationScope, VerificationValue,
};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{
    EntityId, OperationControl, REVISION_MAX_V1, RuleId, ScenarioDocument, ScenarioId,
};
use std::collections::BTreeMap;

pub(crate) fn check_objective_context(
    objective_context: Option<&RunObjectiveContext>,
    scenario_id: ScenarioId,
) -> Result<(), DomainPackError> {
    profile::ProfileKind::parse(objective_context, scenario_id).map(|_| ())
}

fn invalid_objective_context(message: &'static str) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: "objectiveContext".to_owned(),
        message: message.to_owned(),
    }
}

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

/// Classifies relaxable authored rules from original source, never compiler provenance.
pub(crate) fn workforce_diagnostic_verification_scope(
    document: &ScenarioDocument,
    scenario_revision: u64,
    control: &OperationControl,
) -> Result<DiagnosticVerificationScope, DomainPackError> {
    let mut budget = OperationBudget::evaluation(Some(control));
    budget.check().map_err(|error| operation_error(&error))?;
    check_revision(scenario_revision)?;
    let input = prepare_source(document, &mut budget)?;
    let obligations = original_obligations(&input, &mut budget)?;
    let mut authored = Vec::new();
    for &(rule_id, kind) in &obligations {
        budget.step().map_err(|error| operation_error(&error))?;
        if kind == "required_rule" {
            budget
                .reserve(1, 1, 16)
                .map_err(|error| operation_error(&error))?;
            authored.push(rule_id);
        }
    }
    let scope = make_scope(
        document.scenario_id,
        scenario_revision,
        &input.source_document_hash,
        &obligations,
        &mut budget,
    )?;
    // Constructor membership checks perform one bounded binary search per authored rule.
    let searches = authored
        .len()
        .checked_mul(obligations.len().checked_ilog2().unwrap_or(0) as usize + 1)
        .ok_or_else(|| contract("arithmetic"))?;
    budget
        .steps(count(searches).map_err(|error| operation_error(&error))?)
        .map_err(|error| operation_error(&error))?;
    let classified = DiagnosticVerificationScope::new(scope, authored)?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(classified)
}

/// Computes feasibility and the initial source-derived score: supported preference levels precede
/// the minimized assignment rank tie-break. Inactive or unsupported-later preference definitions
/// retain rank-only compatibility when no active supported preference exists.
///
/// # Errors
/// Rejects malformed true or false decisions, unsupported policies and bounded-operation failures.
pub fn score_workforce_solution(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    objective_context: Option<&RunObjectiveContext>,
    control: Option<&OperationControl>,
) -> Result<ScoreVector, DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    Ok(assess(document, solution, objective_context, &mut budget)?
        .0
        .score)
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
    objective_context: Option<&RunObjectiveContext>,
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
    let (assessment, input) = assess(document, solution, objective_context, &mut budget)?;
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
    finish(
        assessment,
        &input.source_document_hash,
        solution,
        context,
        &mut budget,
    )
}

/// Public checksums establish consistency, not truth. Share re-evaluates the complete source.
pub(super) fn revalidate_workforce_accepted(
    document: &ScenarioDocument,
    accepted: &AcceptedResult,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<AssignmentInput, DomainPackError> {
    revalidate_workforce_accepted_with_outcomes(document, accepted, objective_context, budget, None)
}

pub(super) fn revalidate_workforce_accepted_with_outcomes(
    document: &ScenarioDocument,
    accepted: &AcceptedResult,
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
    outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<AssignmentInput, DomainPackError> {
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
    let (assessment, input) = assess_prepared(
        document,
        &accepted.solution,
        objective_context,
        input,
        solution_size,
        budget,
        outcomes,
    )?;
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
    let fresh = finish(
        assessment,
        &input.source_document_hash,
        &accepted.solution,
        &context,
        budget,
    )?;
    if !fresh.accepted || fresh.score.feasibility != 0 || fresh != *supplied {
        return Err(contract("accepted_result_mismatch"));
    }
    Ok(input)
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
    // Rank-only persisted results have one level; supported preference results have the
    // policy's ordered levels followed by rank. The generated score is checked exactly below.
    if score.levels.is_empty() {
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

/// Evaluates every original Required/fixed-fact obligation for a complete proposed selection.
///
/// Successful evaluation proves complete coverage, not feasibility or accepted-result authority.
/// Native admission must reject any returned unsatisfied outcome. Preferences and Soft scoring
/// do not participate; old distance data is not re-admitted by this function during replay.
///
/// # Errors
/// Rejects malformed selections, unsupported genuine Required obligations, incomplete outcomes
/// and existing cancellation/deadline/finite-work bounds, without returning a partial report.
pub fn evaluate_workforce_required_selection(
    document: &ScenarioDocument,
    selected_pairs: &[AssignmentPair],
    control: Option<&OperationControl>,
) -> Result<AssignmentRuleEvaluation, DomainPackError> {
    evaluate_required_with_budget(
        document,
        selected_pairs,
        &mut OperationBudget::evaluation(control),
    )
}

pub(super) fn evaluate_required_with_budget(
    document: &ScenarioDocument,
    selected_pairs: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<AssignmentRuleEvaluation, DomainPackError> {
    preflight(selected_pairs, budget, ContractJsonLimits::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    let input = prepare_source(document, budget)?;
    input
        .validate_selection(selected_pairs, budget)
        .map_err(|error| operation_error(&error))?;
    let (obligations, _) = source_obligations(&input, budget)?;
    let (evaluations, partition) =
        evaluate_validated_selection(document, &input, selected_pairs, &document.settings, budget)
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
    evaluation_counts(&evaluations, budget)?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(AssignmentRuleEvaluation {
        source_document_hash: input.source_document_hash,
        evaluations,
        obligations: partition,
    })
}

struct Assessment {
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
    objective_context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<(Assessment, AssignmentInput), DomainPackError> {
    let solution_size =
        preflight(solution, budget, DOMAIN_LIMITS).map_err(|error| operation_error(&error))?;
    let input = prepare_source(document, budget)?;
    assess_prepared(
        document,
        solution,
        objective_context,
        input,
        solution_size,
        budget,
        None,
    )
}

#[allow(clippy::too_many_lines)]
fn assess_prepared(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    objective_context: Option<&RunObjectiveContext>,
    input: AssignmentInput,
    solution_size: Measured,
    budget: &mut OperationBudget<'_>,
    mut outcomes: Option<&mut super::alternative_outcomes::Collector>,
) -> Result<(Assessment, AssignmentInput), DomainPackError> {
    check_objective_context(objective_context, document.scenario_id)?;
    let input = input.with_objective_context(objective_context, document.scenario_id, budget)?;
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
    let (obligations, has_score_policy) = original_obligations_with_policy(&input, budget)?;
    check_objective_support(
        &input,
        document.scenario_id,
        objective_context,
        has_score_policy,
        budget,
    )?;
    let preference_plan = preference::prepare(
        document,
        &input,
        objective_context,
        PlanningIrLimitsV1::DEFAULT,
        budget,
    )
    .map_err(|error| operation_error(&error))?;
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
    budget
        .sort_work(selected.len())
        .map_err(|error| operation_error(&error))?;
    selected.sort_unstable();
    let baseline_pairs = if preference_plan.layout.changes(objective_context) {
        Some(
            profile::sorted_baseline_pairs(
                &input,
                document.scenario_id,
                objective_context,
                budget,
            )?
            .ok_or_else(|| contract("baseline_required"))?,
        )
    } else {
        profile::snapshot_pairs(&input, document.scenario_id, objective_context, budget)?;
        None
    };
    // Canonical assignment IDs and exact pair decoding establish uniqueness for both values.
    let (evaluations, partition) = super::evaluation::evaluate_validated_selection_with_outcomes(
        document,
        &input,
        &selected,
        &document.settings,
        budget,
        outcomes.as_deref_mut(),
    )
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
    let preference_score = if let Some(outcomes) = outcomes.as_deref_mut() {
        preference::score_with_outcomes(
            &input,
            &preference_plan,
            &selected,
            &document.settings,
            budget,
            Some(outcomes),
        )
    } else {
        preference::score(
            &input,
            &preference_plan,
            &selected,
            &document.settings,
            budget,
        )
    }
    .map_err(|error| operation_error(&error))?;
    let profile_score = profile::prefix_values(
        &preference_plan.layout,
        &input.domain,
        &selected,
        baseline_pairs.as_ref(),
        budget,
    )?;
    if let Some(outcomes) = outcomes {
        super::alternative_outcomes::collect_baseline_and_soft(
            &input,
            document,
            objective_context.ok_or_else(|| contract("objective_context"))?,
            &selected,
            outcomes,
            baseline_pairs.as_ref(),
            budget,
        )?;
    }
    let score = rank_score(
        violations,
        rank,
        profile_score,
        preference_score,
        &preference_plan,
        budget,
    )?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok((
        Assessment {
            solution_size,
            obligations,
            evaluations,
            score,
            selected: i64::try_from(selected.len()).map_err(|_| contract("arithmetic"))?,
            checked,
        },
        input,
    ))
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
pub(super) fn check_objective_support(
    input: &AssignmentInput,
    scenario_id: ScenarioId,
    objective_context: Option<&RunObjectiveContext>,
    has_score_policy: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    let kind = profile::ProfileKind::parse(objective_context, scenario_id)?;
    let mut active_preference = false;
    let mut unsupported_preference: Option<UnsupportedWorkforceObligation> = None;
    for preference in input.domain.preferences.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        let (id, active, ..) = preference.header();
        active_preference |= active;
        if active
            && !(matches!(
                preference,
                crate::model::WorkforcePreference::Time { .. }
                    | crate::model::WorkforcePreference::AssignmentType { .. }
                    | crate::model::WorkforcePreference::Location { .. }
                    | crate::model::WorkforcePreference::RequestedTimeOff { .. }
                    | crate::model::WorkforcePreference::AssignmentTarget { .. }
                    | crate::model::WorkforcePreference::WeekendTarget { .. }
                    | crate::model::WorkforcePreference::WorkloadBalance { .. }
                    | crate::model::WorkforcePreference::Adjacency { .. }
                    | crate::model::WorkforcePreference::BaseStability { .. }
                    | crate::model::WorkforcePreference::TogetherSeparate { .. }
                    | crate::model::WorkforcePreference::PreferredCoverage { .. }
            ) || matches!(
                preference,
                crate::model::WorkforcePreference::ConsecutiveWork { mode, .. }
                    if preference_consecutive::supports(mode)
            ))
        {
            let value = UnsupportedWorkforceObligation::Preference(id);
            unsupported_preference =
                Some(unsupported_preference.map_or(value, |prior| prior.min(value)));
        }
    }
    if !has_score_policy && (!kind.is_some_and(profile::ProfileKind::named) || active_preference) {
        return Err(operation_error(&AssignmentRuleError::MissingScorePolicy));
    }
    if let Some(value) = unsupported_preference {
        return Err(operation_error(
            &AssignmentRuleError::UnsupportedObligation(value),
        ));
    }
    for lock in input.domain.locked_assignments.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        if matches!(lock.state, LockState::Soft { .. })
            && !profile::supports_soft(objective_context)
        {
            if objective_context.is_some() {
                return Err(invalid_objective_context(
                    "Soft locks require a named objective profile; Authored cannot score them",
                ));
            }
            return Err(operation_error(
                &AssignmentRuleError::UnsupportedObligation(
                    UnsupportedWorkforceObligation::SoftLock(lock.id),
                ),
            ));
        }
    }
    Ok(())
}

pub(super) fn original_obligations(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(RuleId, &'static str)>, DomainPackError> {
    Ok(original_obligations_with_policy(input, budget)?.0)
}

#[allow(clippy::type_complexity)] // The bool reports whether a score policy was present.
pub(super) fn original_obligations_with_policy(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<(Vec<(RuleId, &'static str)>, bool), DomainPackError> {
    source_obligations(input, budget)
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::type_complexity)] // The bool reports whether a score policy was present.
fn source_obligations(
    input: &AssignmentInput,
    budget: &mut OperationBudget<'_>,
) -> Result<(Vec<(RuleId, &'static str)>, bool), DomainPackError> {
    let mut obligations = Vec::new();
    let mut has_score_policy = false;
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
                has_score_policy = true;
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
                | WorkforceRule::MaximumAssignmentCount { .. }
                | WorkforceRule::WorkloadQuota(_)
                | WorkforceRule::MaximumHours { .. }
                | WorkforceRule::MaximumConsecutive { .. }
                | WorkforceRule::MaximumWorkedWeekends { .. }
                | WorkforceRule::RequiredSkillMix { .. }
                | WorkforceRule::FixedAssignment { .. }
                | WorkforceRule::MutualAssignmentRestriction { .. }
                | WorkforceRule::TransitionTime { .. }
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
    for lock in input.domain.locked_assignments.values() {
        budget.step().map_err(|error| operation_error(&error))?;
        match lock.state {
            LockState::Hard {} => {
                budget
                    .reserve(1, 2, 48)
                    .map_err(|error| operation_error(&error))?;
                obligations.push((RuleId::from_uuid(lock.id.as_uuid()), "hard_lock"));
            }
            LockState::Soft { .. } | LockState::Unlocked {} => {}
        }
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
    Ok((obligations, has_score_policy))
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

#[allow(clippy::too_many_lines)]
fn rank_score(
    feasibility: i64,
    rank: i64,
    profile_score: Vec<i64>,
    preference_score: Option<PreferenceScore>,
    preference_plan: &PreferencePlan<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<ScoreVector, DomainPackError> {
    if profile_score.len() != preference_plan.layout.prefix.len() {
        return Err(contract("score_shape"));
    }
    let level_count = preference_plan
        .layout
        .prefix
        .len()
        .checked_add(preference_plan.levels.len())
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| contract("arithmetic"))?;
    budget
        .reserve(
            count(level_count).map_err(|error| operation_error(&error))?,
            count(level_count)
                .map_err(|error| operation_error(&error))?
                .checked_mul(4)
                .ok_or_else(|| contract("arithmetic"))?,
            512,
        )
        .map_err(|error| operation_error(&error))?;
    let mut levels = Vec::with_capacity(level_count);
    for (tier, value) in preference_plan.layout.prefix.iter().zip(profile_score) {
        let (id, category) = (tier.id(), tier.category());
        let mut category_breakdown = BTreeMap::new();
        if value != 0 {
            category_breakdown.insert(
                ScoreCategoryId::new(category).map_err(|_| contract("score_identity"))?,
                value,
            );
        }
        levels.push(ScoreLevelValue {
            level_id: ScoreLevelId::new(id).map_err(|_| contract("score_identity"))?,
            value,
            direction: OptimizationDirection::Minimize,
            category_breakdown,
        });
    }
    if let Some(preference_score) = preference_score {
        if preference_score.levels.len() != preference_plan.levels.len() {
            return Err(contract("score_shape"));
        }
        for (level, values) in preference_plan.levels.iter().zip(preference_score.levels) {
            let mut category_breakdown = BTreeMap::new();
            if values.time != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::TIME_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.time,
                );
            }
            if values.assignment_type != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::ASSIGNMENT_TYPE_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.assignment_type,
                );
            }
            if values.location != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::LOCATION_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.location,
                );
            }
            if values.requested_time_off != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::REQUESTED_TIME_OFF_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.requested_time_off,
                );
            }
            if values.assignment_target != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::ASSIGNMENT_TARGET_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.assignment_target,
                );
            }
            if values.weekend_target != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::WEEKEND_TARGET_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.weekend_target,
                );
            }
            if values.workload_balance != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::WORKLOAD_BALANCE_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.workload_balance,
                );
            }
            if values.adjacency != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::ADJACENCY_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.adjacency,
                );
            }
            if values.consecutive_work != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::CONSECUTIVE_WORK_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.consecutive_work,
                );
            }
            if values.base_stability != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::BASE_STABILITY_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.base_stability,
                );
            }
            if values.together_separate != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::TOGETHER_SEPARATE_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.together_separate,
                );
            }
            if values.preferred_coverage != 0 {
                category_breakdown.insert(
                    ScoreCategoryId::new(preference::PREFERRED_COVERAGE_CATEGORY)
                        .map_err(|_| contract("score_identity"))?,
                    values.preferred_coverage,
                );
            }
            levels.push(ScoreLevelValue {
                level_id: ScoreLevelId::new(level.id.clone())
                    .map_err(|_| contract("score_identity"))?,
                value: values.value,
                direction: OptimizationDirection::Minimize,
                category_breakdown,
            });
        }
    }
    let mut category_breakdown = BTreeMap::new();
    if rank != 0 {
        category_breakdown.insert(
            ScoreCategoryId::new(RANK_CATEGORY).map_err(|_| contract("score_identity"))?,
            rank,
        );
    }
    levels.push(ScoreLevelValue {
        level_id: ScoreLevelId::new(RANK_LEVEL).map_err(|_| contract("score_identity"))?,
        value: rank,
        direction: OptimizationDirection::Minimize,
        category_breakdown,
    });
    let score = ScoreVector {
        feasibility,
        levels,
    };
    score
        .validate_current_shape()
        .map_err(|_| contract("score"))?;
    Ok(score)
}

fn finish(
    assessment: Assessment,
    document_hash: &str,
    solution: &NormalizedSolution,
    context: &VerificationContextV1,
    budget: &mut OperationBudget<'_>,
) -> Result<VerificationReport, DomainPackError> {
    let scope = make_scope(
        solution.scenario_id,
        solution.scenario_revision,
        document_hash,
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
        || context.document_hash != document_hash
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
        AssignmentRuleError::LimitExceeded(_)
        | AssignmentRuleError::InvalidDocument(DomainPackError::ResourceLimitExceeded) => {
            DomainPackError::ResourceLimitExceeded
        }
        _ => DomainPackError::Contract(error.code().to_owned()),
    }
}
pub(super) fn contract(code: &'static str) -> DomainPackError {
    DomainPackError::Contract(format!("official.workforce.authority.{code}"))
}
