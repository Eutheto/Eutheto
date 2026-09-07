use crate::decision::{DecisionStatus, RouterDiagnostic, RouterPolicy, RoutingDecision};
use eutheto_planning_ir::{PlanningProblem, PlanningProblemSummary};
use eutheto_solver_api::{
    BackendCandidate, BackendError, BackendSolveOutcome, BackendStopReason,
    BackendTerminationReason, BoundedBackendOutput, CompatibilityReport, OutcomeError, OutputError,
    PreflightError, ProgressSink, SolveProgressEvent, SolveRequest, SolverApiLimits, SolverBackend,
    SolverRegistry, preflight, validate_outcome,
};
use eutheto_types::{
    BackendId, BackendSelection, DurationMillis, OperationInterruption, ParentSolveBudget,
    SolveOptions, SolveStatus,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// External verification decision. The router never manufactures this evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateReview {
    AwaitingIndependentVerification,
    Verified { objective_matches: bool },
    VerificationFailed { diagnostic_code: String },
    Interrupted(OperationInterruption),
}

/// Authority injected by the application layer after projection and independent verification exist.
pub trait CandidateReviewer: Send {
    fn review(&mut self, backend_id: &BackendId, candidate: &BackendCandidate) -> CandidateReview;
}

/// Safe Phase-02 reviewer: it never accepts raw backend output.
pub struct RequireIndependentVerification;

impl CandidateReviewer for RequireIndependentVerification {
    fn review(
        &mut self,
        _backend_id: &BackendId,
        _candidate: &BackendCandidate,
    ) -> CandidateReview {
        CandidateReview::AwaitingIndependentVerification
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendInvocationCounter {
    value: u32,
}

impl BackendInvocationCounter {
    #[must_use]
    pub const fn value(self) -> u32 {
        self.value
    }

    fn mark_invocation(&mut self) -> u32 {
        self.value = self.value.saturating_add(1);
        self.value
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "termination", content = "details")]
pub enum AttemptTermination {
    PreflightRejected { code: String },
    BackendError { code: String },
    OutcomeContractFailure { code: String },
    BackendOutcome(BackendTerminationReason),
    CandidateAwaitingVerification,
    CandidateVerified,
    VerificationQuarantined { diagnostic_code: String },
    ReviewCancelled,
    ReviewDeadlineExceeded,
    ParentCancelled,
    ParentDeadlineExceeded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptRecord {
    pub backend_id: BackendId,
    pub backend_version: String,
    pub adapter_version: String,
    pub solve_fingerprint: String,
    /// Present only after preflight succeeds and immediately before `SolverBackend::solve`.
    pub invocation_index: Option<u32>,
    pub remaining_at_dispatch_milliseconds: DurationMillis,
    pub backend_limit_milliseconds: DurationMillis,
    pub preflight_compatibility: Option<CompatibilityReport>,
    pub candidate_count: u32,
    pub termination: AttemptTermination,
    /// Safe retained backend error code, including failures that followed candidate emission.
    pub backend_failure_code: Option<String>,
    /// Present only after the complete backend outcome passes the request/output contract.
    pub outcome: Option<BackendSolveOutcome>,
    pub fallback_eligible: bool,
    pub fallback_taken: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionTerminalReason {
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
    ParentDeadlineExceeded,
    Cancelled,
}

/// Complete bounded record of routing and all sequential attempts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouterExecutionRecord {
    pub decision: RoutingDecision,
    pub solve_options: SolveOptions,
    pub attempts: Vec<AttemptRecord>,
    pub invocation_count: u32,
    pub terminal_status: SolveStatus,
    pub terminal_reason: ExecutionTerminalReason,
    pub selected_candidate: Option<BackendCandidate>,
    /// Elapsed time from router entry to the first independently verified feasible candidate.
    pub first_verified_feasible_milliseconds: Option<DurationMillis>,
    pub diagnostics: Vec<RouterDiagnostic>,
}

pub struct SolverRouter<'a> {
    registry: &'a SolverRegistry,
    policy: RouterPolicy,
    limits: SolverApiLimits,
}

struct ExecutionState {
    decision: RoutingDecision,
    solve_options: SolveOptions,
    attempts: Vec<AttemptRecord>,
    counter: BackendInvocationCounter,
    diagnostics: Vec<RouterDiagnostic>,
}

impl ExecutionState {
    fn new(decision: RoutingDecision, solve_options: SolveOptions) -> Self {
        let diagnostics = decision.diagnostics.clone();
        Self {
            decision,
            solve_options,
            attempts: Vec::new(),
            counter: BackendInvocationCounter::default(),
            diagnostics,
        }
    }

    fn finish(self, policy: RouterPolicy, terminal: Terminal) -> RouterExecutionRecord {
        RouterExecutionRecord {
            decision: self.decision,
            solve_options: self.solve_options,
            attempts: self.attempts,
            invocation_count: self.counter.value(),
            terminal_status: terminal.status,
            terminal_reason: terminal.reason,
            selected_candidate: terminal.selected_candidate,
            first_verified_feasible_milliseconds: None,
            diagnostics: policy.bounded_diagnostics(self.diagnostics),
        }
    }
}

struct Terminal {
    status: SolveStatus,
    reason: ExecutionTerminalReason,
    selected_candidate: Option<BackendCandidate>,
}

impl Terminal {
    const fn new(status: SolveStatus, reason: ExecutionTerminalReason) -> Self {
        Self {
            status,
            reason,
            selected_candidate: None,
        }
    }

    fn with_candidate(
        status: SolveStatus,
        reason: ExecutionTerminalReason,
        selected_candidate: Option<BackendCandidate>,
    ) -> Self {
        Self {
            status,
            reason,
            selected_candidate,
        }
    }
}

enum AttemptControl {
    Continue,
    Finish(Terminal),
}

struct AttemptInputs<'a> {
    problem: &'a Arc<PlanningProblem>,
    summary: &'a PlanningProblemSummary,
    parent_budget: &'a ParentSolveBudget,
    progress: &'a mut dyn ProgressSink,
    reviewer: &'a mut dyn CandidateReviewer,
    position: usize,
    order_len: usize,
}

impl AttemptInputs<'_> {
    fn solve_request(
        &self,
        backend_id: &BackendId,
        backend: &dyn SolverBackend,
        solve_options: &SolveOptions,
        profile: Option<super::profile::RoutingProfile>,
    ) -> Option<SolveRequest> {
        let mut attempt_options = solve_options.clone();
        attempt_options.backend = BackendSelection::Specific(backend_id.clone());
        SolveRequest::new(
            backend_id.clone(),
            &backend.descriptor().version,
            &backend.descriptor().adapter_version,
            Arc::clone(self.problem),
            self.summary.clone(),
            attempt_options,
            self.parent_budget,
            profile.map(super::profile::RoutingProfile::backend_cap),
        )
        .ok()
    }
}

