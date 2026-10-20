//! Native router claims are descriptive; projection and replay never grant acceptance authority.

use eutheto_domain_ir::{
    DomainContractError, MAX_SCORE_LEVELS, RunInput, RunManifest, RunNativeAttempt,
    RunNativeAttemptTermination, RunNativeObjective, RunNativeObservations, RunNativeOutcome,
    RunNativeRouterReason, RunNativeRuntime, RunNativeTerminationReason, ScoreLevelId,
};
use eutheto_planning_ir::{PlanningProblem, SolveFingerprintInput, routed_solve_fingerprint};
use eutheto_solver_api::{BackendTerminationReason, SolverRegistry};
use eutheto_solver_router::{AttemptTermination, ExecutionTerminalReason, RouterExecutionRecord};
use eutheto_types::BackendSelection;
use std::fmt::Write;

fn invalid(invariant: &'static str) -> DomainContractError {
    DomainContractError::InvalidNativeObservations(invariant)
}

pub(crate) fn capture(
    input: &RunInput,
    problem: &PlanningProblem,
    registry: &SolverRegistry,
    record: &RouterExecutionRecord,
) -> Result<RunNativeObservations, DomainContractError> {
    if record.attempts.len() > 1
        || record.invocation_count > 1
        || record.attempts.iter().any(|attempt| attempt.fallback_taken)
        || record.solve_options != input.solve_options
        || !matches!(&input.solve_options.backend,
            BackendSelection::Specific(backend_id) if backend_id == &input.backend_id)
        || problem.objectives.levels.len() > MAX_SCORE_LEVELS
    {
        return Err(invalid("current specific-only router projection"));
    }
    let objectives = problem
        .objectives
        .levels
        .iter()
        .map(|level| {
            Ok(RunNativeObjective {
                level_id: ScoreLevelId::new(level.id.as_str())
                    .map_err(|_| invalid("compiled objective identity"))?,
                direction: level.direction,
                lower_bound: level.lower_bound,
                upper_bound: level.upper_bound,
            })
        })
        .collect::<Result<Vec<_>, DomainContractError>>()?;
    let attempt = record
        .attempts
        .first()
        .map(|attempt| {
            let backend = registry
                .get(&attempt.backend_id)
                .ok_or_else(|| invalid("selected backend absent from immutable registry"))?;
            let descriptor = backend.descriptor();
            if descriptor.id != attempt.backend_id
                || descriptor.version != attempt.backend_version
                || descriptor.adapter_version != attempt.adapter_version
            {
                return Err(invalid("selected backend descriptor binding"));
            }
            let approved_worker_manifest_sha256 = backend
                .verified_worker_manifest_sha256()
                .map(digest_hex)
                .transpose()?;
            let outcome = attempt
                .outcome
                .as_ref()
                .map(|outcome| capture_outcome(input, attempt, outcome))
                .transpose()?;
            Ok(RunNativeAttempt {
                backend_id: attempt.backend_id.clone(),
                backend_version: attempt.backend_version.clone(),
                adapter_version: attempt.adapter_version.clone(),
                solve_fingerprint: attempt.solve_fingerprint.clone(),
                invocation_index: attempt.invocation_index,
                remaining_at_dispatch_milliseconds: attempt.remaining_at_dispatch_milliseconds,
                backend_limit_milliseconds: attempt.backend_limit_milliseconds,
                termination: native_attempt_termination(&attempt.termination),
                backend_failure_code: attempt.backend_failure_code.clone(),
                approved_worker_manifest_sha256,
                outcome,
            })
        })
        .transpose()?;
    let observations = RunNativeObservations::new(
        input,
        objectives,
        record.terminal_status,
        native_router_reason(record.terminal_reason),
        attempt,
    )?;
    if let Some(attempt) = &observations.attempt {
        validate_attempt_fingerprint(input, attempt)?;
    }
    Ok(observations)
}

/// Call only after complete manifest/input structural validation; this is recorded binding, not
/// authentication of an imported historical artifact or independent model reconstruction.
pub(crate) fn validate_fingerprint(
    input: &RunInput,
    manifest: &RunManifest,
) -> Result<(), DomainContractError> {
    let Some(attempt) = manifest
        .native_observations
        .as_ref()
        .and_then(|observations| observations.attempt.as_ref())
    else {
        return Ok(());
    };
    validate_attempt_fingerprint(input, attempt)
}

