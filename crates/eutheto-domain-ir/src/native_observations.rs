//! Descriptive native claims retained separately from application acceptance and verification.

use crate::{
    DomainContractError, MAX_RUN_DIAGNOSTIC_CODE_BYTES, MAX_SCORE_LEVELS, OptimizationDirection,
    RunInput, ScoreLevelId, blake3_hex, validate_hash, validate_run_version,
};
use eutheto_types::{BackendId, DurationMillis, SolveStatus};
use serde::{Deserialize, Serialize};

/// Current compact native-observation schema. It does not confer verification authority.
pub const RUN_NATIVE_OBSERVATIONS_SCHEMA_VERSION: u32 = 1;

/// One ordered compiled objective, without its expression or constraint graph.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNativeObjective {
    pub level_id: ScoreLevelId,
    pub direction: OptimizationDirection,
    pub lower_bound: i64,
    pub upper_bound: i64,
}

/// Native conclusion before independent original-domain acceptance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunNativeTerminationReason {
    OptimalityClaimed,
    CandidateFound,
    InfeasibilityClaimed,
    UnboundedClaimed,
    TimeLimit,
    SolutionLimit,
    Cancelled,
    InvalidModel,
    Unavailable,
    Failed,
}

/// Exact router conclusion; a later application cleanup/alarm may have a different outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunNativeRouterReason {
    InvalidModel,
    UnsupportedOverride,
    NoCompatibleBackend,
    PreflightRejected,
    BackendTerminated,
    BackendFailure,
    CandidateAwaitingVerification,
    CandidateVerified,
    VerificationQuarantined,
    SharedOutputLimitExhausted,
    ReviewResourceLimitExceeded,
    ParentDeadlineExceeded,
    Cancelled,
}

/// Closed projection of the actual attempt termination, not a recovered application result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "termination",
    content = "details",
    deny_unknown_fields
)]
pub enum RunNativeAttemptTermination {
    PreflightRejected { code: String },
    BackendError { code: String },
    OutcomeContractFailure { code: String },
    BackendOutcome(RunNativeTerminationReason),
    CandidateAwaitingVerification,
    CandidateVerified,
    VerificationQuarantined { diagnostic_code: String },
    ReviewCancelled,
    ReviewDeadlineExceeded,
    ReviewResourceLimitExceeded,
    ParentCancelled,
    ParentDeadlineExceeded,
}

/// Present only after an admitted, protocol-validated terminal corroborated its parameter digest.
/// Configured identity and a parent-computed translation hash alone cannot create this record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNativeRuntime {
    pub worker_version: String,
    pub engine_version: String,
    pub protocol_major: u32,
    pub protocol_minor: u32,
    pub model_fingerprint_sha256: String,
    pub applied_parameters_sha256: String,
}

/// Only a complete outcome admitted by the backend request/output contract may be projected here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNativeOutcome {
    pub reason: RunNativeTerminationReason,
    pub objective_values: Option<Vec<i64>>,
    pub best_bound_values: Option<Vec<i64>>,
    pub runtime: Option<RunNativeRuntime>,
}

/// Zero or one actual attempt is representable: current producers use Specific backend selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNativeAttempt {
    pub backend_id: BackendId,
    pub backend_version: String,
    pub adapter_version: String,
    pub solve_fingerprint: String,
    pub invocation_index: Option<u32>,
    pub remaining_at_dispatch_milliseconds: DurationMillis,
    pub backend_limit_milliseconds: DurationMillis,
    pub termination: RunNativeAttemptTermination,
    pub backend_failure_code: Option<String>,
    /// Identity from the selected backend's authenticated artifact admission, never worker output.
    pub approved_worker_manifest_sha256: Option<String>,
    pub outcome: Option<RunNativeOutcome>,
}

/// Compact source-bound native observations, not candidates, logs, parameter vectors or proof.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNativeObservations {
    pub schema_version: u32,
    pub model_hash: String,
    pub objective_policy_hash: String,
    pub solve_options_hash: String,
    pub objectives: Vec<RunNativeObjective>,
    pub terminal_status: SolveStatus,
    pub terminal_reason: RunNativeRouterReason,
    pub attempt: Option<RunNativeAttempt>,
}

