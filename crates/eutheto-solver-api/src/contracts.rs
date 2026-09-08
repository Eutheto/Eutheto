use eutheto_planning_ir::{
    BoolVariableId, CandidateValues, IntVariableId, PlanningProblem, PlanningProblemSummary,
    SolveFingerprintInput, Variable, routed_solve_fingerprint,
};
use eutheto_types::{
    BackendId, DurationMillis, ParentSolveBudget, SolveBudgetView, SolveOptions, SolveStatus,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

const MAX_DIAGNOSTIC_CODE_BYTES: usize = 96;
const MAX_DIAGNOSTIC_LINE_BYTES: usize = 512;
const MAX_EVIDENCE_ID_BYTES: usize = 160;

const MAX_RUNTIME_VERSION_BYTES: usize = 64;

/// Immutable executable identity captured before a backend is dispatched.
///
/// This operational identity is intentionally separate from [`crate::SolverDescriptor`].
/// `solver_version` is the exact value persisted as `RunInputV1.solver_version`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendRuntimeIdentity {
    backend_id: BackendId,
    backend_version: String,
    adapter_version: String,
    worker_version: String,
    solver_version: String,
    protocol_major: u32,
    protocol_minor: u32,
}

impl BackendRuntimeIdentity {
    /// Constructs a bounded executable identity.
    ///
    /// # Errors
    ///
    /// Returns an error when a version does not satisfy the solver descriptor version grammar or
    /// when the protocol major version is zero.
    pub fn new(
        backend_id: BackendId,
        backend_version: String,
        adapter_version: String,
        worker_version: String,
        solver_version: String,
        protocol_major: u32,
        protocol_minor: u32,
    ) -> Result<Self, BackendRuntimeIdentityError> {
        let identity = Self {
            backend_id,
            backend_version,
            adapter_version,
            worker_version,
            solver_version,
            protocol_major,
            protocol_minor,
        };
        identity.validate()?;
        Ok(identity)
    }

    /// Validates bounded versions and the usable protocol-major invariant.
    ///
    /// # Errors
    ///
    /// Returns the field-specific identity error for the first invalid field.
    pub fn validate(&self) -> Result<(), BackendRuntimeIdentityError> {
        for (value, error) in [
            (
                self.backend_version.as_str(),
                BackendRuntimeIdentityError::InvalidBackendVersion,
            ),
            (
                self.adapter_version.as_str(),
                BackendRuntimeIdentityError::InvalidAdapterVersion,
            ),
            (
                self.worker_version.as_str(),
                BackendRuntimeIdentityError::InvalidWorkerVersion,
            ),
            (
                self.solver_version.as_str(),
                BackendRuntimeIdentityError::InvalidSolverVersion,
            ),
        ] {
            if !valid_runtime_version(value) {
                return Err(error);
            }
        }
        if self.protocol_major == 0 {
            return Err(BackendRuntimeIdentityError::ZeroProtocolMajor);
        }
        Ok(())
    }

    #[must_use]
    pub fn backend_id(&self) -> &BackendId {
        &self.backend_id
    }

    #[must_use]
    pub fn backend_version(&self) -> &str {
        &self.backend_version
    }

    #[must_use]
    pub fn adapter_version(&self) -> &str {
        &self.adapter_version
    }

    #[must_use]
    pub fn worker_version(&self) -> &str {
        &self.worker_version
    }

    /// Exact solver/engine version persisted as `RunInputV1.solver_version`.
    #[must_use]
    pub fn solver_version(&self) -> &str {
        &self.solver_version
    }

    #[must_use]
    pub const fn protocol_major(&self) -> u32 {
        self.protocol_major
    }

    #[must_use]
    pub const fn protocol_minor(&self) -> u32 {
        self.protocol_minor
    }
}

fn valid_runtime_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RUNTIME_VERSION_BYTES
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// Invalid executable identity rejected before backend registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendRuntimeIdentityError {
    InvalidBackendVersion,
    InvalidAdapterVersion,
    InvalidWorkerVersion,
    InvalidSolverVersion,
    ZeroProtocolMajor,
}

impl fmt::Display for BackendRuntimeIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid backend runtime identity: {self:?}")
    }
}