#[derive(Clone, Copy)]
struct RemainingOutput {
    candidates: u32,
    candidate_assignments: u32,
    progress_events: u32,
    diagnostic_lines: u32,
}

impl RemainingOutput {
    const fn new(limits: SolverApiLimits) -> Self {
        Self {
            candidates: limits.max_candidates,
            candidate_assignments: limits.max_candidate_assignments,
            progress_events: limits.max_progress_events,
            diagnostic_lines: limits.max_diagnostic_lines,
        }
    }

    const fn exhausted(self) -> bool {
        self.candidates == 0
            || self.candidate_assignments == 0
            || self.progress_events == 0
            || self.diagnostic_lines == 0
    }

    const fn limits(self, configured: SolverApiLimits) -> SolverApiLimits {
        SolverApiLimits {
            max_candidates: self.candidates,
            max_candidate_assignments: self.candidate_assignments,
            max_progress_events: self.progress_events,
            max_diagnostic_lines: self.diagnostic_lines,
            max_evidence_refs_per_candidate: configured.max_evidence_refs_per_candidate,
        }
    }

    fn consume(&mut self, run: &BackendRun) {
        self.candidates = self
            .candidates
            .saturating_sub(candidate_count(&run.candidates));
        self.candidate_assignments = self
            .candidate_assignments
            .saturating_sub(run.candidate_assignments_used);
        self.progress_events = self.progress_events.saturating_sub(run.progress_used);
        self.diagnostic_lines = self.diagnostic_lines.saturating_sub(run.diagnostic_used);
    }
}

struct AttemptBase {
    backend_id: BackendId,
    backend_version: String,
    adapter_version: String,
    solve_fingerprint: String,
    invocation_index: Option<u32>,
    remaining_at_dispatch_milliseconds: DurationMillis,
    backend_limit_milliseconds: DurationMillis,
    preflight_compatibility: Option<CompatibilityReport>,
}

impl AttemptBase {
    fn new(
        backend_id: &BackendId,
        request: &SolveRequest,
        invocation_index: u32,
        preflight_report: CompatibilityReport,
    ) -> Self {
        Self {
            backend_id: backend_id.clone(),
            backend_version: request.backend_version().to_owned(),
            adapter_version: request.adapter_version().to_owned(),
            solve_fingerprint: request.solve_fingerprint().to_owned(),
            invocation_index: Some(invocation_index),
            remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
            backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
            preflight_compatibility: Some(preflight_report),
        }
    }

    fn finish(self, conclusion: AttemptConclusion) -> AttemptRecord {
        AttemptRecord {
            backend_id: self.backend_id,
            backend_version: self.backend_version,
            adapter_version: self.adapter_version,
            solve_fingerprint: self.solve_fingerprint,
            invocation_index: self.invocation_index,
            remaining_at_dispatch_milliseconds: self.remaining_at_dispatch_milliseconds,
            backend_limit_milliseconds: self.backend_limit_milliseconds,
            preflight_compatibility: self.preflight_compatibility,
            candidate_count: conclusion.candidate_count,
            termination: conclusion.termination,
            backend_failure_code: conclusion.backend_failure_code,
            outcome: conclusion.outcome,
            fallback_eligible: conclusion.fallback_eligible,
            fallback_taken: conclusion.fallback_taken,
        }
    }
}

