use crate::{
    BACKEND_STATISTIC_MAX_V1, BackendAssumptionCoreEvidence, BackendCandidate, BackendSolveOutcome,
    BackendTerminationReason, CapabilityMatrix, CompatibilityReport, ModelCostEstimate,
    RequiredFeature, SolverApiLimits, SolverDescriptor, SupportFeatureId,
};
use eutheto_domain_ir::OptimizationDirection;
use eutheto_planning_ir::{
    Capability, LexicographicStrategy, PlanningIrLimitsV1, PlanningProblemSummary,
    summarize_with_budget,
};
use eutheto_types::{
    BackendSelection, ExplanationMode, ReproducibilityMode, SolveOptions, WorkerThreadPolicy,
};
use std::fmt;
use std::sync::Arc;

/// Derives every matrix requirement visible in the frozen planning summary and solve options.
///
/// # Errors
///
/// Returns [`PreflightError::InternalFeature`] if an internal capability or solve requirement
/// cannot be represented as a support-feature identifier.
pub fn compatibility_for(
    matrix: &CapabilityMatrix,
    backend: &eutheto_types::BackendId,
    summary: &PlanningProblemSummary,
    options: &SolveOptions,
) -> Result<CompatibilityReport, PreflightError> {
    let mut required = Vec::new();
    for (capability, count) in &summary.manifest.capability_counts {
        required.push(RequiredFeature {
            id: capability_feature(*capability)?,
            usage_count: *count,
            path: "planningProblem.manifest.capabilityCounts".to_owned(),
        });
    }
    match &summary.lexicographic_strategy {
        LexicographicStrategy::ExactScalarization { .. } if summary.objective_level_count > 0 => {
            required.push(required_feature(
                "ir.scalarized-objectives",
                summary.objective_level_count,
                "planningProblem.objectives",
            )?);
        }
        LexicographicStrategy::Multipass => {
            required.push(required_feature(
                "ir.multipass-objectives",
                summary.objective_level_count,
                "planningProblem.objectives",
            )?);
        }
        LexicographicStrategy::ExactScalarization { .. } => {}
    }
    required.push(required_feature(
        "solve.cancellation",
        1,
        "solveOptions.timeLimitMilliseconds",
    )?);
    required.push(required_feature(
        "solve.resource-limits",
        1,
        "solveOptions.resourceLimits",
    )?);
    if matches!(options.reproducibility, ReproducibilityMode::Deterministic) {
        required.push(required_feature(
            "solve.deterministic-mode",
            1,
            "solveOptions.reproducibility",
        )?);
    }
    if options.collect_intermediate_solutions {
        required.push(required_feature(
            "solve.intermediate-candidates",
            1,
            "solveOptions.collectIntermediateSolutions",
        )?);
    }
    if summary.objective_level_count > 0 {
        required.push(required_feature(
            "solve.proof-and-bounds",
            summary.objective_level_count,
            "planningProblem.objectives",
        )?);
    }
    if !matches!(options.explanation_mode, ExplanationMode::None) {
        required.push(required_feature(
            "solve.infeasibility-evidence",
            1,
            "solveOptions.explanationMode",
        )?);
    }
    let estimate = ModelCostEstimate {
        variables: summary.variable_count,
        constraints: summary.constraint_count,
        references: summary.total_reference_count,
    };
    Ok(matrix.report(backend, required, Some(estimate)))
}

fn capability_feature(capability: Capability) -> Result<SupportFeatureId, PreflightError> {
    let value = match capability {
        Capability::BoolOr => "ir.bool-or",
        Capability::BoolAnd => "ir.bool-and",
        Capability::Implication => "ir.implication",
        Capability::Equivalence => "ir.equivalence",
        Capability::AtMostOne => "ir.at-most-one",
        Capability::ExactlyOne => "ir.exactly-one",
        Capability::CardinalityRange => "ir.cardinality-range",
        Capability::LinearComparison => "ir.integer-linear",
        Capability::ReifiedLinearComparison => "ir.reified-linear-comparison",
        Capability::AllDifferent => "ir.all-different",
        Capability::AllowedTable => "ir.allowed-table",
        Capability::ForbiddenTable => "ir.forbidden-table",
        Capability::Element => "ir.element",
        Capability::Min => "ir.minimum",
        Capability::Max => "ir.maximum",
        Capability::Equality => "ir.equality",
        Capability::AbsDifference => "ir.absolute-difference",
        Capability::NoOverlap => "ir.no-overlap",
        Capability::Cumulative => "ir.cumulative",
        Capability::OptionalIntervals => "ir.optional-interval",
        Capability::ObjectivePenalty => "ir.objective-penalty",
        Capability::ObjectiveReward => "ir.objective-reward",
        Capability::Assumptions => "ir.assumptions",
        Capability::BooleanProjection => "projection.boolean",
        Capability::IntegerProjection => "projection.integer",
        Capability::IntervalProjection => "projection.interval",
        Capability::AbsentProjection => "projection.absent",
    };
    SupportFeatureId::new(value).map_err(|error| PreflightError::InternalFeature(error.to_string()))
}