impl std::error::Error for BackendRuntimeIdentityError {}

/// Hard adapter-output ceilings. Zero is rejected because it cannot represent a usable contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolverApiLimits {
    pub max_candidates: u32,
    /// Maximum Boolean and integer assignment entries retained across all candidates, including
    /// the presence and scalar component assignments used to reconstruct intervals.
    pub max_candidate_assignments: u32,
    pub max_progress_events: u32,
    pub max_diagnostic_lines: u32,
    pub max_evidence_refs_per_candidate: u32,
}

impl SolverApiLimits {
    pub const DEFAULT: Self = Self {
        max_candidates: 128,
        max_candidate_assignments: 100_000,
        max_progress_events: 4_096,
        max_diagnostic_lines: 256,
        max_evidence_refs_per_candidate: 64,
    };

    /// Validates that every output ceiling is usable.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidLimits`] if any ceiling is zero.
    pub const fn validate(self) -> Result<Self, OutputError> {
        if self.max_candidates == 0
            || self.max_candidate_assignments == 0
            || self.max_progress_events == 0
            || self.max_diagnostic_lines == 0
            || self.max_evidence_refs_per_candidate == 0
        {
            Err(OutputError::InvalidLimits)
        } else {
            Ok(self)
        }
    }
}

/// Parent-derived backend budget. The child view retains the original absolute deadline.
#[derive(Clone)]
pub struct SolveDispatchBudget {
    parent_view: SolveBudgetView,
    remaining_at_dispatch: DurationMillis,
    backend_limit: DurationMillis,
}

impl SolveDispatchBudget {
    fn from_parent(
        parent: &ParentSolveBudget,
        backend_cap: Option<DurationMillis>,
        acceptance_reserve: DurationMillis,
    ) -> Self {
        let parent_view = parent.phase_view();
        let remaining_at_dispatch = parent_view.snapshot().remaining_milliseconds;
        // Both values derive from this one observation; a prior cap calculation could spend the reserve.
        let available = DurationMillis::new(
            remaining_at_dispatch
                .value()
                .saturating_sub(acceptance_reserve.value()),
        )
        .unwrap_or(DurationMillis::ZERO);
        let backend_limit = backend_cap.map_or(available, |cap| cap.min(available));
        Self {
            parent_view,
            remaining_at_dispatch,
            backend_limit,
        }
    }

    /// Another phase view over the same absolute parent deadline; never a fresh duration.
    #[must_use]
    pub fn child_view(&self) -> SolveBudgetView {
        self.parent_view.phase_view()
    }

    #[must_use]
    pub const fn remaining_at_dispatch(&self) -> DurationMillis {
        self.remaining_at_dispatch
    }

    #[must_use]
    pub const fn backend_limit(&self) -> DurationMillis {
        self.backend_limit
    }

    /// Parent-measured elapsed time since backend dispatch.
    #[must_use]
    pub fn elapsed_milliseconds(&self) -> DurationMillis {
        let remaining = self.parent_view.snapshot().remaining_milliseconds;
        match DurationMillis::new(self.elapsed_milliseconds_value(remaining)) {
            Ok(elapsed) => elapsed,
            Err(_) => DurationMillis::MAX,
        }
    }

    /// Distinguishes cancellation, the parent deadline, and a shorter backend cap.
    #[must_use]
    pub fn stop_reason(&self) -> Option<BackendStopReason> {
        let snapshot = self.parent_view.snapshot();
        if snapshot.cancelled {
            Some(BackendStopReason::Cancelled)
        } else if snapshot.expired || snapshot.remaining_milliseconds == DurationMillis::ZERO {
            Some(BackendStopReason::DeadlineExceeded)
        } else if self.elapsed_milliseconds_value(snapshot.remaining_milliseconds)
            >= self.backend_limit.value()
        {
            Some(BackendStopReason::BackendLimitExceeded)
        } else {
            None
        }
    }

    /// Remaining authoritative backend time derived from the dispatch clock, never backend data.
    #[must_use]
    pub fn remaining_backend_duration(&self) -> Duration {
        let remaining = self.parent_view.snapshot().remaining_milliseconds;
        let elapsed = self.elapsed_milliseconds_value(remaining);
        Duration::from_millis(self.backend_limit.value().saturating_sub(elapsed))
    }