struct AttemptConclusion {
    candidate_count: u32,
    termination: AttemptTermination,
    backend_failure_code: Option<String>,
    outcome: Option<BackendSolveOutcome>,
    fallback_eligible: bool,
    fallback_taken: bool,
}

enum BackendCompletion {
    Returned(Result<Box<BackendSolveOutcome>, BackendError>),
    TimedOut,
}

struct BackendRun {
    completion: BackendCompletion,
    stop_reason: Option<BackendStopReason>,
    candidates: Vec<BackendCandidate>,
    candidate_assignments_used: u32,
    progress_used: u32,
    diagnostic_used: u32,
}

struct CandidateDisposition {
    termination: AttemptTermination,
    status: SolveStatus,
    reason: ExecutionTerminalReason,
    selected_candidate: Option<BackendCandidate>,
    diagnostic: Option<RouterDiagnostic>,
}

impl<'a> SolverRouter<'a> {
    #[must_use]
    pub fn new(registry: &'a SolverRegistry) -> Self {
        Self {
            registry,
            policy: RouterPolicy::default(),
            limits: SolverApiLimits::DEFAULT,
        }
    }

    #[must_use]
    pub const fn with_policy(mut self, policy: RouterPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Overrides adapter output limits while preserving one shared ceiling across fallbacks.
    ///
    /// # Errors
    /// Returns the solver API output error if any limit is zero.
    pub fn with_limits(mut self, limits: SolverApiLimits) -> Result<Self, OutputError> {
        self.limits = limits.validate()?;
        Ok(self)
    }

    #[must_use]
    pub fn decide(&self, problem: &PlanningProblem, options: &SolveOptions) -> RoutingDecision {
        self.policy.decide(self.registry, problem, options)
    }

    /// Runs at most one backend at a time. Every attempt derives its dispatch view from the same
    /// absolute parent budget; no fallback creates a deadline.
    pub async fn execute(
        &self,
        problem: Arc<PlanningProblem>,
        options: SolveOptions,
        parent_budget: &ParentSolveBudget,
        progress: &mut dyn ProgressSink,
        reviewer: &mut dyn CandidateReviewer,
    ) -> RouterExecutionRecord {
        let initial_remaining = parent_budget.snapshot().remaining_milliseconds;
        let decision = self.decide(&problem, &options);
        let mut state = ExecutionState::new(decision, options);
        if let Some(terminal) = initial_terminal(state.decision.status) {
            return state.finish(self.policy, terminal);
        }
        let Some(summary) = state.decision.summary.clone() else {
            return state.finish(
                self.policy,
                Terminal::new(
                    SolveStatus::InvalidModel,
                    ExecutionTerminalReason::InvalidModel,
                ),
            );
        };
        let order = backend_order(&state.decision);
        let mut remaining_output = RemainingOutput::new(self.limits);

        for (position, backend_id) in order.iter().enumerate() {
            let inputs = AttemptInputs {
                problem: &problem,
                summary: &summary,
                parent_budget,
                progress,
                reviewer,
                position,
                order_len: order.len(),
            };
            match self
                .run_attempt(backend_id, &mut state, &mut remaining_output, inputs)
                .await
            {
                AttemptControl::Continue => {}
                AttemptControl::Finish(terminal) => {
                    let verified = terminal.reason == ExecutionTerminalReason::CandidateVerified;
                    let mut record = state.finish(self.policy, terminal);
                    if verified {
                        record.first_verified_feasible_milliseconds = Some(elapsed_since(
                            initial_remaining,
                            parent_budget.snapshot().remaining_milliseconds,
                        ));
                    }
                    return record;
                }
            }
        }

        state.finish(
            self.policy,
            Terminal::new(
                SolveStatus::BackendUnavailable,
                ExecutionTerminalReason::NoCompatibleBackend,
            ),
        )
    }

    async fn run_attempt(
        &self,
        backend_id: &BackendId,
        state: &mut ExecutionState,
        remaining_output: &mut RemainingOutput,
        inputs: AttemptInputs<'_>,
    ) -> AttemptControl {
        if let Some(terminal) = pre_attempt_terminal(inputs.parent_budget, *remaining_output) {
            return AttemptControl::Finish(terminal);
        }
        let Some(backend) = self.registry.get(backend_id) else {
            state
                .diagnostics
                .push(RouterDiagnostic::backend_code("solver.registry_lookup"));
            return AttemptControl::Finish(Terminal::new(
                SolveStatus::BackendUnavailable,
                ExecutionTerminalReason::BackendFailure,
            ));
        };
        let request = inputs.solve_request(
            backend_id,
            backend.as_ref(),
            &state.solve_options,
            state.decision.profile,
        );
        let Some(request) = request else {
            state
                .diagnostics
                .push(RouterDiagnostic::backend_code("solver.request_fingerprint"));
            return AttemptControl::Finish(Terminal::new(
                SolveStatus::InvalidModel,
                ExecutionTerminalReason::PreflightRejected,
            ));
        };
        let preflight_report =
            match preflight(self.registry.matrix(), backend.descriptor(), &request) {
                Ok(report) => report,
                Err(error) => {
                    state
                        .attempts
                        .push(preflight_attempt(backend_id, &request, &error));
                    let (status, reason) = preflight_terminal(&error);
                    return AttemptControl::Finish(Terminal::new(status, reason));
                }
            };
        let invocation_index = state.counter.mark_invocation();
        let attempt_limits = remaining_output.limits(self.limits);
        let attempt = AttemptBase::new(backend_id, &request, invocation_index, preflight_report);
        let backend_run = run_backend(
            backend.as_ref(),
            &request,
            inputs.problem,
            &mut *inputs.progress,
            attempt_limits,
        )
        .await;
        let output_error = backend_run.as_ref().err().map(ToString::to_string);
        let Ok(backend_run) = backend_run else {
            let Some(error_code) = output_error else {
                unreachable!("failed backend run must contain an output error");
            };
            return record_output_rejection(state, attempt, error_code);
        };
        remaining_output.consume(&backend_run);
        let stopped = stopped_after_backend(request.dispatch_budget());
        if let Some((termination, terminal)) = stopped {
            state.attempts.push(attempt.finish(AttemptConclusion {
                candidate_count: candidate_count(&backend_run.candidates),
                termination,
                backend_failure_code: None,
                outcome: None,
                fallback_eligible: false,
                fallback_taken: false,
            }));
            return AttemptControl::Finish(terminal);
        }
        if backend_run.candidates.is_empty() {
            finish_empty_attempt(
                state,
                attempt,
                backend_run,
                &request,
                attempt_limits,
                &inputs,
            )
        } else {
            AttemptControl::Finish(finish_candidate_attempt(
                state,
                attempt,
                &backend_run,
                &request,
                attempt_limits,
                &mut *inputs.reviewer,
                inputs.parent_budget,
            ))
        }
    }
}

fn initial_terminal(status: DecisionStatus) -> Option<Terminal> {
    match status {
        DecisionStatus::InvalidModel => Some(Terminal::new(
            SolveStatus::InvalidModel,
            ExecutionTerminalReason::InvalidModel,
        )),
        DecisionStatus::UnsupportedOverride => Some(Terminal::new(
            SolveStatus::BackendUnavailable,
            ExecutionTerminalReason::UnsupportedOverride,
        )),
        DecisionStatus::NoCompatibleBackend => Some(Terminal::new(
            SolveStatus::BackendUnavailable,
            ExecutionTerminalReason::NoCompatibleBackend,
        )),
        DecisionStatus::Ready => None,
    }
}

fn backend_order(decision: &RoutingDecision) -> Vec<BackendId> {
    let mut order = Vec::new();
    if let Some(chosen) = &decision.chosen_backend {
        order.push(chosen.clone());
    }
    order.extend(decision.fallback_order.iter().cloned());
    order
}

fn stopped_after_backend(
    dispatch_budget: &eutheto_solver_api::SolveDispatchBudget,
) -> Option<(AttemptTermination, Terminal)> {
    match dispatch_budget.stop_reason()? {
        BackendStopReason::Cancelled => Some((
            AttemptTermination::ParentCancelled,
            Terminal::new(SolveStatus::Cancelled, ExecutionTerminalReason::Cancelled),
        )),
        BackendStopReason::DeadlineExceeded => Some((
            AttemptTermination::ParentDeadlineExceeded,
            Terminal::new(
                SolveStatus::NoSolutionWithinLimit,
                ExecutionTerminalReason::ParentDeadlineExceeded,
            ),
        )),
        BackendStopReason::BackendLimitExceeded => None,
    }
}

fn pre_attempt_terminal(
    parent_budget: &ParentSolveBudget,
    remaining_output: RemainingOutput,
) -> Option<Terminal> {
    let snapshot = parent_budget.phase_view().snapshot();
    if snapshot.cancelled {
        Some(Terminal::new(
            SolveStatus::Cancelled,
            ExecutionTerminalReason::Cancelled,
        ))
    } else if snapshot.expired || snapshot.remaining_milliseconds == DurationMillis::ZERO {
        Some(Terminal::new(
            SolveStatus::NoSolutionWithinLimit,
            ExecutionTerminalReason::ParentDeadlineExceeded,
        ))
    } else if remaining_output.exhausted() {
        Some(Terminal::new(
            SolveStatus::BackendFailed,
            ExecutionTerminalReason::SharedOutputLimitExhausted,
        ))
    } else {
        None
    }
}

fn preflight_attempt(
    backend_id: &BackendId,
    request: &SolveRequest,
    error: &PreflightError,
) -> AttemptRecord {
    AttemptRecord {
        backend_id: backend_id.clone(),
        backend_version: request.backend_version().to_owned(),
        adapter_version: request.adapter_version().to_owned(),
        solve_fingerprint: request.solve_fingerprint().to_owned(),
        invocation_index: None,
        remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
        backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
        preflight_compatibility: preflight_report_from_error(error),
        candidate_count: 0,
        termination: AttemptTermination::PreflightRejected {
            code: preflight_code(error).to_owned(),
        },
        backend_failure_code: None,
        outcome: None,
        fallback_eligible: false,
        fallback_taken: false,
    }
}

fn record_output_rejection(
    state: &mut ExecutionState,
    attempt: AttemptBase,
    error_code: String,
) -> AttemptControl {
    state
        .diagnostics
        .push(RouterDiagnostic::backend_code("solver.output_limits"));
    state.attempts.push(attempt.finish(AttemptConclusion {
        candidate_count: 0,
        termination: AttemptTermination::BackendError {
            code: error_code.clone(),
        },
        backend_failure_code: Some(error_code),
        outcome: None,
        fallback_eligible: false,
        fallback_taken: false,
    }));
    AttemptControl::Finish(Terminal::new(
        SolveStatus::BackendFailed,
        ExecutionTerminalReason::SharedOutputLimitExhausted,
    ))
}

async fn run_backend(
    backend: &dyn SolverBackend,
    request: &SolveRequest,
    problem: &PlanningProblem,
    progress: &mut dyn ProgressSink,
    limits: SolverApiLimits,
) -> Result<BackendRun, OutputError> {
    let mut counting_progress = CountingProgress::new(progress);
    let mut output = BoundedBackendOutput::new(
        problem,
        &mut counting_progress,
        request.dispatch_budget(),
        limits,
    )?;
    let completion = match tokio::time::timeout(
        request.dispatch_budget().remaining_backend_duration(),
        backend.solve(request, &mut output),
    )
    .await
    {
        Ok(result) => BackendCompletion::Returned(result.map(Box::new)),
        Err(_) => BackendCompletion::TimedOut,
    };
    let rejection = output.contract_rejection().cloned();
    let candidate_assignments_used = output.candidate_assignment_count();
    let progress_used = output.progress_event_count();
    let candidates = output.into_candidates();
    let rejection_stop = match rejection {
        Some(OutputError::Cancelled) => Some(BackendStopReason::Cancelled),
        Some(OutputError::ParentDeadlineExceeded) => Some(BackendStopReason::DeadlineExceeded),
        Some(OutputError::BackendLimitExceeded) => Some(BackendStopReason::BackendLimitExceeded),
        Some(error) => return Err(error),
        None => None,
    };
    let stop_reason = rejection_stop
        .or_else(|| request.dispatch_budget().stop_reason())
        .or_else(|| {
            matches!(&completion, BackendCompletion::TimedOut)
                .then_some(BackendStopReason::BackendLimitExceeded)
        });
    Ok(BackendRun {
        completion,
        stop_reason,
        candidates,
        candidate_assignments_used,
        progress_used,
        diagnostic_used: counting_progress.diagnostic_count,
    })
}

fn finish_candidate_attempt(
    state: &mut ExecutionState,
    attempt: AttemptBase,
    run: &BackendRun,
    request: &SolveRequest,
    limits: SolverApiLimits,
    reviewer: &mut dyn CandidateReviewer,
    parent_budget: &ParentSolveBudget,
) -> Terminal {
    let backend_failure_code = match &run.completion {
        BackendCompletion::Returned(Err(error)) => Some(safe_backend_code(error)),
        BackendCompletion::Returned(Ok(_)) | BackendCompletion::TimedOut => None,
    };
    let mut validated_outcome = None;
    let candidate_disposition = if run.stop_reason == Some(BackendStopReason::BackendLimitExceeded)
    {
        review_candidates(
            &attempt.backend_id,
            &run.candidates,
            reviewer,
            parent_budget,
            None,
        )
    } else {
        match &run.completion {
            BackendCompletion::Returned(Err(_)) | BackendCompletion::TimedOut => review_candidates(
                &attempt.backend_id,
                &run.candidates,
                reviewer,
                parent_budget,
                None,
            ),
            BackendCompletion::Returned(Ok(outcome)) => {
                if let Err(error) =
                    validate_outcome(request, outcome.as_ref(), &run.candidates, limits)
                {
                    CandidateDisposition {
                        termination: AttemptTermination::OutcomeContractFailure {
                            code: outcome_error_code(error).to_owned(),
                        },
                        status: SolveStatus::BackendFailed,
                        reason: ExecutionTerminalReason::BackendFailure,
                        selected_candidate: None,
                        diagnostic: None,
                    }
                } else {
                    validated_outcome = Some(outcome.as_ref().clone());
                    review_candidates(
                        &attempt.backend_id,
                        &run.candidates,
                        reviewer,
                        parent_budget,
                        Some(outcome.as_ref()),
                    )
                }
            }
        }
    };
    if let Some(diagnostic) = candidate_disposition.diagnostic {
        state.diagnostics.push(diagnostic);
    }
    let terminal = Terminal::with_candidate(
        candidate_disposition.status,
        candidate_disposition.reason,
        candidate_disposition.selected_candidate,
    );
    state.attempts.push(attempt.finish(AttemptConclusion {
        candidate_count: candidate_count(&run.candidates),
        termination: candidate_disposition.termination,
        backend_failure_code,
        outcome: validated_outcome,
        fallback_eligible: false,
        fallback_taken: false,
    }));
    terminal
}

fn finish_empty_attempt(
    state: &mut ExecutionState,
    attempt: AttemptBase,
    run: BackendRun,
    request: &SolveRequest,
    limits: SolverApiLimits,
    inputs: &AttemptInputs<'_>,
) -> AttemptControl {
    let backend_failure_code = match &run.completion {
        BackendCompletion::Returned(Err(error)) => Some(safe_backend_code(error)),
        BackendCompletion::Returned(Ok(_)) | BackendCompletion::TimedOut => None,
    };
    let (termination, status, reason, fallback_eligible, validated_outcome) =
        if run.stop_reason == Some(BackendStopReason::BackendLimitExceeded) {
            (
                AttemptTermination::BackendOutcome(BackendTerminationReason::TimeLimit),
                SolveStatus::NoSolutionWithinLimit,
                ExecutionTerminalReason::BackendTerminated,
                true,
                None,
            )
        } else {
            match run.completion {
                BackendCompletion::Returned(Err(error)) => (
                    AttemptTermination::BackendError {
                        code: safe_backend_code(&error),
                    },
                    SolveStatus::BackendFailed,
                    ExecutionTerminalReason::BackendFailure,
                    true,
                    None,
                ),
                BackendCompletion::Returned(Ok(outcome)) => {
                    if let Err(error) =
                        validate_outcome(request, outcome.as_ref(), &run.candidates, limits)
                    {
                        (
                            AttemptTermination::OutcomeContractFailure {
                                code: outcome_error_code(error).to_owned(),
                            },
                            SolveStatus::BackendFailed,
                            ExecutionTerminalReason::BackendFailure,
                            true,
                            None,
                        )
                    } else {
                        let (termination, status, reason, fallback) =
                            terminal_from_outcome(outcome.as_ref());
                        (termination, status, reason, fallback, Some(*outcome))
                    }
                }
                BackendCompletion::TimedOut => unreachable!("timeout records backend cap expiry"),
            }
        };
    let snapshot = inputs.parent_budget.phase_view().snapshot();
    let fallback_taken = fallback_eligible
        && state
            .decision
            .profile
            .is_none_or(|profile| profile.allow_fallback)
        && inputs.position + 1 < inputs.order_len
        && !snapshot.cancelled
        && !snapshot.expired
        && snapshot.remaining_milliseconds != DurationMillis::ZERO;
    state.attempts.push(attempt.finish(AttemptConclusion {
        candidate_count: 0,
        termination,
        backend_failure_code,
        outcome: validated_outcome,
        fallback_eligible,
        fallback_taken,
    }));
    if fallback_taken {
        AttemptControl::Continue
    } else {
        AttemptControl::Finish(Terminal::new(status, reason))
    }
}

fn candidate_count(candidates: &[BackendCandidate]) -> u32 {
    u32::try_from(candidates.len()).unwrap_or(u32::MAX)
}

struct CountingProgress<'a> {
    downstream: &'a mut dyn ProgressSink,
    diagnostic_count: u32,
}