fn required_feature(
    id: &str,
    usage_count: u64,
    path: &str,
) -> Result<RequiredFeature, PreflightError> {
    Ok(RequiredFeature {
        id: SupportFeatureId::new(id)
            .map_err(|error| PreflightError::InternalFeature(error.to_string()))?,
        usage_count,
        path: path.to_owned(),
    })
}

/// Validates a selected request before a backend may be invoked.
///
/// # Errors
///
/// Returns an error if the descriptor, request identity, selected backend, budget, planning
/// problem, summary, solve options, or support-matrix agreement is invalid. It also returns
/// [`PreflightError::Cancelled`], [`PreflightError::DeadlineExceeded`], or
/// [`PreflightError::Incompatible`] when the request cannot be dispatched for the corresponding
/// reason.
pub fn preflight(
    matrix: &CapabilityMatrix,
    descriptor: &SolverDescriptor,
    request: &crate::SolveRequest,
) -> Result<CompatibilityReport, PreflightError> {
    descriptor
        .validate()
        .map_err(|error| PreflightError::InvalidDescriptor(error.to_string()))?;
    if request.backend_id() != &descriptor.id {
        return Err(PreflightError::RequestBackendMismatch);
    }
    if request.backend_version() != descriptor.version.as_str() {
        return Err(PreflightError::BackendVersionMismatch);
    }
    if request.adapter_version() != descriptor.adapter_version.as_str() {
        return Err(PreflightError::AdapterVersionMismatch);
    }
    match &request.options().backend {
        BackendSelection::Auto => return Err(PreflightError::UnresolvedBackendSelection),
        BackendSelection::Specific(id) if id != request.backend_id() => {
            return Err(PreflightError::OptionBackendMismatch);
        }
        BackendSelection::Specific(_) => {}
    }
    if request.dispatch_budget().child_view().is_cancelled() {
        return Err(PreflightError::Cancelled);
    }
    if request.dispatch_budget().child_view().is_expired()
        || request.dispatch_budget().backend_limit().value() == 0
    {
        return Err(PreflightError::DeadlineExceeded);
    }
    matrix
        .validate_descriptor(descriptor)
        .map_err(|error| PreflightError::Matrix(error.to_string()))?;
    let limits = PlanningIrLimitsV1::DEFAULT.tightened_by(request.options().resource_limits);
    let prepared_matches = matches!(
        request.prepared_model(),
        Some(prepared)
            if prepared.limits() == limits
                && Arc::ptr_eq(prepared.problem(), request.problem())
                && prepared.summary() == request.summary()
    );
    if !prepared_matches {
        let mut preparation = request.dispatch_budget().preparation_budget(limits);
        let actual = summarize_with_budget(request.problem(), limits, &mut preparation)
            .map_err(|error| PreflightError::InvalidProblem(error.to_string()))?;
        if actual.summary() != request.summary() {
            return Err(PreflightError::SummaryMismatch);
        }
    }
    if request.options().validate().is_err() {
        return Err(PreflightError::InvalidSolveOptions);
    }
    let report = compatibility_for(
        matrix,
        request.backend_id(),
        request.summary(),
        request.options(),
    )?;
    if !report.compatible() {
        return Err(PreflightError::Incompatible(report));
    }
    Ok(report)
}