    fn elapsed_milliseconds_value(&self, remaining: DurationMillis) -> u64 {
        self.remaining_at_dispatch
            .value()
            .saturating_sub(remaining.value())
    }
}

/// Why cooperative backend work must stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendStopReason {
    Cancelled,
    DeadlineExceeded,
    BackendLimitExceeded,
}

/// Immutable request delivered to one already-selected backend.
#[derive(Clone)]
pub struct SolveRequest {
    backend_id: BackendId,
    backend_version: String,
    adapter_version: String,
    problem: Arc<PlanningProblem>,
    summary: PlanningProblemSummary,
    options: SolveOptions,
    model_hash: String,
    solve_fingerprint: String,
    dispatch_budget: SolveDispatchBudget,
}

impl SolveRequest {
    /// Constructs a routed request and binds its canonical options to backend and adapter versions.
    /// Acceptance reserve is deducted from the same observation used for dispatch accounting.
    ///
    /// # Errors
    /// Returns a serialization error if canonical `SolveOptions` bytes cannot be produced.
    // The arguments are the complete immutable solve-request contract; grouping them would either
    // obscure that boundary or introduce a second public configuration type with no other use.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend_id: BackendId,
        backend_version: &str,
        adapter_version: &str,
        problem: Arc<PlanningProblem>,
        summary: PlanningProblemSummary,
        options: SolveOptions,
        parent_budget: &ParentSolveBudget,
        backend_cap: Option<DurationMillis>,
        acceptance_reserve: DurationMillis,
    ) -> Result<Self, serde_json::Error> {
        let model_hash = summary.canonical_ir_hash.clone();
        let canonical_options = serde_json::to_vec(&options)?;
        let solve_fingerprint = routed_solve_fingerprint(&SolveFingerprintInput {
            canonical_ir_hash: model_hash.clone(),
            backend_id: backend_id.as_str().to_owned(),
            backend_version: backend_version.to_owned(),
            adapter_version: adapter_version.to_owned(),
            canonical_options,
        });
        Ok(Self {
            backend_id,
            backend_version: backend_version.to_owned(),
            adapter_version: adapter_version.to_owned(),
            problem,
            summary,
            options,
            model_hash,
            solve_fingerprint,
            dispatch_budget: SolveDispatchBudget::from_parent(
                parent_budget,
                backend_cap,
                acceptance_reserve,
            ),
        })
    }

    #[must_use]
    pub fn backend_id(&self) -> &BackendId {
        &self.backend_id
    }

    #[must_use]
    pub fn backend_version(&self) -> &str {
        &self.backend_version
    }

    #[must_use]
    pub fn adapter_version(&self) -> &str {
        &self.adapter_version
    }

    #[must_use]
    pub fn problem(&self) -> &Arc<PlanningProblem> {
        &self.problem
    }

    #[must_use]
    pub const fn summary(&self) -> &PlanningProblemSummary {
        &self.summary
    }

    #[must_use]
    pub const fn options(&self) -> &SolveOptions {
        &self.options
    }

    #[must_use]
    pub fn model_hash(&self) -> &str {
        &self.model_hash
    }

    #[must_use]
    pub fn solve_fingerprint(&self) -> &str {
        &self.solve_fingerprint
    }

    #[must_use]
    pub const fn dispatch_budget(&self) -> &SolveDispatchBudget {
        &self.dispatch_budget
    }
}

/// Bounded safe diagnostic line. Construction rejects controls and multiline content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SafeDiagnosticLine {
    code: String,
    message: String,
}

impl SafeDiagnosticLine {
    /// Creates a bounded diagnostic line.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidDiagnosticCode`] if `code` is empty, too long, or contains
    /// characters outside the diagnostic-code alphabet. Returns
    /// [`OutputError::UnsafeDiagnosticLine`] if `message` is empty, too long, multiline, or
    /// contains control characters.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Result<Self, OutputError> {
        let code = code.into();
        let message = message.into();
        if code.is_empty()
            || code.len() > MAX_DIAGNOSTIC_CODE_BYTES
            || !code.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(OutputError::InvalidDiagnosticCode);
        }
        if message.is_empty()
            || message.len() > MAX_DIAGNOSTIC_LINE_BYTES
            || message.chars().any(char::is_control)
        {
            return Err(OutputError::UnsafeDiagnosticLine);
        }
        Ok(Self { code, message })
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl<'de> Deserialize<'de> for SafeDiagnosticLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            code: String,
            message: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::new(raw.code, raw.message).map_err(serde::de::Error::custom)
    }
}

