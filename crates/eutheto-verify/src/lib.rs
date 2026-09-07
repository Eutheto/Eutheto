//! Independent acceptance policy over solver-neutral contracts.
//!
//! Backend candidates are untrusted. This crate projects them through a compiled-in domain
//! pack, validates the projected shape against the planning IR, checks complete required-rule
//! coverage and authoritative score bindings, and only then constructs an accepted result.

use eutheto_domain_api::{DomainPack, DomainPackError};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, DomainAssignmentId, NormalizedSolution, ScoreVector,
    VerificationContextV1, VerificationReport, VerificationScope, blake3_hex,
};
use eutheto_planning_ir::{
    CandidateValues, PlanningIrLimitsV1, PlanningProblem, ProjectionExpression, Variable,
    canonical_ir_hash, project_candidate, validate,
};
use eutheto_solver_api::{BackendCandidate, BackendObjectiveEvidence};
use eutheto_types::{
    DurationMillis, OperationControl, OperationInterruption, REVISION_MAX_V1, ScenarioDocument,
    SolutionId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// Stable category for a candidate that cannot cross the acceptance boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrectnessAlarmCategory {
    InvalidPlanningProblem,
    ProjectionFailed,
    StructuralValidationFailed,
    VerificationScopeFailed,
    ScoreRecomputationFailed,
    RequiredRuleVerificationFailed,
    ReportBindingFailed,
    RequiredRuleCoverageFailed,
    RequiredRuleRejected,
    ScoreIntegrityFailed,
    ClockFailed,
}

impl CorrectnessAlarmCategory {
    /// Stable safe diagnostic code suitable for persistence and API mapping.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidPlanningProblem => "verification.planning_problem_invalid",
            Self::ProjectionFailed => "verification.projection_failed",
            Self::StructuralValidationFailed => "verification.structure_invalid",
            Self::VerificationScopeFailed => "verification.scope_invalid",
            Self::ScoreRecomputationFailed => "verification.score_failed",
            Self::RequiredRuleVerificationFailed => "verification.required_rules_failed",
            Self::ReportBindingFailed => "verification.report_binding_invalid",
            Self::RequiredRuleCoverageFailed => "verification.required_rule_coverage_invalid",
            Self::RequiredRuleRejected => "verification.required_rule_rejected",
            Self::ScoreIntegrityFailed => "verification.score_integrity_failed",
            Self::ClockFailed => "verification.clock_invalid",
        }
    }
}

/// Safe bounded correctness alarm. Raw candidate/backend details belong in quarantined diagnostics.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrectnessAlarm {
    pub category: CorrectnessAlarmCategory,
    pub diagnostic_code: String,
}

impl CorrectnessAlarm {
    fn new(category: CorrectnessAlarmCategory) -> Self {
        Self {
            category,
            diagnostic_code: category.code().to_owned(),
        }
    }
}

/// Parent-observed acceptance-stage durations. Backend incumbent timing is deliberately absent.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptancePhaseTimings {
    pub projection_milliseconds: DurationMillis,
    pub structural_validation_milliseconds: DurationMillis,
    pub score_recomputation_milliseconds: DurationMillis,
    pub required_rule_verification_milliseconds: DurationMillis,
}

/// Successful structural validation bound to one canonical normalized-solution hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StructuralValidationReport {
    pub assignment_count: u32,
    pub normalized_solution_hash: String,
}

/// Stable structural failure without unbounded backend or domain text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralValidationFailure {
    pub code: &'static str,
    pub assignment_id: Option<DomainAssignmentId>,
}

impl StructuralValidationFailure {
    const fn new(code: &'static str, assignment_id: Option<DomainAssignmentId>) -> Self {
        Self {
            code,
            assignment_id,
        }
    }
}