/// Checks backend identity, timing, terminal/candidate coherence, and bounded evidence.
///
/// # Errors
///
/// Returns an error if the limits are invalid or if the outcome's identity, budget evidence,
/// timing, objective dimensions and bound consistency, candidates, termination reason,
/// first-incumbent evidence, or profile-bound complete assumption-core evidence violates the
/// request contract.
pub fn validate_outcome(
    request: &crate::SolveRequest,
    outcome: &BackendSolveOutcome,
    candidates: &[BackendCandidate],
    limits: SolverApiLimits,
) -> Result<(), OutcomeError> {
    limits.validate().map_err(|_| OutcomeError::InvalidLimits)?;
    if &outcome.backend_id != request.backend_id() {
        return Err(OutcomeError::BackendMismatch);
    }
    if outcome.model_hash != request.model_hash() {
        return Err(OutcomeError::ModelHashMismatch);
    }
    if outcome.solve_fingerprint != request.solve_fingerprint() {
        return Err(OutcomeError::SolveFingerprintMismatch);
    }
    if outcome.evidence.remaining_at_dispatch_milliseconds
        != request.dispatch_budget().remaining_at_dispatch()
        || outcome.evidence.backend_limit_milliseconds != request.dispatch_budget().backend_limit()
    {
        return Err(OutcomeError::BudgetEvidenceMismatch);
    }
    if outcome.evidence.elapsed_milliseconds > outcome.evidence.backend_limit_milliseconds {
        return Err(OutcomeError::BackendLimitExceeded);
    }
    if outcome
        .evidence
        .first_incumbent_milliseconds
        .is_some_and(|value| value > outcome.evidence.elapsed_milliseconds)
    {
        return Err(OutcomeError::InvalidFirstIncumbentTime);
    }
    validate_execution_evidence(request, outcome)?;
    validate_assumption_core(request, outcome)?;
    if outcome.evidence.evidence_refs.len() > limits.max_evidence_refs_per_candidate as usize {
        return Err(OutcomeError::OutputLimitExceeded);
    }
    validate_objective_evidence(
        outcome.evidence.objective.as_ref(),
        request.problem().objectives.levels.len(),
    )?;
    validate_candidates(
        request,
        candidates,
        limits,
        outcome.evidence.elapsed_milliseconds,
    )?;
    validate_best_bound_evidence(
        request,
        outcome.evidence.best_bound_values.as_deref(),
        outcome.evidence.objective.as_ref(),
        candidates,
    )?;
    let has_candidates = !candidates.is_empty();
    match outcome.termination {
        BackendTerminationReason::OptimalityClaimed | BackendTerminationReason::CandidateFound
            if !has_candidates =>
        {
            return Err(OutcomeError::MissingCandidate);
        }
        BackendTerminationReason::InfeasibilityClaimed
        | BackendTerminationReason::UnboundedClaimed
        | BackendTerminationReason::InvalidModel
        | BackendTerminationReason::Unavailable
        | BackendTerminationReason::Failed
            if has_candidates =>
        {
            return Err(OutcomeError::CandidateContradictsTermination);
        }
        BackendTerminationReason::Cancelled
            if !request.dispatch_budget().child_view().is_cancelled() =>
        {
            return Err(OutcomeError::UnrequestedCancellation);
        }
        BackendTerminationReason::TimeLimit
            if !request.dispatch_budget().child_view().is_expired()
                && outcome.evidence.elapsed_milliseconds
                    < outcome.evidence.backend_limit_milliseconds =>
        {
            return Err(OutcomeError::PrematureTimeLimit);
        }
        BackendTerminationReason::OptimalityClaimed
        | BackendTerminationReason::CandidateFound
        | BackendTerminationReason::InfeasibilityClaimed
        | BackendTerminationReason::UnboundedClaimed
        | BackendTerminationReason::TimeLimit
        | BackendTerminationReason::SolutionLimit
        | BackendTerminationReason::Cancelled
        | BackendTerminationReason::InvalidModel
        | BackendTerminationReason::Unavailable
        | BackendTerminationReason::Failed => {}
    }
    let expected_first_incumbent = candidates
        .first()
        .map(|candidate| candidate.observed_after_milliseconds);
    if outcome.evidence.first_incumbent_milliseconds != expected_first_incumbent {
        return Err(OutcomeError::FirstIncumbentMismatch);
    }
    Ok(())
}

fn validate_assumption_core(
    request: &crate::SolveRequest,
    outcome: &BackendSolveOutcome,
) -> Result<(), OutcomeError> {
    let problem = request.problem();
    let safe_profile = !problem.assumptions.is_empty()
        && problem.objectives.levels.is_empty()
        && request.options().worker_threads == WorkerThreadPolicy::Exact(1);
    let expected =
        safe_profile && outcome.termination == BackendTerminationReason::InfeasibilityClaimed;
    let Some(core) = &outcome.evidence.assumption_core else {
        return if expected {
            Err(OutcomeError::InvalidAssumptionCoreEvidence)
        } else {
            Ok(())
        };
    };
    if !expected {
        return Err(OutcomeError::InvalidAssumptionCoreEvidence);
    }
    let BackendAssumptionCoreEvidence::Sufficient { literals } = core else {
        return Ok(());
    };
    if literals.is_empty()
        || literals.len() > problem.assumptions.len()
        || !literals.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(OutcomeError::InvalidAssumptionCoreEvidence);
    }
    // The admitted IR has unique assumption variables. Searching the canonical core while
    // walking submitted assumptions proves complete signed membership without an extra index
    // allocation or quadratic work, regardless of assumption-group identity order.
    let matched = problem
        .assumptions
        .iter()
        .filter(|assumption| literals.binary_search(&assumption.literal).is_ok())
        .count();
    if matched != literals.len() {
        return Err(OutcomeError::InvalidAssumptionCoreEvidence);
    }
    Ok(())
}