/// Bounded opaque reference to adapter or verifier evidence kept outside normal results.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct BackendEvidenceRef(String);

impl BackendEvidenceRef {
    /// Creates a bounded dotted evidence reference.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidEvidenceRef`] if the value is not a bounded, lowercase,
    /// dotted identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, OutputError> {
        let value = value.into();
        if value.len() >= 3
            && value.len() <= MAX_EVIDENCE_ID_BYTES
            && value.split('.').count() >= 2
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'_' | b'-')
                    })
            })
        {
            Ok(Self(value))
        } else {
            Err(OutputError::InvalidEvidenceRef)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BackendEvidenceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Backend-native objective and bound evidence. It is never an authoritative domain score.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendObjectiveEvidence {
    pub objective_values: Vec<i64>,
    pub best_bound_values: Option<Vec<i64>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelReductionSummary {
    pub variables_removed: u64,
    pub constraints_removed: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncumbentSummary {
    pub sequence: u32,
    pub observed_after_milliseconds: DurationMillis,
    pub objective: Option<BackendObjectiveEvidence>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundSummary {
    pub observed_after_milliseconds: DurationMillis,
    pub bound_values: Vec<i64>,
}

/// Final application-level completion only. A backend cannot emit this before verification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolveCompletionSummary {
    pub status: SolveStatus,
    pub accepted_candidate_count: u32,
}

/// Truthful detailed progress. Percent is optional and validated before emission.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum SolveProgressEvent {
    Queued,
    Compiling { phase: String, percent: Option<f32> },
    BackendStarted { backend: BackendId },
    PresolveSummary(ModelReductionSummary),
    IncumbentFound(IncumbentSummary),
    BoundImproved(BoundSummary),
    LogLine(SafeDiagnosticLine),
    Verifying,
    Explaining,
    Completed(SolveCompletionSummary),
}

/// Synchronous callback boundary; adapters remain free to run asynchronously.
pub trait ProgressSink: Send {
    /// Delivers one validated progress event.
    ///
    /// # Errors
    ///
    /// Returns an error if the downstream consumer cannot accept the event.
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), OutputError>;
}

/// Candidate submitted by a backend before sequence assignment and independent verification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateSubmission {
    pub values: CandidateValues,
    pub observed_after_milliseconds: DurationMillis,
    pub objective: Option<BackendObjectiveEvidence>,
    pub evidence_refs: Vec<BackendEvidenceRef>,
}

/// Accepted adapter output after strict model-domain and volume validation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendCandidate {
    pub sequence: u32,
    pub values: CandidateValues,
    pub observed_after_milliseconds: DurationMillis,
    pub objective: Option<BackendObjectiveEvidence>,
    pub evidence_refs: Vec<BackendEvidenceRef>,
}

/// Backend termination reason, kept distinct from application acceptance status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendTerminationReason {
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

/// Parent-measured adapter and worker lifecycle spans for one backend invocation.
///
/// Worker-native solver timings are kept separately because they come from an untrusted backend
/// clock and must not be used to enforce the parent deadline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendTimingEvidence {
    /// Planning-IR translation plus bounded request framing and streaming.
    pub translation_serialization_milliseconds: DurationMillis,
    pub worker_startup_milliseconds: Option<DurationMillis>,
    pub handshake_milliseconds: Option<DurationMillis>,
    pub solver_milliseconds: Option<DurationMillis>,
    pub protocol_decode_milliseconds: Option<DurationMillis>,
}

/// Redacted Planning-IR and translated-backend model sizes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendModelCountEvidence {
    pub planning_variable_count: u64,
    pub planning_constraint_count: u64,
    pub translated_variable_count: u64,
    pub translated_constraint_count: u64,
}