impl<'a> CountingProgress<'a> {
    fn new(downstream: &'a mut dyn ProgressSink) -> Self {
        Self {
            downstream,
            diagnostic_count: 0,
        }
    }
}

impl ProgressSink for CountingProgress<'_> {
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        if matches!(event, SolveProgressEvent::LogLine(_)) {
            self.diagnostic_count = self.diagnostic_count.saturating_add(1);
        }
        self.downstream.emit(event)
    }
}

fn review_candidates(
    backend_id: &BackendId,
    candidates: &[BackendCandidate],
    reviewer: &mut dyn CandidateReviewer,
    parent_budget: &ParentSolveBudget,
    outcome: Option<&BackendSolveOutcome>,
) -> CandidateDisposition {
    for candidate in candidates {
        if let Some(stopped) = stopped_review(parent_budget) {
            return stopped;
        }
        let review = reviewer.review(backend_id, candidate);
        if let Some(stopped) = stopped_review(parent_budget) {
            return stopped;
        }
        match review {
            CandidateReview::VerificationFailed { diagnostic_code } => {
                let diagnostic = RouterDiagnostic::backend_code(&diagnostic_code);
                return CandidateDisposition {
                    termination: AttemptTermination::VerificationQuarantined { diagnostic_code },
                    status: SolveStatus::BackendFailed,
                    reason: ExecutionTerminalReason::VerificationQuarantined,
                    selected_candidate: None,
                    diagnostic: Some(diagnostic),
                };
            }
            CandidateReview::Interrupted(reason) => return interrupted_review(reason),
            CandidateReview::Verified { objective_matches } => {
                return CandidateDisposition {
                    termination: AttemptTermination::CandidateVerified,
                    status: if objective_matches
                        && selected_optimal_candidate(candidate, candidates, outcome)
                    {
                        SolveStatus::Optimal
                    } else {
                        SolveStatus::Feasible
                    },
                    reason: ExecutionTerminalReason::CandidateVerified,
                    selected_candidate: Some(candidate.clone()),
                    diagnostic: None,
                };
            }
            CandidateReview::AwaitingIndependentVerification => {}
        }
    }
    CandidateDisposition {
        termination: AttemptTermination::CandidateAwaitingVerification,
        status: SolveStatus::NoSolutionWithinLimit,
        reason: ExecutionTerminalReason::CandidateAwaitingVerification,
        selected_candidate: candidates.first().cloned(),
        diagnostic: None,
    }
}