/// Validates that a pack projection exactly matches the planning projection contract.
///
/// # Errors
/// Rejects invalid planning/solution contracts, stale bindings, assignment omissions/extras,
/// entity mismatches, requested solution-ID mismatches, or any value that differs from the
/// canonical projection of the backend candidate.
pub fn validate_structure(
    problem: &PlanningProblem,
    expected_model_hash: &str,
    candidate: &CandidateValues,
    expected_solution_id: SolutionId,
    solution: &NormalizedSolution,
) -> Result<StructuralValidationReport, StructuralValidationFailure> {
    validate(problem, PlanningIrLimitsV1::DEFAULT)
        .map_err(|_| StructuralValidationFailure::new("planning_problem_invalid", None))?;
    let model_hash = canonical_ir_hash(problem, PlanningIrLimitsV1::DEFAULT)
        .map_err(|_| StructuralValidationFailure::new("planning_model_hash_failed", None))?;
    if model_hash != expected_model_hash {
        return Err(StructuralValidationFailure::new(
            "planning_model_hash_mismatch",
            None,
        ));
    }
    solution
        .validate()
        .map_err(|_| StructuralValidationFailure::new("normalized_solution_invalid", None))?;
    if solution.pack_id != problem.metadata.pack_id {
        return Err(StructuralValidationFailure::new("pack_mismatch", None));
    }
    if solution.scenario_id != problem.metadata.scenario_id {
        return Err(StructuralValidationFailure::new("scenario_mismatch", None));
    }
    if solution.scenario_revision != problem.metadata.scenario_revision {
        return Err(StructuralValidationFailure::new("revision_mismatch", None));
    }
    if solution.projection_version != problem.metadata.projection_version {
        return Err(StructuralValidationFailure::new(
            "projection_version_mismatch",
            None,
        ));
    }
    if solution.solution_id != expected_solution_id {
        return Err(StructuralValidationFailure::new(
            "solution_id_mismatch",
            None,
        ));
    }

    let projections = problem
        .projections
        .iter()
        .map(|projection| (&projection.assignment_id, projection))
        .collect::<BTreeMap<_, _>>();
    if projections.len() != problem.projections.len() {
        return Err(StructuralValidationFailure::new(
            "duplicate_projection_assignment",
            None,
        ));
    }
    let assignments = solution
        .assignments
        .iter()
        .map(|assignment| (&assignment.id, assignment))
        .collect::<BTreeMap<_, _>>();
    if assignments.len() != projections.len() {
        return Err(StructuralValidationFailure::new(
            "assignment_set_mismatch",
            None,
        ));
    }
    for (assignment_id, projection) in projections {
        let Some(assignment) = assignments.get(assignment_id) else {
            return Err(StructuralValidationFailure::new(
                "missing_assignment",
                Some(assignment_id.clone()),
            ));
        };
        if assignment.entity != projection.entity {
            return Err(StructuralValidationFailure::new(
                "assignment_entity_mismatch",
                Some(assignment_id.clone()),
            ));
        }
        if !projected_value_matches(problem, projection, &assignment.value) {
            return Err(StructuralValidationFailure::new(
                "assignment_value_kind_mismatch",
                Some(assignment_id.clone()),
            ));
        }
    }
    validate_exact_projection(problem, candidate, expected_solution_id, solution)?;
    let assignment_count = u32::try_from(solution.assignments.len())
        .map_err(|_| StructuralValidationFailure::new("assignment_count_overflow", None))?;
    let normalized_solution_hash = solution
        .canonical_hash()
        .map_err(|_| StructuralValidationFailure::new("solution_hash_failed", None))?;
    Ok(StructuralValidationReport {
        assignment_count,
        normalized_solution_hash,
    })
}

fn validate_exact_projection(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    expected_solution_id: SolutionId,
    solution: &NormalizedSolution,
) -> Result<(), StructuralValidationFailure> {
    let expected = project_candidate(
        problem,
        candidate,
        expected_solution_id,
        PlanningIrLimitsV1::DEFAULT,
    )
    .map_err(|_| StructuralValidationFailure::new("candidate_projection_invalid", None))?;
    if solution
        .assignments
        .iter()
        .zip(&expected.assignments)
        .any(|(observed, expected)| {
            observed.id != expected.id
                || observed.entity != expected.entity
                || observed.value != expected.value
        })
    {
        return Err(StructuralValidationFailure::new(
            "projected_assignment_mismatch",
            None,
        ));
    }
    Ok(())
}

fn projected_value_matches(
    problem: &PlanningProblem,
    projection: &eutheto_planning_ir::SolutionProjection,
    value: &AssignmentValue,
) -> bool {
    if matches!(value, AssignmentValue::Absent) {
        return !projection.required
            || match &projection.expression {
                ProjectionExpression::Interval(id) => problem.variables.iter().any(|variable| {
                    matches!(variable, Variable::Interval(interval) if &interval.id == id && interval.presence.is_some())
                }),
                ProjectionExpression::Constant(AssignmentValue::Absent) => true,
                _ => false,
            };
    }
    match (&projection.expression, value) {
        (ProjectionExpression::Boolean(_), AssignmentValue::Boolean(_))
        | (
            ProjectionExpression::Integer(_) | ProjectionExpression::Linear(_),
            AssignmentValue::Integer(_),
        )
        | (ProjectionExpression::Interval(_), AssignmentValue::Interval(_)) => true,
        (ProjectionExpression::Constant(expected), observed) => expected == observed,
        _ => false,
    }
}