/// Largest worker statistic exactly representable by every JSON/TypeScript client.
pub const BACKEND_STATISTIC_MAX_V1: u64 = 9_007_199_254_740_991;

/// Bounded worker-native timing and search counters.
///
/// These values are diagnostic quality evidence only. They are not authoritative scores, proof,
/// or parent-deadline measurements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendWorkerStatistics {
    pub wall_time_milliseconds: Option<DurationMillis>,
    pub user_time_milliseconds: Option<DurationMillis>,
    pub deterministic_time_milliseconds: Option<DurationMillis>,
    pub conflicts: Option<u64>,
    pub branches: Option<u64>,
    pub binary_propagations: Option<u64>,
    pub integer_propagations: Option<u64>,
}

/// Fully resolved worker allowlist values for one dispatch.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendAppliedParameterEvidence {
    pub wall_time_milliseconds: Option<DurationMillis>,
    pub memory_limit_bytes: Option<u64>,
    pub worker_threads: u32,
    pub random_seed: i32,
    pub stop_after_first_feasible: bool,
    pub emit_intermediate_solutions: bool,
    pub log_search_progress: bool,
    pub deterministic_test_profile: bool,
}

/// Complete reproducibility inputs and corroborated worker hashes for one invocation.
///
/// `applied_parameters_sha256` is absent when the worker failed before a corroborated terminal
/// parameters hash. Candidate assignments and domain acceptance never enter this metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendReproducibilityEvidence {
    pub backend_version: String,
    pub adapter_version: String,
    pub worker_version: String,
    pub engine_version: String,
    pub protocol_major: u32,
    pub protocol_minor: u32,
    pub applied_options: SolveOptions,
    pub applied_parameters: BackendAppliedParameterEvidence,
    pub model_fingerprint_sha256: String,
    pub applied_parameters_sha256: Option<String>,
}

/// Backend-specific execution evidence exposed without granting result authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendExecutionEvidence {
    pub timings: BackendTimingEvidence,
    pub model_counts: BackendModelCountEvidence,
    pub worker_statistics: Option<BackendWorkerStatistics>,
    pub reproducibility: BackendReproducibilityEvidence,
}

/// Bounded timing/evidence returned by one backend invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendTerminationEvidence {
    pub remaining_at_dispatch_milliseconds: DurationMillis,
    pub backend_limit_milliseconds: DurationMillis,
    pub elapsed_milliseconds: DurationMillis,
    /// Parent-observed time of the first bounded backend candidate.
    ///
    /// This is pre-verification evidence and never means the candidate was accepted.
    pub first_incumbent_milliseconds: Option<DurationMillis>,
    pub objective: Option<BackendObjectiveEvidence>,
    pub evidence_refs: Vec<BackendEvidenceRef>,
    /// Optional adapter-specific operational evidence; it conveys no solver-result authority.
    pub execution: Option<BackendExecutionEvidence>,
}

/// Backend outcome before projection or independent verification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendSolveOutcome {
    pub backend_id: BackendId,
    pub model_hash: String,
    pub solve_fingerprint: String,
    pub termination: BackendTerminationReason,
    pub evidence: BackendTerminationEvidence,
}

impl BackendSolveOutcome {
    /// A safe normalized status only when no raw candidate must first be independently verified.
    #[must_use]
    pub const fn conclusive_status_without_candidate(&self) -> Option<SolveStatus> {
        match self.termination {
            BackendTerminationReason::InfeasibilityClaimed => Some(SolveStatus::Infeasible),
            BackendTerminationReason::UnboundedClaimed => Some(SolveStatus::Unbounded),
            BackendTerminationReason::TimeLimit | BackendTerminationReason::SolutionLimit => {
                Some(SolveStatus::NoSolutionWithinLimit)
            }
            BackendTerminationReason::Cancelled => Some(SolveStatus::Cancelled),
            BackendTerminationReason::InvalidModel => Some(SolveStatus::InvalidModel),
            BackendTerminationReason::Unavailable => Some(SolveStatus::BackendUnavailable),
            BackendTerminationReason::Failed => Some(SolveStatus::BackendFailed),
            BackendTerminationReason::OptimalityClaimed
            | BackendTerminationReason::CandidateFound => None,
        }
    }
}