/// Requires the source tuple to retain an attempt from a currently approved worker artifact.
///
/// Fingerprint consistency and independent model verification do not establish worker origin.
pub(crate) fn validate_approved_worker_origin(
    input: &RunInput,
    manifest: &RunManifest,
    registry: &SolverRegistry,
) -> Result<(), DomainContractError> {
    validate_fingerprint(input, manifest)?;
    let attempt = manifest
        .native_observations
        .as_ref()
        .and_then(|observations| observations.attempt.as_ref())
        .ok_or_else(|| invalid("approved worker attempt is missing"))?;
    let backend = registry
        .get(&attempt.backend_id)
        .ok_or_else(|| invalid("approved worker is absent from the current registry"))?;
    let approved_manifest = backend
        .verified_worker_manifest_sha256()
        .ok_or_else(|| invalid("selected backend has no approved worker artifact"))?;
    let approved_manifest = digest_hex(approved_manifest)?;
    if attempt.approved_worker_manifest_sha256.as_deref() != Some(approved_manifest.as_str())
        || attempt.outcome.is_none()
    {
        return Err(invalid("retained approved worker binding"));
    }
    Ok(())
}

pub(crate) fn validate_counterfactual_fingerprints(
    result: &eutheto_domain_ir::CounterfactualResult,
) -> Result<(), DomainContractError> {
    validate_fingerprint(&result.base_run_input, &result.base_run_manifest)?;
    validate_fingerprint(&result.run_input, &result.run_manifest)
}

fn validate_attempt_fingerprint(
    input: &RunInput,
    attempt: &RunNativeAttempt,
) -> Result<(), DomainContractError> {
    let options = if matches!(&input.solve_options.backend,
        BackendSelection::Specific(backend_id) if backend_id == &attempt.backend_id)
    {
        std::borrow::Cow::Borrowed(&input.solve_options)
    } else {
        let mut options = input.solve_options.clone();
        options.backend = BackendSelection::Specific(attempt.backend_id.clone());
        std::borrow::Cow::Owned(options)
    };
    let fingerprint = routed_solve_fingerprint(&SolveFingerprintInput {
        canonical_ir_hash: input.model_hash.clone(),
        backend_id: attempt.backend_id.as_str().to_owned(),
        backend_version: attempt.backend_version.clone(),
        adapter_version: attempt.adapter_version.clone(),
        canonical_options: serde_json::to_vec(&options)
            .map_err(|error| DomainContractError::MalformedJson(error.to_string()))?,
    });
    if fingerprint != attempt.solve_fingerprint {
        return Err(invalid("recorded routed fingerprint binding"));
    }
    Ok(())
}

fn digest_hex(digest: &[u8; 32]) -> Result<String, DomainContractError> {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(encoded, "{byte:02x}").map_err(|_| invalid("digest encoding"))?;
    }
    Ok(encoded)
}

fn native_reason(reason: BackendTerminationReason) -> RunNativeTerminationReason {
    match reason {
        BackendTerminationReason::OptimalityClaimed => {
            RunNativeTerminationReason::OptimalityClaimed
        }
        BackendTerminationReason::CandidateFound => RunNativeTerminationReason::CandidateFound,
        BackendTerminationReason::InfeasibilityClaimed => {
            RunNativeTerminationReason::InfeasibilityClaimed
        }
        BackendTerminationReason::UnboundedClaimed => RunNativeTerminationReason::UnboundedClaimed,
        BackendTerminationReason::TimeLimit => RunNativeTerminationReason::TimeLimit,
        BackendTerminationReason::SolutionLimit => RunNativeTerminationReason::SolutionLimit,
        BackendTerminationReason::Cancelled => RunNativeTerminationReason::Cancelled,
        BackendTerminationReason::InvalidModel => RunNativeTerminationReason::InvalidModel,
        BackendTerminationReason::Unavailable => RunNativeTerminationReason::Unavailable,
        BackendTerminationReason::Failed => RunNativeTerminationReason::Failed,
    }
}