fn validate_execution_evidence(
    request: &crate::SolveRequest,
    outcome: &BackendSolveOutcome,
) -> Result<(), OutcomeError> {
    let Some(execution) = &outcome.evidence.execution else {
        return Ok(());
    };
    let reproducibility = &execution.reproducibility;
    if reproducibility.backend_version != request.backend_version() {
        return Err(OutcomeError::ExecutionBackendVersionMismatch);
    }
    if reproducibility.adapter_version != request.adapter_version() {
        return Err(OutcomeError::ExecutionAdapterVersionMismatch);
    }
    if &reproducibility.applied_options != request.options() {
        return Err(OutcomeError::ExecutionOptionsMismatch);
    }
    let expected_seed = i32::try_from(request.options().random_seed)
        .map_err(|_| OutcomeError::InvalidExecutionEvidence)?;
    let applied = &reproducibility.applied_parameters;
    if applied.random_seed != expected_seed
        || applied.worker_threads == 0
        || applied.stop_after_first_feasible != request.options().stop_after_first_feasible
        || (applied.emit_intermediate_solutions
            && !request.options().collect_intermediate_solutions)
        || applied.deterministic_test_profile
            != matches!(
                request.options().reproducibility,
                ReproducibilityMode::Deterministic
            )
        || matches!(
            request.options().worker_threads,
            WorkerThreadPolicy::Exact(count)
                if applied.worker_threads != u32::from(count)
        )
    {
        return Err(OutcomeError::InvalidExecutionEvidence);
    }
    if execution.model_counts.planning_variable_count != request.summary().variable_count
        || execution.model_counts.planning_constraint_count != request.summary().constraint_count
    {
        return Err(OutcomeError::ExecutionModelCountMismatch);
    }
    let timings = &execution.timings;
    if [
        timings.translation_serialization_milliseconds,
        timings.worker_startup_milliseconds.unwrap_or_default(),
        timings.handshake_milliseconds.unwrap_or_default(),
        timings.solver_milliseconds.unwrap_or_default(),
        timings.protocol_decode_milliseconds.unwrap_or_default(),
    ]
    .into_iter()
    .any(|value| value > outcome.evidence.elapsed_milliseconds)
        || applied.wall_time_milliseconds.is_some_and(|value| {
            value == eutheto_types::DurationMillis::ZERO
                || value > outcome.evidence.backend_limit_milliseconds
        })
    {
        return Err(OutcomeError::InvalidExecutionEvidence);
    }
    if reproducibility.protocol_major == 0
        || !valid_version_field(&reproducibility.worker_version)
        || !valid_version_field(&reproducibility.engine_version)
        || !valid_sha256(&reproducibility.model_fingerprint_sha256)
        || reproducibility
            .applied_parameters_sha256
            .as_deref()
            .is_some_and(|value| !valid_sha256(value))
        || execution
            .worker_statistics
            .as_ref()
            .is_some_and(|statistics| {
                [
                    statistics.conflicts,
                    statistics.branches,
                    statistics.binary_propagations,
                    statistics.integer_propagations,
                ]
                .into_iter()
                .flatten()
                .any(|value| value > BACKEND_STATISTIC_MAX_V1)
            })
    {
        return Err(OutcomeError::InvalidExecutionEvidence);
    }
    Ok(())
}