/// Full bounded adapter return assembled by the caller from the outcome and output sink.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendSolveResult {
    pub outcome: BackendSolveOutcome,
    pub candidates: Vec<BackendCandidate>,
    pub progress_event_count: u32,
}

/// Hard-limited sink passed to a backend. It rejects malformed candidate values before storage.
pub struct BoundedBackendOutput<'a> {
    problem: &'a PlanningProblem,
    progress: &'a mut dyn ProgressSink,
    dispatch_budget: SolveDispatchBudget,
    limits: SolverApiLimits,
    candidates: Vec<BackendCandidate>,
    progress_event_count: u32,
    candidate_assignment_count: u32,
    contract_rejection: Option<OutputError>,
    diagnostic_line_count: u32,
}

impl<'a> BoundedBackendOutput<'a> {
    /// Creates an empty bounded output collector.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidLimits`] if any configured output ceiling is zero.
    pub fn new(
        problem: &'a PlanningProblem,
        progress: &'a mut dyn ProgressSink,
        dispatch_budget: &SolveDispatchBudget,
        limits: SolverApiLimits,
    ) -> Result<Self, OutputError> {
        Ok(Self {
            problem,
            progress,
            dispatch_budget: dispatch_budget.clone(),
            limits: limits.validate()?,
            candidates: Vec::new(),
            candidate_assignment_count: 0,
            contract_rejection: None,
            progress_event_count: 0,
            diagnostic_line_count: 0,
        })
    }

    #[must_use]
    pub fn candidates(&self) -> &[BackendCandidate] {
        &self.candidates
    }

    #[must_use]
    pub const fn candidate_assignment_count(&self) -> u32 {
        self.candidate_assignment_count
    }

    #[must_use]
    pub const fn progress_event_count(&self) -> u32 {
        self.progress_event_count
    }

    #[must_use]
    pub fn into_result(self, outcome: BackendSolveOutcome) -> BackendSolveResult {
        BackendSolveResult {
            outcome,
            candidates: self.candidates,
            progress_event_count: self.progress_event_count,
        }
    }

    /// Consumes the collector without cloning retained candidate assignments.
    #[must_use]
    pub fn into_candidates(self) -> Vec<BackendCandidate> {
        self.candidates
    }
}

/// Only this bounded interface is exposed to backend implementations.
pub trait BackendOutputSink: Send {
    /// Validates and delivers one backend progress event.
    ///
    /// # Errors
    ///
    /// Returns an error if the event violates the progress contract, exceeds an output ceiling,
    /// or is rejected by the downstream progress sink.
    fn emit_progress(&mut self, event: SolveProgressEvent) -> Result<(), OutputError>;
    /// Validates and stores one backend candidate.
    ///
    /// # Errors
    ///
    /// Returns an error if the candidate violates the planning problem or output contract, or if
    /// accepting it would exceed an output ceiling.
    fn submit_candidate(
        &mut self,
        candidate: CandidateSubmission,
    ) -> Result<BackendCandidate, OutputError>;
}