/// Non-authoritative backend objective reconciliation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendObjectiveReconciliation {
    Matched,
    Missing,
    Mismatch,
}

/// Complete decision for one backend candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcceptanceDecision {
    Awaiting,
    Accepted {
        result: Box<AcceptedResult>,
        objective_reconciliation: BackendObjectiveReconciliation,
        timings: AcceptancePhaseTimings,
    },
    Quarantined {
        alarm: CorrectnessAlarm,
        timings: AcceptancePhaseTimings,
    },
    Interrupted {
        reason: OperationInterruption,
        timings: AcceptancePhaseTimings,
    },
}

/// Injectable monotonic clock for deterministic acceptance timing tests.
pub trait VerificationClock: Send + Sync {
    fn now_milliseconds(&self) -> DurationMillis;
}

/// Process-local monotonic clock used by production acceptance orchestration.
pub struct SystemVerificationClock {
    started: Instant,
}

impl Default for SystemVerificationClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl VerificationClock for SystemVerificationClock {
    fn now_milliseconds(&self) -> DurationMillis {
        let elapsed = self.started.elapsed().as_millis();
        let bounded = elapsed.min(u128::from(DurationMillis::MAX.value()));
        match u64::try_from(bounded)
            .ok()
            .and_then(|value| DurationMillis::new(value).ok())
        {
            Some(value) => value,
            None => DurationMillis::MAX,
        }
    }
}

/// Router-agnostic acceptance service for one immutable scenario/planning problem.
pub struct AcceptanceReviewer<'a> {
    pack: &'a dyn DomainPack,
    document: &'a ScenarioDocument,
    scenario_revision: u64,
    problem: &'a PlanningProblem,
    document_hash: String,
    planning_model_hash: String,
    clock: &'a dyn VerificationClock,
}

impl<'a> AcceptanceReviewer<'a> {
    /// Creates an acceptance reviewer after validating immutable scenario/model bindings.
    ///
    /// # Errors
    /// Rejects invalid planning IR or a pack/scenario/revision mismatch before any candidate work.
    pub fn new(
        pack: &'a dyn DomainPack,
        document: &'a ScenarioDocument,
        scenario_revision: u64,
        problem: &'a PlanningProblem,
        clock: &'a dyn VerificationClock,
    ) -> Result<Self, CorrectnessAlarm> {
        validate(problem, PlanningIrLimitsV1::DEFAULT)
            .map_err(|_| CorrectnessAlarm::new(CorrectnessAlarmCategory::InvalidPlanningProblem))?;
        if scenario_revision > REVISION_MAX_V1
            || problem.metadata.pack_id != document.domain_pack.id
            || problem.metadata.scenario_id != document.scenario_id
            || problem.metadata.scenario_revision != scenario_revision
        {
            return Err(CorrectnessAlarm::new(
                CorrectnessAlarmCategory::InvalidPlanningProblem,
            ));
        }
        let planning_model_hash = canonical_ir_hash(problem, PlanningIrLimitsV1::DEFAULT)
            .map_err(|_| CorrectnessAlarm::new(CorrectnessAlarmCategory::InvalidPlanningProblem))?;
        let document_bytes = serde_json::to_vec(document)
            .map_err(|_| CorrectnessAlarm::new(CorrectnessAlarmCategory::InvalidPlanningProblem))?;
        Ok(Self {
            pack,
            document,
            scenario_revision,
            problem,
            document_hash: blake3_hex(&document_bytes),
            planning_model_hash,
            clock,
        })
    }

    /// Projects, structurally validates, scores once, verifies required rules, and decides whether
    /// one untrusted backend candidate may cross the acceptance boundary.
    #[must_use]
    // Keeping the ordered trust-boundary stages together makes timing and early-return behavior
    // auditable; extracting them would obscure which failures can cross each gate.
    #[allow(clippy::too_many_lines)]
    pub fn review(
        &self,
        candidate: &BackendCandidate,
        solution_id: SolutionId,
        control: &OperationControl,
    ) -> AcceptanceDecision {
        let mut timings = AcceptancePhaseTimings::default();
        if let Some(reason) = interrupted(control, None) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }

        let started = self.clock.now_milliseconds();
        let projection = self
            .pack
            .project(self.problem, &candidate.values, solution_id, control);
        let finished = self.clock.now_milliseconds();
        let projection_elapsed = elapsed(started, finished);
        if let Some(duration) = projection_elapsed {
            timings.projection_milliseconds = duration;
        }
        if let Some(reason) = interrupted(control, projection.as_ref().err()) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if projection_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        let Ok(solution) = projection else {
            return quarantine(CorrectnessAlarmCategory::ProjectionFailed, timings);
        };