fn invalid(invariant: &'static str) -> DomainContractError {
    DomainContractError::InvalidNativeObservations(invariant)
}

fn validate_code(code: &str) -> Result<(), DomainContractError> {
    if code.is_empty()
        || code.len() > MAX_RUN_DIAGNOSTIC_CODE_BYTES
        || !code.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(invalid("diagnostic code"));
    }
    Ok(())
}

impl RunNativeObservations {
    /// Binds actual projected observations to an already validated immutable input.
    ///
    /// # Errors
    /// Rejects invalid observation shape or source bindings; it does not authenticate public hashes.
    pub fn new(
        input: &RunInput,
        objectives: Vec<RunNativeObjective>,
        terminal_status: SolveStatus,
        terminal_reason: RunNativeRouterReason,
        attempt: Option<RunNativeAttempt>,
    ) -> Result<Self, DomainContractError> {
        let value = Self {
            schema_version: RUN_NATIVE_OBSERVATIONS_SCHEMA_VERSION,
            model_hash: input.model_hash.clone(),
            objective_policy_hash: input.objective_policy_hash.clone(),
            solve_options_hash: blake3_hex(
                &serde_json::to_vec(&input.solve_options)
                    .map_err(|error| DomainContractError::MalformedJson(error.to_string()))?,
            ),
            objectives,
            terminal_status,
            terminal_reason,
            attempt,
        };
        value.validate()?;
        value.validate_attempt_for(input)?;
        Ok(value)
    }

    /// Checks closed bounded shape and internal attempt/outcome/value consistency.
    ///
    /// # Errors
    /// Rejects unknown versions, malformed identities/codes/digests and inconsistent native values.
    pub fn validate(&self) -> Result<(), DomainContractError> {
        if self.schema_version != RUN_NATIVE_OBSERVATIONS_SCHEMA_VERSION {
            return Err(DomainContractError::UnsupportedVersion(self.schema_version));
        }
        for hash in [
            &self.model_hash,
            &self.objective_policy_hash,
            &self.solve_options_hash,
        ] {
            validate_hash(hash)?;
        }
        if self.objectives.len() > MAX_SCORE_LEVELS {
            return Err(invalid("objective count"));
        }
        for (index, objective) in self.objectives.iter().enumerate() {
            if objective.lower_bound > objective.upper_bound
                || self.objectives[..index]
                    .iter()
                    .any(|prior| prior.level_id == objective.level_id)
            {
                return Err(invalid("objective definition"));
            }
        }
        let Some(attempt) = &self.attempt else {
            return Ok(());
        };
        validate_run_version("native backend version", &attempt.backend_version)?;
        validate_run_version("native adapter version", &attempt.adapter_version)?;
        validate_hash(&attempt.solve_fingerprint)?;
        if attempt.invocation_index.is_some_and(|index| index != 1)
            || attempt.backend_limit_milliseconds > attempt.remaining_at_dispatch_milliseconds
        {
            return Err(invalid("single attempt dispatch"));
        }
        if let Some(code) = &attempt.backend_failure_code {
            validate_code(code)?;
        }
        if let Some(hash) = &attempt.approved_worker_manifest_sha256 {
            validate_hash(hash)?;
        }
        match &attempt.termination {
            RunNativeAttemptTermination::PreflightRejected { code } => {
                validate_code(code)?;
                if attempt.invocation_index.is_some() || attempt.outcome.is_some() {
                    return Err(invalid("preflight attempt"));
                }
            }
            RunNativeAttemptTermination::BackendError { code }
            | RunNativeAttemptTermination::OutcomeContractFailure { code } => {
                validate_code(code)?;
                if attempt.invocation_index.is_none() || attempt.outcome.is_some() {
                    return Err(invalid("unadmitted backend outcome"));
                }
            }
            RunNativeAttemptTermination::VerificationQuarantined { diagnostic_code } => {
                validate_code(diagnostic_code)?;
            }
            // A router-enforced backend cap can finish without an admitted worker terminal.
            RunNativeAttemptTermination::BackendOutcome(RunNativeTerminationReason::TimeLimit)
                if attempt.outcome.is_none() =>
            {
                if attempt.invocation_index != Some(1)
                    || self.terminal_status != SolveStatus::NoSolutionWithinLimit
                    || self.terminal_reason != RunNativeRouterReason::BackendTerminated
                {
                    return Err(invalid("attempt terminal reason"));
                }
            }
            RunNativeAttemptTermination::BackendOutcome(reason)
                if attempt
                    .outcome
                    .as_ref()
                    .is_none_or(|outcome| outcome.reason != *reason) =>
            {
                return Err(invalid("attempt terminal reason"));
            }
            _ => {}
        }
        let Some(outcome) = &attempt.outcome else {
            return Ok(());
        };
        if attempt.invocation_index.is_none() {
            return Err(invalid("outcome without invocation"));
        }
        validate_outcome(outcome, &self.objectives)
    }