impl BackendOutputSink for BoundedBackendOutput<'_> {
    fn emit_progress(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        if let Some(error) = self.budget_error() {
            return self.reject(error);
        }
        validate_progress_event(&event, self.problem.objectives.levels.len())?;
        if self.progress_event_count >= self.limits.max_progress_events {
            return Err(OutputError::ProgressLimitExceeded);
        }
        if matches!(event, SolveProgressEvent::LogLine(_)) {
            if self.diagnostic_line_count >= self.limits.max_diagnostic_lines {
                return Err(OutputError::DiagnosticLimitExceeded);
            }
            self.diagnostic_line_count += 1;
        }
        self.progress.emit(event)?;
        self.progress_event_count += 1;
        Ok(())
    }

    fn submit_candidate(
        &mut self,
        candidate: CandidateSubmission,
    ) -> Result<BackendCandidate, OutputError> {
        if let Some(error) = self.budget_error() {
            return self.reject(error);
        }
        if self.candidates.len() >= self.limits.max_candidates as usize {
            return self.reject(OutputError::CandidateLimitExceeded);
        }
        if candidate.evidence_refs.len() > self.limits.max_evidence_refs_per_candidate as usize {
            return self.reject(OutputError::EvidenceLimitExceeded);
        }
        let Some(assignment_count) = candidate
            .values
            .booleans
            .len()
            .checked_add(candidate.values.integers.len())
            .and_then(|count| u32::try_from(count).ok())
        else {
            return self.reject(OutputError::CandidateAssignmentLimitExceeded);
        };
        let next_assignment_count = self
            .candidate_assignment_count
            .checked_add(assignment_count)
            .filter(|count| *count <= self.limits.max_candidate_assignments)
            .ok_or(OutputError::CandidateAssignmentLimitExceeded);
        let next_assignment_count = match next_assignment_count {
            Ok(count) => count,
            Err(error) => return self.reject(error),
        };
        validate_objective_evidence(
            candidate.objective.as_ref(),
            self.problem.objectives.levels.len(),
        )?;
        if self.candidates.last().is_some_and(|previous| {
            candidate.observed_after_milliseconds < previous.observed_after_milliseconds
        }) {
            return Err(OutputError::NonMonotonicCandidateTime);
        }
        validate_candidate_values(self.problem, &candidate.values)?;
        let sequence = u32::try_from(self.candidates.len() + 1)
            .map_err(|_| OutputError::CandidateLimitExceeded)?;
        let accepted = BackendCandidate {
            sequence,
            values: candidate.values,
            observed_after_milliseconds: candidate.observed_after_milliseconds,
            objective: candidate.objective,
            evidence_refs: candidate.evidence_refs,
        };
        let returned = accepted.clone();
        if let Some(error) = self.budget_error() {
            return self.reject(error);
        }
        self.candidate_assignment_count = next_assignment_count;
        self.candidates.push(accepted);
        Ok(returned)
    }
}

impl BoundedBackendOutput<'_> {
    fn budget_error(&self) -> Option<OutputError> {
        match self.dispatch_budget.stop_reason()? {
            BackendStopReason::Cancelled => Some(OutputError::Cancelled),
            BackendStopReason::DeadlineExceeded => Some(OutputError::ParentDeadlineExceeded),
            BackendStopReason::BackendLimitExceeded => Some(OutputError::BackendLimitExceeded),
        }
    }

    fn reject<T>(&mut self, error: OutputError) -> Result<T, OutputError> {
        if self.contract_rejection.is_none() {
            self.contract_rejection = Some(error.clone());
        }
        Err(error)
    }

    #[must_use]
    pub fn contract_rejection(&self) -> Option<&OutputError> {
        self.contract_rejection.as_ref()
    }
}

fn validate_progress_event(
    event: &SolveProgressEvent,
    objective_level_count: usize,
) -> Result<(), OutputError> {
    match event {
        SolveProgressEvent::Verifying
        | SolveProgressEvent::Explaining
        | SolveProgressEvent::Completed(_) => {
            return Err(OutputError::BackendProgressAuthorityViolation);
        }
        SolveProgressEvent::Compiling { phase, percent } => {
            if phase.is_empty() || phase.len() > 96 || phase.chars().any(char::is_control) {
                return Err(OutputError::InvalidProgressPhase);
            }
            if percent.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
                return Err(OutputError::InvalidProgressPercent);
            }
        }
        SolveProgressEvent::IncumbentFound(summary) => {
            validate_objective_evidence(summary.objective.as_ref(), objective_level_count)?;
        }
        SolveProgressEvent::BoundImproved(summary) => {
            if summary.bound_values.len() != objective_level_count {
                return Err(OutputError::InvalidObjectiveDimension);
            }
        }
        SolveProgressEvent::Queued
        | SolveProgressEvent::BackendStarted { .. }
        | SolveProgressEvent::PresolveSummary(_)
        | SolveProgressEvent::LogLine(_) => {}
    }
    Ok(())
}

fn validate_objective_evidence(
    evidence: Option<&BackendObjectiveEvidence>,
    objective_level_count: usize,
) -> Result<(), OutputError> {
    if evidence.is_some_and(|value| {
        value.objective_values.len() != objective_level_count
            || value
                .best_bound_values
                .as_ref()
                .is_some_and(|bounds| bounds.len() != objective_level_count)
    }) {
        return Err(OutputError::InvalidObjectiveDimension);
    }
    Ok(())
}