fn stopped_review(parent_budget: &ParentSolveBudget) -> Option<CandidateDisposition> {
    let snapshot = parent_budget.snapshot();
    if snapshot.cancelled {
        Some(interrupted_review(OperationInterruption::Cancelled))
    } else if snapshot.expired || snapshot.remaining_milliseconds == DurationMillis::ZERO {
        Some(interrupted_review(OperationInterruption::DeadlineExceeded))
    } else {
        None
    }
}

fn interrupted_review(reason: OperationInterruption) -> CandidateDisposition {
    let (termination, status, reason) = match reason {
        OperationInterruption::Cancelled => (
            AttemptTermination::ReviewCancelled,
            SolveStatus::Cancelled,
            ExecutionTerminalReason::Cancelled,
        ),
        OperationInterruption::DeadlineExceeded => (
            AttemptTermination::ReviewDeadlineExceeded,
            SolveStatus::NoSolutionWithinLimit,
            ExecutionTerminalReason::ParentDeadlineExceeded,
        ),
    };
    CandidateDisposition {
        termination,
        status,
        reason,
        selected_candidate: None,
        diagnostic: None,
    }
}

fn selected_optimal_candidate(
    selected: &BackendCandidate,
    candidates: &[BackendCandidate],
    outcome: Option<&BackendSolveOutcome>,
) -> bool {
    let Some(outcome) = outcome else { return false };
    if outcome.termination != BackendTerminationReason::OptimalityClaimed
        || candidates
            .last()
            .is_none_or(|last| last.sequence != selected.sequence)
    {
        return false;
    }
    let (Some(selected_objective), Some(final_objective)) =
        (&selected.objective, &outcome.evidence.objective)
    else {
        return false;
    };
    selected_objective.objective_values == final_objective.objective_values
        && final_objective.best_bound_values.as_ref() == Some(&final_objective.objective_values)
}