fn native_router_reason(reason: ExecutionTerminalReason) -> RunNativeRouterReason {
    match reason {
        ExecutionTerminalReason::InvalidModel => RunNativeRouterReason::InvalidModel,
        ExecutionTerminalReason::UnsupportedOverride => RunNativeRouterReason::UnsupportedOverride,
        ExecutionTerminalReason::NoCompatibleBackend => RunNativeRouterReason::NoCompatibleBackend,
        ExecutionTerminalReason::PreflightRejected => RunNativeRouterReason::PreflightRejected,
        ExecutionTerminalReason::BackendTerminated => RunNativeRouterReason::BackendTerminated,
        ExecutionTerminalReason::BackendFailure => RunNativeRouterReason::BackendFailure,
        ExecutionTerminalReason::CandidateAwaitingVerification => {
            RunNativeRouterReason::CandidateAwaitingVerification
        }
        ExecutionTerminalReason::CandidateVerified => RunNativeRouterReason::CandidateVerified,
        ExecutionTerminalReason::VerificationQuarantined => {
            RunNativeRouterReason::VerificationQuarantined
        }
        ExecutionTerminalReason::SharedOutputLimitExhausted => {
            RunNativeRouterReason::SharedOutputLimitExhausted
        }
        ExecutionTerminalReason::ReviewResourceLimitExceeded => {
            RunNativeRouterReason::ReviewResourceLimitExceeded
        }
        ExecutionTerminalReason::ParentDeadlineExceeded => {
            RunNativeRouterReason::ParentDeadlineExceeded
        }
        ExecutionTerminalReason::Cancelled => RunNativeRouterReason::Cancelled,
    }
}

fn native_attempt_termination(termination: &AttemptTermination) -> RunNativeAttemptTermination {
    match termination {
        AttemptTermination::PreflightRejected { code } => {
            RunNativeAttemptTermination::PreflightRejected { code: code.clone() }
        }
        AttemptTermination::BackendError { code } => {
            RunNativeAttemptTermination::BackendError { code: code.clone() }
        }
        AttemptTermination::OutcomeContractFailure { code } => {
            RunNativeAttemptTermination::OutcomeContractFailure { code: code.clone() }
        }
        AttemptTermination::BackendOutcome(reason) => {
            RunNativeAttemptTermination::BackendOutcome(native_reason(*reason))
        }
        AttemptTermination::CandidateAwaitingVerification => {
            RunNativeAttemptTermination::CandidateAwaitingVerification
        }
        AttemptTermination::CandidateVerified => RunNativeAttemptTermination::CandidateVerified,
        AttemptTermination::VerificationQuarantined { diagnostic_code } => {
            RunNativeAttemptTermination::VerificationQuarantined {
                diagnostic_code: diagnostic_code.clone(),
            }
        }
        AttemptTermination::ReviewCancelled => RunNativeAttemptTermination::ReviewCancelled,
        AttemptTermination::ReviewDeadlineExceeded => {
            RunNativeAttemptTermination::ReviewDeadlineExceeded
        }
        AttemptTermination::ReviewResourceLimitExceeded => {
            RunNativeAttemptTermination::ReviewResourceLimitExceeded
        }
        AttemptTermination::ParentCancelled => RunNativeAttemptTermination::ParentCancelled,
        AttemptTermination::ParentDeadlineExceeded => {
            RunNativeAttemptTermination::ParentDeadlineExceeded
        }
    }
}

fn capture_outcome(
    input: &RunInput,
    attempt: &eutheto_solver_router::AttemptRecord,
    outcome: &eutheto_solver_api::BackendSolveOutcome,
) -> Result<RunNativeOutcome, DomainContractError> {
    if outcome.backend_id != attempt.backend_id
        || outcome.model_hash != input.model_hash
        || outcome.solve_fingerprint != attempt.solve_fingerprint
    {
        return Err(invalid("admitted outcome binding"));
    }
    let runtime = outcome.evidence.execution.as_ref().and_then(|execution| {
        let retained = &execution.reproducibility;
        retained
            .applied_parameters_sha256
            .as_ref()
            .map(|digest| RunNativeRuntime {
                worker_version: retained.worker_version.clone(),
                engine_version: retained.engine_version.clone(),
                protocol_major: retained.protocol_major,
                protocol_minor: retained.protocol_minor,
                model_fingerprint_sha256: retained.model_fingerprint_sha256.clone(),
                applied_parameters_sha256: digest.clone(),
            })
    });
    Ok(RunNativeOutcome {
        reason: native_reason(outcome.termination),
        objective_values: outcome
            .evidence
            .objective
            .as_ref()
            .map(|objective| objective.objective_values.clone()),
        best_bound_values: outcome.evidence.best_bound_values.clone(),
        runtime,
    })
}