/// Strictly validates a complete typed assignment against the immutable planning problem.
///
/// # Errors
///
/// Returns an error if a Boolean or integer assignment is unknown or missing, or if an integer
/// assignment lies outside its declared domain.
pub fn validate_candidate_values(
    problem: &PlanningProblem,
    values: &CandidateValues,
) -> Result<(), OutputError> {
    let mut expected_bools = BTreeSet::new();
    let mut expected_ints = BTreeSet::new();
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(value) => {
                expected_bools.insert(value.id.clone());
            }
            Variable::Integer(value) => {
                expected_ints.insert(value.id.clone());
                if values
                    .integers
                    .get(&value.id)
                    .is_some_and(|candidate| !value.domain.contains(*candidate))
                {
                    return Err(OutputError::OutOfDomain(value.id.clone()));
                }
            }
            Variable::Interval(_) => {}
        }
    }
    if let Some(id) = values
        .booleans
        .keys()
        .find(|id| !expected_bools.contains(*id))
    {
        return Err(OutputError::UnknownBoolean(id.clone()));
    }
    if let Some(id) = values
        .integers
        .keys()
        .find(|id| !expected_ints.contains(*id))
    {
        return Err(OutputError::UnknownInteger(id.clone()));
    }
    if let Some(id) = expected_bools
        .iter()
        .find(|id| !values.booleans.contains_key(*id))
    {
        return Err(OutputError::MissingBoolean(id.clone()));
    }
    if let Some(id) = expected_ints
        .iter()
        .find(|id| !values.integers.contains_key(*id))
    {
        return Err(OutputError::MissingInteger(id.clone()));
    }
    Ok(())
}

/// Bounded backend contract failure, never a domain result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError {
    code: String,
    message: String,
}

impl BackendError {
    /// Creates a bounded backend error from a diagnostic code and message.
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`SafeDiagnosticLine::new`] when either field
    /// violates the diagnostic-line contract.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Result<Self, OutputError> {
        let line = SafeDiagnosticLine::new(code, message)?;
        Ok(Self {
            code: line.code,
            message: line.message,
        })
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BackendError {}

impl From<OutputError> for BackendError {
    fn from(error: OutputError) -> Self {
        Self {
            code: "solver.output_contract".to_owned(),
            message: error.to_string(),
        }
    }
}

/// Object-safe future returned by [`SolverBackend::solve`].
pub type BackendSolveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<BackendSolveOutcome, BackendError>> + Send + 'a>>;

/// Stable object-safe backend interface. Selection, fallback, and verification live elsewhere.
pub trait SolverBackend: Send + Sync {
    fn descriptor(&self) -> &crate::SolverDescriptor;

    fn runtime_identity(&self) -> &BackendRuntimeIdentity;

    fn compatibility(
        &self,
        problem: &PlanningProblemSummary,
        options: &SolveOptions,
    ) -> crate::CompatibilityReport;

    fn solve<'a>(
        &'a self,
        request: &'a SolveRequest,
        output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a>;
}

/// Output/candidate contract violation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputError {
    InvalidLimits,
    InvalidDiagnosticCode,
    UnsafeDiagnosticLine,
    InvalidEvidenceRef,
    InvalidProgressPhase,
    InvalidProgressPercent,
    ProgressLimitExceeded,
    DiagnosticLimitExceeded,
    CandidateLimitExceeded,
    CandidateAssignmentLimitExceeded,
    Cancelled,
    ParentDeadlineExceeded,
    BackendLimitExceeded,
    EvidenceLimitExceeded,
    InvalidObjectiveDimension,
    NonMonotonicCandidateTime,
    BackendProgressAuthorityViolation,
    UnknownBoolean(BoolVariableId),
    UnknownInteger(IntVariableId),
    MissingBoolean(BoolVariableId),
    MissingInteger(IntVariableId),
    OutOfDomain(IntVariableId),
    DownstreamProgressRejected,
}

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "solver output contract error: {self:?}")
    }
}

impl std::error::Error for OutputError {}