fn terminal_from_outcome(
    outcome: &BackendSolveOutcome,
) -> (
    AttemptTermination,
    SolveStatus,
    ExecutionTerminalReason,
    bool,
) {
    let status = outcome
        .conclusive_status_without_candidate()
        .unwrap_or(SolveStatus::BackendFailed);
    let fallback = matches!(
        outcome.termination,
        BackendTerminationReason::Unavailable
            | BackendTerminationReason::Failed
            | BackendTerminationReason::TimeLimit
    );
    let reason = if matches!(
        outcome.termination,
        BackendTerminationReason::Unavailable | BackendTerminationReason::Failed
    ) {
        ExecutionTerminalReason::BackendFailure
    } else if outcome.termination == BackendTerminationReason::Cancelled {
        ExecutionTerminalReason::Cancelled
    } else {
        ExecutionTerminalReason::BackendTerminated
    };
    (
        AttemptTermination::BackendOutcome(outcome.termination),
        status,
        reason,
        fallback,
    )
}

fn preflight_report_from_error(error: &PreflightError) -> Option<CompatibilityReport> {
    match error {
        PreflightError::Incompatible(report) => Some(report.clone()),
        _ => None,
    }
}

fn preflight_terminal(error: &PreflightError) -> (SolveStatus, ExecutionTerminalReason) {
    match error {
        PreflightError::Cancelled => (SolveStatus::Cancelled, ExecutionTerminalReason::Cancelled),
        PreflightError::DeadlineExceeded => (
            SolveStatus::NoSolutionWithinLimit,
            ExecutionTerminalReason::ParentDeadlineExceeded,
        ),
        PreflightError::InvalidProblem(_)
        | PreflightError::SummaryMismatch
        | PreflightError::InvalidSolveOptions => (
            SolveStatus::InvalidModel,
            ExecutionTerminalReason::PreflightRejected,
        ),
        PreflightError::InvalidDescriptor(_)
        | PreflightError::RequestBackendMismatch
        | PreflightError::BackendVersionMismatch
        | PreflightError::UnresolvedBackendSelection
        | PreflightError::OptionBackendMismatch
        | PreflightError::AdapterVersionMismatch
        | PreflightError::Matrix(_)
        | PreflightError::InternalFeature(_)
        | PreflightError::Incompatible(_) => (
            SolveStatus::BackendUnavailable,
            ExecutionTerminalReason::PreflightRejected,
        ),
    }
}