        let started = finished;
        let structural = validate_structure(
            self.problem,
            &self.planning_model_hash,
            &candidate.values,
            solution_id,
            &solution,
        );
        let finished = self.clock.now_milliseconds();
        let structural_elapsed = elapsed(started, finished);
        if let Some(duration) = structural_elapsed {
            timings.structural_validation_milliseconds = duration;
        }
        if let Some(reason) = interrupted(control, None) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if structural_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        let Ok(structural) = structural else {
            return quarantine(
                CorrectnessAlarmCategory::StructuralValidationFailed,
                timings,
            );
        };

        let started = finished;
        let authoritative_score = self.pack.score(self.document, &solution, control);
        let score_finished = self.clock.now_milliseconds();
        let score_elapsed = elapsed(started, score_finished);
        if let Some(duration) = score_elapsed {
            timings.score_recomputation_milliseconds = duration;
        }
        if let Some(reason) = interrupted(control, authoritative_score.as_ref().err()) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if score_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        let Ok(authoritative_score) = authoritative_score else {
            return quarantine(CorrectnessAlarmCategory::ScoreRecomputationFailed, timings);
        };

        let verification_started = score_finished;
        let verification_scope =
            self.pack
                .verification_scope(self.document, self.scenario_revision, control);
        if let Some(reason) = interrupted(control, verification_scope.as_ref().err()) {
            if let Some(duration) = elapsed(verification_started, self.clock.now_milliseconds()) {
                timings.required_rule_verification_milliseconds = duration;
            }
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        let Ok(verification_scope) = verification_scope else {
            let failed = self.clock.now_milliseconds();
            let Some(verification_elapsed) = elapsed(verification_started, failed) else {
                return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
            };
            timings.required_rule_verification_milliseconds = verification_elapsed;
            return quarantine(CorrectnessAlarmCategory::VerificationScopeFailed, timings);
        };

        let context = VerificationContextV1::new(
            self.document.scenario_id,
            self.scenario_revision,
            self.document_hash.clone(),
            self.planning_model_hash.clone(),
            structural.normalized_solution_hash.clone(),
            verification_scope.checksum.clone(),
        );
        let Ok(context) = context else {
            let failed = self.clock.now_milliseconds();
            let Some(verification_elapsed) = elapsed(verification_started, failed) else {
                return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
            };
            timings.required_rule_verification_milliseconds = verification_elapsed;
            return quarantine(CorrectnessAlarmCategory::ReportBindingFailed, timings);
        };

        let report = self.pack.verify(
            self.document,
            &solution,
            &context,
            &authoritative_score,
            control,
        );
        if let Some(reason) = interrupted(control, report.as_ref().err()) {
            if let Some(duration) = elapsed(verification_started, self.clock.now_milliseconds()) {
                timings.required_rule_verification_milliseconds = duration;
            }
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        let Ok(report) = report else {
            let failed = self.clock.now_milliseconds();
            let Some(verification_elapsed) = elapsed(verification_started, failed) else {
                return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
            };
            timings.required_rule_verification_milliseconds = verification_elapsed;
            return quarantine(
                CorrectnessAlarmCategory::RequiredRuleVerificationFailed,
                timings,
            );
        };
        let validation = validate_report(
            self.problem,
            &verification_scope,
            &context,
            &report,
            &authoritative_score,
            candidate.objective.as_ref(),
        );
        let verify_finished = self.clock.now_milliseconds();
        let verify_elapsed = elapsed(verification_started, verify_finished);
        if let Some(duration) = verify_elapsed {
            timings.required_rule_verification_milliseconds = duration;
        }
        if let Some(reason) = interrupted(control, None) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if verify_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if let Some(category) = validation.as_ref().err().copied() {
            return quarantine(category, timings);
        }
        let Some(objective_reconciliation) = validation.ok() else {
            return quarantine(CorrectnessAlarmCategory::ReportBindingFailed, timings);
        };
        let accepted = AcceptedResult::new(solution, report);
        if let Some(reason) = interrupted(control, None) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        match accepted {
            Ok(result) => AcceptanceDecision::Accepted {
                result: Box::new(result),
                objective_reconciliation,
                timings,
            },
            Err(_) => quarantine(CorrectnessAlarmCategory::ReportBindingFailed, timings),
        }
    }
}

fn interrupted(
    control: &OperationControl,
    error: Option<&DomainPackError>,
) -> Option<OperationInterruption> {
    control.check().err().or(match error {
        Some(DomainPackError::Cancelled) => Some(OperationInterruption::Cancelled),
        Some(DomainPackError::BudgetExpired) => Some(OperationInterruption::DeadlineExceeded),
        _ => None,
    })
}

fn validate_report(
    problem: &PlanningProblem,
    verification_scope: &VerificationScope,
    context: &VerificationContextV1,
    report: &VerificationReport,
    authoritative_score: &ScoreVector,
    backend_objective: Option<&BackendObjectiveEvidence>,
) -> Result<BackendObjectiveReconciliation, CorrectnessAlarmCategory> {
    if &report.score != authoritative_score {
        return Err(CorrectnessAlarmCategory::ScoreIntegrityFailed);
    }
    verification_scope
        .validate()
        .map_err(|_| CorrectnessAlarmCategory::VerificationScopeFailed)?;
    context
        .validate()
        .map_err(|_| CorrectnessAlarmCategory::ReportBindingFailed)?;
    report
        .validate()
        .map_err(|_| CorrectnessAlarmCategory::ReportBindingFailed)?;
    if verification_scope.scenario_id != context.scenario_id
        || verification_scope.scenario_revision != context.evaluated_revision
        || verification_scope.checksum != context.verification_scope_checksum
        || report.scenario_id != context.scenario_id
        || report.evaluated_revision != context.evaluated_revision
        || report.document_hash != context.document_hash
        || report.planning_model_hash != context.planning_model_hash
        || report.normalized_solution_hash != context.normalized_solution_hash
        || report.verification_scope_checksum != context.verification_scope_checksum
    {
        return Err(CorrectnessAlarmCategory::ReportBindingFailed);
    }
    if verification_scope
        .required_rules
        .iter()
        .map(|binding| binding.rule_id)
        .ne(report
            .required_rule_results
            .iter()
            .map(|evaluation| evaluation.rule_id))
    {
        return Err(CorrectnessAlarmCategory::RequiredRuleCoverageFailed);
    }
    if !report.accepted
        || report
            .required_rule_results
            .iter()
            .any(|evaluation| !evaluation.satisfied)
    {
        return Err(CorrectnessAlarmCategory::RequiredRuleRejected);
    }
    if report.score.feasibility != 0 {
        return Err(CorrectnessAlarmCategory::ScoreIntegrityFailed);
    }
    validate_score(problem, &report.score)?;
    Ok(reconcile_backend_objective(
        &report.score,
        backend_objective,
    ))
}

fn validate_score(
    problem: &PlanningProblem,
    score: &ScoreVector,
) -> Result<(), CorrectnessAlarmCategory> {
    score
        .validate_current_shape()
        .map_err(|_| CorrectnessAlarmCategory::ScoreIntegrityFailed)?;
    if problem.objectives.levels.len() != score.levels.len() {
        return Err(CorrectnessAlarmCategory::ScoreIntegrityFailed);
    }
    for (objective, verified) in problem.objectives.levels.iter().zip(&score.levels) {
        let declared_categories = objective
            .terms
            .iter()
            .map(|term| term.category.as_str())
            .collect::<BTreeSet<_>>();
        if objective.id.as_str() != verified.level_id.as_str()
            || objective.direction != verified.direction
            || verified.value < objective.lower_bound
            || verified.value > objective.upper_bound
            || verified
                .category_breakdown
                .keys()
                .any(|category| !declared_categories.contains(category.as_str()))
        {
            return Err(CorrectnessAlarmCategory::ScoreIntegrityFailed);
        }
    }
    Ok(())
}

fn reconcile_backend_objective(
    score: &ScoreVector,
    backend_objective: Option<&BackendObjectiveEvidence>,
) -> BackendObjectiveReconciliation {
    let Some(backend_objective) = backend_objective else {
        return BackendObjectiveReconciliation::Missing;
    };
    if backend_objective.objective_values.len() != score.levels.len()
        || backend_objective
            .objective_values
            .iter()
            .zip(&score.levels)
            .any(|(backend, verified)| backend != &verified.value)
    {
        BackendObjectiveReconciliation::Mismatch
    } else {
        BackendObjectiveReconciliation::Matched
    }
}

fn elapsed(start: DurationMillis, end: DurationMillis) -> Option<DurationMillis> {
    end.value()
        .checked_sub(start.value())
        .and_then(|value| DurationMillis::new(value).ok())
}

fn quarantine(
    category: CorrectnessAlarmCategory,
    timings: AcceptancePhaseTimings,
) -> AcceptanceDecision {
    AcceptanceDecision::Quarantined {
        alarm: CorrectnessAlarm::new(category),
        timings,
    }
}

#[cfg(test)]
mod tests;