    /// Validates exact recorded input identity. Core additionally validates the routed fingerprint.
    ///
    /// # Errors
    /// Rejects model/policy/options/backend/runtime substitution. Public hashes remain descriptive.
    pub fn validate_for(&self, input: &RunInput) -> Result<(), DomainContractError> {
        self.validate()?;
        self.validate_input_bindings(input)
    }

    pub(crate) fn validate_input_bindings(
        &self,
        input: &RunInput,
    ) -> Result<(), DomainContractError> {
        let options_hash = blake3_hex(
            &serde_json::to_vec(&input.solve_options)
                .map_err(|error| DomainContractError::MalformedJson(error.to_string()))?,
        );
        if self.model_hash != input.model_hash
            || self.objective_policy_hash != input.objective_policy_hash
            || self.solve_options_hash != options_hash
        {
            return Err(invalid("input model/policy/options binding"));
        }
        self.validate_attempt_for(input)
    }

    fn validate_attempt_for(&self, input: &RunInput) -> Result<(), DomainContractError> {
        if let Some(attempt) = &self.attempt {
            if attempt.backend_id != input.backend_id
                || attempt.backend_version != input.backend_version
                || attempt.adapter_version != input.adapter_version
                || attempt.backend_limit_milliseconds.value()
                    > input.solve_options.time_limit_milliseconds.value()
            {
                return Err(invalid("input backend binding"));
            }
            if let Some(runtime) = attempt
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.runtime.as_ref())
                && (runtime.worker_version != input.worker_version
                    || runtime.engine_version != input.solver_version
                    || runtime.protocol_major != input.protocol_major
                    || runtime.protocol_minor != input.protocol_minor)
            {
                return Err(invalid("input runtime binding"));
            }
        }
        Ok(())
    }
}

fn validate_outcome(
    outcome: &RunNativeOutcome,
    objectives: &[RunNativeObjective],
) -> Result<(), DomainContractError> {
    for values in [&outcome.objective_values, &outcome.best_bound_values]
        .into_iter()
        .flatten()
    {
        if values.is_empty() || values.len() != objectives.len() {
            return Err(invalid("native value dimension"));
        }
        if values.iter().zip(objectives).any(|(value, definition)| {
            *value < definition.lower_bound || *value > definition.upper_bound
        }) {
            return Err(invalid("native value range"));
        }
    }
    if let (Some(objective), Some(bound)) = (&outcome.objective_values, &outcome.best_bound_values)
    {
        for ((objective, bound), definition) in objective.iter().zip(bound).zip(objectives) {
            if match definition.direction {
                OptimizationDirection::Minimize => bound > objective,
                OptimizationDirection::Maximize => bound < objective,
            } {
                return Err(invalid("native bound direction"));
            }
        }
    }
    if let Some(runtime) = &outcome.runtime {
        validate_run_version("native worker version", &runtime.worker_version)?;
        validate_run_version("native engine version", &runtime.engine_version)?;
        validate_hash(&runtime.model_fingerprint_sha256)?;
        validate_hash(&runtime.applied_parameters_sha256)?;
        if runtime.protocol_major == 0 {
            return Err(invalid("native runtime protocol"));
        }
    }
    Ok(())
}