fn preflight_code(error: &PreflightError) -> &'static str {
    match error {
        PreflightError::InvalidDescriptor(_) => "solver.preflight.invalid_descriptor",
        PreflightError::RequestBackendMismatch => "solver.preflight.request_backend_mismatch",
        PreflightError::BackendVersionMismatch => "solver.preflight.backend_version_mismatch",
        PreflightError::UnresolvedBackendSelection => "solver.preflight.unresolved_selection",
        PreflightError::OptionBackendMismatch => "solver.preflight.option_backend_mismatch",
        PreflightError::AdapterVersionMismatch => "solver.preflight.adapter_version_mismatch",
        PreflightError::Cancelled => "solver.preflight.cancelled",
        PreflightError::DeadlineExceeded => "solver.preflight.deadline_exceeded",
        PreflightError::Matrix(_) => "solver.preflight.matrix",
        PreflightError::InvalidProblem(_) => "solver.preflight.invalid_problem",
        PreflightError::SummaryMismatch => "solver.preflight.summary_mismatch",
        PreflightError::InvalidSolveOptions => "solver.preflight.invalid_options",
        PreflightError::InternalFeature(_) => "solver.preflight.internal_feature",
        PreflightError::Incompatible(_) => "solver.preflight.incompatible",
    }
}