fn valid_version_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_candidates(
    request: &crate::SolveRequest,
    candidates: &[BackendCandidate],
    limits: SolverApiLimits,
    termination_elapsed: eutheto_types::DurationMillis,
) -> Result<(), OutcomeError> {
    if candidates.len() > limits.max_candidates as usize {
        return Err(OutcomeError::OutputLimitExceeded);
    }
    let assignment_count = candidates.iter().try_fold(0_u64, |count, candidate| {
        let candidate_count = u64::try_from(candidate.values.booleans.len())
            .ok()
            .and_then(|booleans| {
                u64::try_from(candidate.values.integers.len())
                    .ok()
                    .and_then(|integers| booleans.checked_add(integers))
            })
            .ok_or(OutcomeError::CandidateAssignmentLimitExceeded)?;
        count
            .checked_add(candidate_count)
            .filter(|total| *total <= u64::from(limits.max_candidate_assignments))
            .ok_or(OutcomeError::CandidateAssignmentLimitExceeded)
    })?;
    debug_assert!(assignment_count <= u64::from(limits.max_candidate_assignments));
    let objective_level_count = request.problem().objectives.levels.len();
    for (index, candidate) in candidates.iter().enumerate() {
        let expected = u32::try_from(index + 1).map_err(|_| OutcomeError::OutputLimitExceeded)?;
        if candidate.sequence != expected {
            return Err(OutcomeError::CandidateSequenceMismatch);
        }
        if candidate.observed_after_milliseconds > termination_elapsed {
            return Err(OutcomeError::CandidateAfterTermination);
        }
        validate_objective_evidence(candidate.objective.as_ref(), objective_level_count)?;
        if index > 0
            && candidate.observed_after_milliseconds
                < candidates[index - 1].observed_after_milliseconds
        {
            return Err(OutcomeError::NonMonotonicCandidateTime);
        }
    }
    Ok(())
}

fn validate_objective_evidence(
    evidence: Option<&crate::BackendObjectiveEvidence>,
    objective_level_count: usize,
) -> Result<(), OutcomeError> {
    if evidence.is_some_and(|value| value.objective_values.len() != objective_level_count) {
        return Err(OutcomeError::InvalidObjectiveDimension);
    }
    Ok(())
}

fn validate_best_bound_evidence(
    request: &crate::SolveRequest,
    bounds: Option<&[i64]>,
    objective: Option<&crate::BackendObjectiveEvidence>,
    candidates: &[BackendCandidate],
) -> Result<(), OutcomeError> {
    let Some(bounds) = bounds else {
        return Ok(());
    };
    let levels = &request.problem().objectives.levels;
    if levels.is_empty() || bounds.is_empty() || bounds.len() != levels.len() {
        return Err(OutcomeError::InvalidObjectiveDimension);
    }
    for (index, (bound, level)) in bounds.iter().zip(levels).enumerate() {
        if !(level.lower_bound..=level.upper_bound).contains(bound) {
            return Err(OutcomeError::BestBoundOutsideDeclaredRange);
        }
        for evidence in objective.into_iter().chain(
            candidates
                .iter()
                .filter_map(|candidate| candidate.objective.as_ref()),
        ) {
            let value = evidence
                .objective_values
                .get(index)
                .ok_or(OutcomeError::InvalidObjectiveDimension)?;
            let consistent = match level.direction {
                OptimizationDirection::Minimize => bound <= value,
                OptimizationDirection::Maximize => bound >= value,
            };
            if !consistent {
                return Err(OutcomeError::BestBoundContradictsObjective);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreflightError {
    InvalidDescriptor(String),
    RequestBackendMismatch,
    BackendVersionMismatch,
    UnresolvedBackendSelection,
    AdapterVersionMismatch,
    OptionBackendMismatch,
    Cancelled,
    DeadlineExceeded,
    Matrix(String),
    InvalidProblem(String),
    SummaryMismatch,
    InvalidSolveOptions,
    InternalFeature(String),
    Incompatible(CompatibilityReport),
}

impl fmt::Display for PreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "solver preflight error: {self:?}")
    }
}

impl std::error::Error for PreflightError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutcomeError {
    InvalidLimits,
    BackendMismatch,
    ModelHashMismatch,
    SolveFingerprintMismatch,
    BudgetEvidenceMismatch,
    BackendLimitExceeded,
    InvalidFirstIncumbentTime,
    OutputLimitExceeded,
    CandidateAssignmentLimitExceeded,
    InvalidObjectiveDimension,
    BestBoundOutsideDeclaredRange,
    BestBoundContradictsObjective,
    NonMonotonicCandidateTime,
    CandidateSequenceMismatch,
    CandidateAfterTermination,
    MissingCandidate,
    CandidateContradictsTermination,
    UnrequestedCancellation,
    PrematureTimeLimit,
    FirstIncumbentMismatch,
    ExecutionBackendVersionMismatch,
    ExecutionAdapterVersionMismatch,
    ExecutionOptionsMismatch,
    ExecutionModelCountMismatch,
    InvalidExecutionEvidence,
    InvalidAssumptionCoreEvidence,
}

impl fmt::Display for OutcomeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "solver outcome contract error: {self:?}")
    }
}

impl std::error::Error for OutcomeError {}
