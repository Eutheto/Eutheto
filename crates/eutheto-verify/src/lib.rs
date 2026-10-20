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
    CandidateValues, PlanningIrLimitsV1, PlanningPreparationBudget, PlanningPreparationError,
    PlanningProblem, PreparedPlanningModel, ProjectionExpression, Variable, canonical_ir_hash,
    project_candidate, project_candidate_admitted, validate,
};
use eutheto_solver_api::{BackendCandidate, BackendObjectiveEvidence};
use eutheto_types::{
    DurationMillis, MAX_SCENARIO_DOCUMENT_BYTES, OperationControl, OperationInterruption,
    REVISION_MAX_V1, ScenarioDocument, SolutionId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

mod diagnostic;
pub use diagnostic::DiagnosticDecision;

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
    /// A complete original-source witness contradicts locally observed infeasibility.
    InfeasibilityContradicted,
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
            Self::InfeasibilityContradicted => "verification.infeasibility_contradicted",
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
/// Failure from the controlled diagnostic structural path.
#[derive(Debug)]
pub(crate) enum DiagnosticStructuralFailure {
    Invalid,
    Preparation(PlanningPreparationError),
}

/// Validates a projected diagnostic witness against an already admitted immutable model.
///
/// This deliberately reuses the authenticated model hash and skips whole-IR validation and
/// canonical hashing. All maps, projection output, and normalized-solution hashing are admitted
/// against the inherited preparation budget before they are allocated.
// Keep ordered structural gates and their inherited admission in one auditable scope.
#[allow(clippy::too_many_lines)]
pub(crate) fn validate_structure_diagnostic(
    problem: &PlanningProblem,
    expected_model_hash: &str,
    candidate: &CandidateValues,
    expected_solution_id: SolutionId,
    solution: &NormalizedSolution,
    budget: &mut PlanningPreparationBudget,
    control: &OperationControl,
) -> Result<StructuralValidationReport, DiagnosticStructuralFailure> {
    budget
        .check()
        .map_err(DiagnosticStructuralFailure::Preparation)?;
    control.check().map_err(|reason| {
        DiagnosticStructuralFailure::Preparation(match reason {
            OperationInterruption::Cancelled => PlanningPreparationError::Cancelled,
            OperationInterruption::DeadlineExceeded => PlanningPreparationError::DeadlineExceeded,
        })
    })?;
    if expected_model_hash.is_empty() {
        return Err(DiagnosticStructuralFailure::Invalid);
    }
    let units = u64::try_from(problem.variables.len())
        .unwrap_or(u64::MAX)
        .checked_add(u64::try_from(problem.projections.len()).unwrap_or(u64::MAX))
        .and_then(|value| {
            value.checked_add(u64::try_from(solution.assignments.len()).unwrap_or(u64::MAX))
        })
        .and_then(|value| {
            value.checked_add(
                u64::try_from(candidate.booleans.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(candidate.integers.len()).unwrap_or(u64::MAX)),
            )
        })
        .ok_or(DiagnosticStructuralFailure::Preparation(
            PlanningPreparationError::ArithmeticOverflow,
        ))?;
    let scratch = units
        .checked_mul(256)
        .and_then(|value| value.checked_add(8192))
        .ok_or(DiagnosticStructuralFailure::Preparation(
            PlanningPreparationError::ArithmeticOverflow,
        ))?;
    budget
        .reserve_work(units.saturating_mul(4))
        .map_err(DiagnosticStructuralFailure::Preparation)?;
    budget
        .reserve_transient(scratch)
        .map_err(DiagnosticStructuralFailure::Preparation)?;
    let result = (|| {
        solution
            .validate()
            .map_err(|_| DiagnosticStructuralFailure::Invalid)?;
        if solution.pack_id != problem.metadata.pack_id {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        if solution.scenario_id != problem.metadata.scenario_id {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        if solution.scenario_revision != problem.metadata.scenario_revision {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        if solution.projection_version != problem.metadata.projection_version {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        if solution.solution_id != expected_solution_id {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        let projections = problem
            .projections
            .iter()
            .map(|projection| (&projection.assignment_id, projection))
            .collect::<BTreeMap<_, _>>();
        if projections.len() != problem.projections.len() {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        let assignments = solution
            .assignments
            .iter()
            .map(|assignment| (&assignment.id, assignment))
            .collect::<BTreeMap<_, _>>();
        if assignments.len() != projections.len() {
            return Err(DiagnosticStructuralFailure::Invalid);
        }
        for (assignment_id, projection) in projections {
            budget
                .check()
                .map_err(DiagnosticStructuralFailure::Preparation)?;
            let Some(assignment) = assignments.get(assignment_id) else {
                return Err(DiagnosticStructuralFailure::Invalid);
            };
            if assignment.entity != projection.entity {
                return Err(DiagnosticStructuralFailure::Invalid);
            }
            if !projected_value_matches(problem, projection, &assignment.value) {
                return Err(DiagnosticStructuralFailure::Invalid);
            }
        }
        validate_exact_projection_admitted(problem, candidate, expected_solution_id, solution)?;
        let assignment_count = u32::try_from(solution.assignments.len())
            .map_err(|_| DiagnosticStructuralFailure::Invalid)?;
        let normalized_solution_hash = solution
            .canonical_hash()
            .map_err(|_| DiagnosticStructuralFailure::Invalid)?;
        Ok(StructuralValidationReport {
            assignment_count,
            normalized_solution_hash,
        })
    })();
    let release = budget.release_transient(scratch);
    match (result, release) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(error), Ok(())) => Err(error),
        (_, Err(error)) => Err(DiagnosticStructuralFailure::Preparation(error)),
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
fn validate_exact_projection_admitted(
    problem: &PlanningProblem,
    candidate: &CandidateValues,
    expected_solution_id: SolutionId,
    solution: &NormalizedSolution,
) -> Result<(), DiagnosticStructuralFailure> {
    let expected = project_candidate_admitted(
        problem,
        candidate,
        expected_solution_id,
        PlanningIrLimitsV1::DEFAULT,
    )
    .map_err(|_| DiagnosticStructuralFailure::Invalid)?;
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
        return Err(DiagnosticStructuralFailure::Invalid);
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
    /// A finite allowance prevented completing review; no validity claim or alarm is made.
    ResourceLimitExceeded {
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

/// Failure while admitting a diagnostic reviewer against the inherited preparation phase.
#[derive(Debug)]
pub enum DiagnosticReviewerError {
    /// The admitted immutable model/source binding is inconsistent.
    Alarm(CorrectnessAlarm),
    /// Cooperative admission/control stopped before review could begin.
    Preparation(PlanningPreparationError),
}

/// Router-agnostic acceptance service for one immutable scenario/planning problem.
pub struct AcceptanceReviewer<'a> {
    pack: &'a dyn DomainPack,
    document: &'a ScenarioDocument,
    scenario_revision: u64,
    problem: &'a PlanningProblem,
    objective_context: Option<&'a eutheto_domain_ir::RunObjectiveContext>,
    document_hash: String,
    planning_model_hash: String,
    clock: &'a dyn VerificationClock,
    diagnostic_reservation: u64,
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
        objective_context: Option<&'a eutheto_domain_ir::RunObjectiveContext>,
        clock: &'a dyn VerificationClock,
    ) -> Result<Self, CorrectnessAlarm> {
        validate(problem, PlanningIrLimitsV1::DEFAULT)
            .map_err(|_| CorrectnessAlarm::new(CorrectnessAlarmCategory::InvalidPlanningProblem))?;
        if scenario_revision > REVISION_MAX_V1
            || problem.metadata.pack_id != document.domain_pack.id
            || problem.metadata.scenario_id != document.scenario_id
            || problem.metadata.scenario_revision != scenario_revision
            || objective_context.is_some_and(|recorded| {
                recorded
                    .validate_for(&document.domain_pack.id, document.scenario_id)
                    .is_err()
            })
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
            objective_context,
            document_hash: blake3_hex(&document_bytes),
            planning_model_hash,
            clock,
            diagnostic_reservation: 0,
        })
    }
    /// Creates a diagnostic reviewer from the opaque, already-admitted model artifact.
    ///
    /// Unlike [`Self::new`], this path never validates, normalizes, clones, or hashes the full
    /// planning IR. The prepared artifact is the authority for the immutable model hash. Source
    /// serialization and retained structural-review scratch are admitted before they are built.
    ///
    /// # Errors
    /// Rejects mismatched source identity, serialization failure, interruption or exhausted
    /// inherited preparation capacity.
    #[allow(clippy::too_many_arguments)]
    pub fn new_diagnostic(
        pack: &'a dyn DomainPack,
        document: &'a ScenarioDocument,
        scenario_revision: u64,
        prepared: &'a PreparedPlanningModel,
        objective_context: Option<&'a eutheto_domain_ir::RunObjectiveContext>,
        clock: &'a dyn VerificationClock,
        budget: &mut PlanningPreparationBudget,
        control: &OperationControl,
    ) -> Result<Self, DiagnosticReviewerError> {
        budget
            .check()
            .map_err(DiagnosticReviewerError::Preparation)?;
        let problem = prepared.problem().as_ref();
        if scenario_revision > REVISION_MAX_V1
            || problem.metadata.pack_id != document.domain_pack.id
            || problem.metadata.scenario_id != document.scenario_id
            || problem.metadata.scenario_revision != scenario_revision
            || objective_context.is_some_and(|recorded| {
                recorded
                    .validate_for(&document.domain_pack.id, document.scenario_id)
                    .is_err()
            })
        {
            return Err(DiagnosticReviewerError::Alarm(CorrectnessAlarm::new(
                CorrectnessAlarmCategory::InvalidPlanningProblem,
            )));
        }
        control.check().map_err(|reason| {
            DiagnosticReviewerError::Preparation(match reason {
                OperationInterruption::Cancelled => PlanningPreparationError::Cancelled,
                OperationInterruption::DeadlineExceeded => {
                    PlanningPreparationError::DeadlineExceeded
                }
            })
        })?;

        let encoded_bytes = budget
            .precharge_serialized(document, MAX_SCENARIO_DOCUMENT_BYTES)
            .map_err(DiagnosticReviewerError::Preparation)?;
        let source_scratch = PlanningPreparationBudget::serialized_scratch_bytes(encoded_bytes)
            .map_err(DiagnosticReviewerError::Preparation)?;
        budget
            .reserve_transient(source_scratch)
            .map_err(DiagnosticReviewerError::Preparation)?;
        let document_hash = if let Ok(bytes) = serde_json::to_vec(document) {
            blake3_hex(&bytes)
        } else {
            let _ = budget.release_transient(source_scratch);
            return Err(DiagnosticReviewerError::Alarm(CorrectnessAlarm::new(
                CorrectnessAlarmCategory::InvalidPlanningProblem,
            )));
        };
        budget
            .release_transient(source_scratch)
            .map_err(DiagnosticReviewerError::Preparation)?;

        let summary = prepared.summary();
        let units = summary
            .variable_count
            .checked_add(summary.constraint_count)
            .and_then(|value| value.checked_add(summary.assumption_count))
            .and_then(|value| value.checked_add(summary.projection_count))
            .and_then(|value| value.checked_add(summary.provenance_count))
            .ok_or(DiagnosticReviewerError::Preparation(
                PlanningPreparationError::ArithmeticOverflow,
            ))?;
        let diagnostic_reservation = units
            .checked_mul(192)
            .and_then(|value| value.checked_add(8192))
            .ok_or(DiagnosticReviewerError::Preparation(
                PlanningPreparationError::ArithmeticOverflow,
            ))?;
        budget
            .reserve_work(units.saturating_mul(2))
            .map_err(DiagnosticReviewerError::Preparation)?;
        if let Err(error) = budget.reserve_transient(diagnostic_reservation) {
            return Err(DiagnosticReviewerError::Preparation(error));
        }
        Ok(Self {
            pack,
            document,
            scenario_revision,
            problem,
            objective_context,
            document_hash,
            planning_model_hash: summary.canonical_ir_hash.clone(),
            clock,
            diagnostic_reservation,
        })
    }

    /// Releases the retained diagnostic-review scratch after all candidates are reviewed.
    ///
    /// # Errors
    /// Rejects retirement if the budget has insufficient matching raw reservation capacity.
    pub fn release_diagnostic(
        mut self,
        budget: &mut PlanningPreparationBudget,
    ) -> Result<(), PlanningPreparationError> {
        let reservation = self.diagnostic_reservation;
        self.diagnostic_reservation = 0;
        drop(self);
        budget.release_transient(reservation)
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
        let projection_failure =
            pack_failure(&projection, CorrectnessAlarmCategory::ProjectionFailed);
        let finished = self.clock.now_milliseconds();
        let projection_elapsed = elapsed(started, finished);
        if let Some(duration) = projection_elapsed {
            timings.projection_milliseconds = duration;
        }
        if let Some(category) = projection_failure {
            return quarantine(category, timings);
        }
        if let Some(reason) = interrupted(control, projection.as_ref().err()) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if projection_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if matches!(projection, Err(DomainPackError::ResourceLimitExceeded)) {
            return AcceptanceDecision::ResourceLimitExceeded { timings };
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
        let structural_failure = structural
            .as_ref()
            .err()
            .map(|_| CorrectnessAlarmCategory::StructuralValidationFailed);
        let finished = self.clock.now_milliseconds();
        let structural_elapsed = elapsed(started, finished);
        if let Some(duration) = structural_elapsed {
            timings.structural_validation_milliseconds = duration;
        }
        if let Some(category) = structural_failure {
            return quarantine(category, timings);
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
        let authoritative_score =
            self.pack
                .score(self.document, &solution, self.objective_context, control);
        let score_failure = pack_failure(
            &authoritative_score,
            CorrectnessAlarmCategory::ScoreRecomputationFailed,
        );
        let score_finished = self.clock.now_milliseconds();
        let score_elapsed = elapsed(started, score_finished);
        if let Some(duration) = score_elapsed {
            timings.score_recomputation_milliseconds = duration;
        }
        if let Some(category) = score_failure {
            return quarantine(category, timings);
        }
        if let Some(reason) = interrupted(control, authoritative_score.as_ref().err()) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if score_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
        }
        if matches!(
            authoritative_score,
            Err(DomainPackError::ResourceLimitExceeded)
        ) {
            return AcceptanceDecision::ResourceLimitExceeded { timings };
        }
        let Ok(authoritative_score) = authoritative_score else {
            return quarantine(CorrectnessAlarmCategory::ScoreRecomputationFailed, timings);
        };

        let verification_started = score_finished;
        let verification_scope =
            self.pack
                .verification_scope(self.document, self.scenario_revision, control);
        let classification_failure = pack_failure(
            &verification_scope,
            CorrectnessAlarmCategory::VerificationScopeFailed,
        );
        if let Some(category) = classification_failure {
            let failed = self.clock.now_milliseconds();
            if let Some(duration) = elapsed(verification_started, failed) {
                timings.required_rule_verification_milliseconds = duration;
            }
            return quarantine(category, timings);
        }
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
            if matches!(
                verification_scope,
                Err(DomainPackError::ResourceLimitExceeded)
            ) {
                return AcceptanceDecision::ResourceLimitExceeded { timings };
            }
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
            if let Some(duration) = elapsed(verification_started, failed) {
                timings.required_rule_verification_milliseconds = duration;
            }
            return quarantine(CorrectnessAlarmCategory::ReportBindingFailed, timings);
        };

        let report = self.pack.verify(
            self.document,
            &solution,
            self.objective_context,
            &context,
            &authoritative_score,
            control,
        );
        let report_failure = pack_failure(
            &report,
            CorrectnessAlarmCategory::RequiredRuleVerificationFailed,
        );
        if let Some(category) = report_failure {
            let failed = self.clock.now_milliseconds();
            if let Some(duration) = elapsed(verification_started, failed) {
                timings.required_rule_verification_milliseconds = duration;
            }
            return quarantine(category, timings);
        }
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
            if matches!(report, Err(DomainPackError::ResourceLimitExceeded)) {
                return AcceptanceDecision::ResourceLimitExceeded { timings };
            }
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
        let validation_failure = validation.as_ref().err().copied();
        let verify_finished = self.clock.now_milliseconds();
        let verify_elapsed = elapsed(verification_started, verify_finished);
        if let Some(duration) = verify_elapsed {
            timings.required_rule_verification_milliseconds = duration;
        }
        if let Some(category) = validation_failure {
            return quarantine(category, timings);
        }
        if let Some(reason) = interrupted(control, None) {
            return AcceptanceDecision::Interrupted { reason, timings };
        }
        if verify_elapsed.is_none() {
            return quarantine(CorrectnessAlarmCategory::ClockFailed, timings);
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

fn pack_failure<T>(
    result: &Result<T, DomainPackError>,
    category: CorrectnessAlarmCategory,
) -> Option<CorrectnessAlarmCategory> {
    let Err(error) = result else {
        return None;
    };
    if matches!(
        error,
        DomainPackError::Cancelled
            | DomainPackError::BudgetExpired
            | DomainPackError::ResourceLimitExceeded
    ) {
        None
    } else {
        Some(category)
    }
}

fn validate_report(
    problem: &PlanningProblem,
    verification_scope: &VerificationScope,
    context: &VerificationContextV1,
    report: &VerificationReport,
    authoritative_score: &ScoreVector,
    backend_objective: Option<&BackendObjectiveEvidence>,
) -> Result<BackendObjectiveReconciliation, CorrectnessAlarmCategory> {
    validate_report_integrity(verification_scope, context, report, authoritative_score)?;
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

/// Complete source bindings and ordered coverage shared by acceptance and unsaved diagnostics.
fn validate_report_integrity(
    verification_scope: &VerificationScope,
    context: &VerificationContextV1,
    report: &VerificationReport,
    authoritative_score: &ScoreVector,
) -> Result<(), CorrectnessAlarmCategory> {
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
    Ok(())
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