fn outcome_error_code(error: OutcomeError) -> &'static str {
    match error {
        OutcomeError::InvalidLimits => "solver.outcome.invalid_limits",
        OutcomeError::BackendMismatch => "solver.outcome.backend_mismatch",
        OutcomeError::ModelHashMismatch => "solver.outcome.model_hash_mismatch",
        OutcomeError::SolveFingerprintMismatch => "solver.outcome.solve_fingerprint_mismatch",
        OutcomeError::BudgetEvidenceMismatch => "solver.outcome.budget_mismatch",
        OutcomeError::BackendLimitExceeded => "solver.outcome.backend_limit_exceeded",
        OutcomeError::InvalidFirstIncumbentTime => "solver.outcome.first_incumbent_time",
        OutcomeError::OutputLimitExceeded => "solver.outcome.output_limit",
        OutcomeError::CandidateAssignmentLimitExceeded => {
            "solver.outcome.candidate_assignment_limit"
        }
        OutcomeError::InvalidObjectiveDimension => "solver.outcome.objective_dimension",
        OutcomeError::NonMonotonicCandidateTime => "solver.outcome.candidate_time_order",
        OutcomeError::CandidateSequenceMismatch => "solver.outcome.candidate_sequence",
        OutcomeError::CandidateAfterTermination => "solver.outcome.candidate_after_termination",
        OutcomeError::MissingCandidate => "solver.outcome.missing_candidate",
        OutcomeError::CandidateContradictsTermination => "solver.outcome.candidate_contradiction",
        OutcomeError::UnrequestedCancellation => "solver.outcome.unrequested_cancellation",
        OutcomeError::PrematureTimeLimit => "solver.outcome.premature_time_limit",
        OutcomeError::FirstIncumbentMismatch => "solver.outcome.first_incumbent_mismatch",
        OutcomeError::ExecutionBackendVersionMismatch => "solver.outcome.execution_backend_version",
        OutcomeError::ExecutionAdapterVersionMismatch => "solver.outcome.execution_adapter_version",
        OutcomeError::ExecutionOptionsMismatch => "solver.outcome.execution_options",
        OutcomeError::ExecutionModelCountMismatch => "solver.outcome.execution_model_counts",
        OutcomeError::InvalidExecutionEvidence => "solver.outcome.execution_evidence",
    }
}

fn safe_backend_code(error: &BackendError) -> String {
    let code = error.code();
    if code.len() <= 96 && code.bytes().all(|byte| byte.is_ascii_graphic()) {
        code.to_owned()
    } else {
        "solver.backend_failure".to_owned()
    }
}

fn elapsed_since(start: DurationMillis, current: DurationMillis) -> DurationMillis {
    DurationMillis::new(start.value().saturating_sub(current.value()))
        .unwrap_or(DurationMillis::MAX)
}
