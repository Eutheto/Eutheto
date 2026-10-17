//! Native observations stay inert until the exact final terminal manifest is known.

use eutheto_domain_ir::{
    CounterfactualConditionV1, DiagnosticInfeasibilityProofV1, InfeasibilityEvidenceV1,
    OriginalInfeasibilityObservationV1, RunInfeasibilityEvidenceV1, RunInput, RunManifest,
    RunTerminalOutcomeV1,
};
use eutheto_solver_api::SolverBackend;
use eutheto_types::{
    AppError, DurationMillis, OperationControl, ParentSolveBudget, ProtocolFailure,
    Rfc3339Timestamp, SolveStatus,
};
use std::time::Duration;

pub(crate) struct InertInfeasibilityEvidence {
    pub original: OriginalInfeasibilityObservationV1,
    pub condition: Option<CounterfactualConditionV1>,
    pub diagnostic: Option<DiagnosticInfeasibilityProofV1>,
    pub infeasibility: InfeasibilityEvidenceV1,
}

impl InertInfeasibilityEvidence {
    /// Consumes native-only material after refresh/retime; other terminal states retain no proof.
    pub(crate) fn bind(
        self,
        input: &RunInput,
        manifest: &RunManifest,
    ) -> Result<Option<RunInfeasibilityEvidenceV1>, AppError> {
        if !matches!(
            manifest.outcome,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible
            }
        ) {
            return Ok(None);
        }
        RunInfeasibilityEvidenceV1::new(
            input,
            manifest,
            self.original,
            self.condition,
            self.diagnostic,
            self.infeasibility,
        )
        .map(Some)
        .map_err(|_| crate::application::solution_contract_error())
    }
}

/// Called immediately after locally validated original routing, before optional work.
/// The caller separately establishes the actual original Infeasible terminal and runtime outcome.
pub(crate) fn capture_original_infeasibility(
    input: &RunInput,
    backend: &dyn SolverBackend,
    budget: &ParentSolveBudget,
    started_at: Rfc3339Timestamp,
    observed_at: Rfc3339Timestamp,
) -> Result<Option<OriginalInfeasibilityObservationV1>, AppError> {
    let elapsed = budget
        .checked_elapsed()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .and_then(|elapsed| DurationMillis::new(elapsed).ok())
        .ok_or_else(clock_error)?;
    let deadline = started_at
        .as_timestamp()
        .checked_add(Duration::from_millis(
            input.solve_options.time_limit_milliseconds.value(),
        ))
        .map_err(|_| clock_error())?;
    if observed_at < started_at {
        return Err(clock_error());
    }
    if OperationControl::Solve(budget.phase_view())
        .check()
        .is_err()
        || elapsed >= input.solve_options.time_limit_milliseconds
        || observed_at.as_timestamp() >= deadline
    {
        return Ok(None);
    }
    let identity = backend.runtime_identity();
    if identity.backend_id() != &input.backend_id
        || identity.backend_version() != input.backend_version
        || identity.adapter_version() != input.adapter_version
        || identity.worker_version() != input.worker_version
        || identity.solver_version() != input.solver_version
        || identity.protocol_major() != input.protocol_major
        || identity.protocol_minor() != input.protocol_minor
    {
        return Err(crate::application::solution_contract_error());
    }
    Ok(Some(OriginalInfeasibilityObservationV1 {
        run_input_checksum: input.checksum.clone(),
        model_hash: input.model_hash.clone(),
        worker_manifest_sha256: backend
            .verified_worker_manifest_sha256()
            .map(manifest_digest),
        observed_at,
        elapsed_milliseconds: elapsed,
    }))
}

pub(crate) fn manifest_digest(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut digest = String::with_capacity(64);
    for &byte in bytes {
        digest.push(char::from(HEX[usize::from(byte >> 4)]));
        digest.push(char::from(HEX[usize::from(byte & 15)]));
    }
    digest
}

fn clock_error() -> AppError {
    AppError::Protocol(ProtocolFailure {
        code: "solve.clock_invalid".to_owned(),
        message: "The operation timing cannot be represented safely.".to_owned(),
        retryable: false,
        diagnostic_id: None,
    })
}
