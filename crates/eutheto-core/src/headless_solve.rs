use super::super::{
    available_pack, domain_interruption, export_error, operation_interrupted, protocol_error,
    solution_contract_error,
};
use super::HeadlessService;
use crate::RouterCandidateReviewer;
use crate::verification::{
    ParentVerificationClock, accepted_evidence, runtime_evidence_matches, terminal_phase_timings,
};
use eutheto_domain_api::{CompileContext, DomainValidationReport};
use eutheto_domain_ir::{
    AcceptedResult, DomainEvidenceId, PortableAcceptedResultV2, RunInputV1, RunManifestV1,
    RunTerminalOutcomeV1, VerificationValue,
};
use eutheto_export::{ExportError, PreparedPublication, prepare_json_atomic_controlled};
use eutheto_import::{InspectedBundle, InspectionPolicy, validate_standalone_scenario};
use eutheto_planning_ir::{
    MetadataKey, PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningProblem,
    PlanningProblemSummary, ProvenanceParameter, canonical_objective_policy_hash,
};
use eutheto_solver_api::{BackendRuntimeIdentity, ProgressSink};
use eutheto_solver_router::{
    DecisionStatus, ExecutionTerminalReason, RouterExecutionRecord, RoutingDecision, SolverRouter,
};
use eutheto_types::{
    AppError, BackendSelection, CancellationToken, DurationMillis, OperationControl,
    OperationInterruption, ParentSolveBudget, RequestId, Rfc3339Timestamp, ScenarioSnapshotId,
    ScenarioSnapshotV1, SolutionId, SolveOptions, SolveRunId, SolveStatus,
};
use eutheto_verify::{AcceptanceDecision, CorrectnessAlarm, CorrectnessAlarmCategory};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[path = "headless_stored.rs"]
mod stored;
pub use stored::*;

/// One original deadline, created before the caller reads or captures any scenario input.
/// Consuming preparation and execution prevent a file solve from being dispatched twice.
pub struct SolveOperation {
    service: HeadlessService,
    budget: ParentSolveBudget,
    started_at: Rfc3339Timestamp,
    request_id: RequestId,
    run_id: SolveRunId,
    options: SolveOptions,
    acceptance_reserve: DurationMillis,
}

/// Preparation fails before durable run ownership or any backend invocation.
/// A completed readiness report remains available even when compilation cannot proceed.
#[derive(Debug)]
pub struct SolvePreparationFailure {
    pub status: SolveStatus,
    pub error: AppError,
    pub validation: Option<DomainValidationReport>,
    pub routing: Option<RoutingDecision>,
}

/// Original-domain validation plus the actual compiler's bounded metrics and model summary.
#[derive(Clone, Debug)]
pub struct HeadlessCompilationReport {
    pub validation: DomainValidationReport,
    pub summary: PlanningProblemSummary,
    pub compile_metadata: BTreeMap<MetadataKey, ProvenanceParameter>,
}

/// A compiled immutable snapshot with Auto already resolved to one registered runtime.
pub struct PreparedSolve {
    operation: SolveOperation,
    snapshot: ScenarioSnapshotV1,
    problem: Arc<PlanningProblem>,
    report: HeadlessCompilationReport,
    routing: RoutingDecision,
    identity: BackendRuntimeIdentity,
    document_hash: String,
    objective_policy_hash: String,
    compile_elapsed: DurationMillis,
    compile_finished: Duration,
    // Keep all inspected bundle payloads alive through execution; do not silently extract the JSON.
    _bundle: Option<InspectedBundle>,
}

/// Terminal file-solve material. A candidate is exposed only after original-domain acceptance.
///
/// Runtime bindings and terminal proof cannot be substituted before publication:
/// ```compile_fail
/// let _cannot_rebind: fn(&mut eutheto_core::HeadlessSolveOutcome) =
///     |outcome| { let _ = &mut outcome.run_input; };
/// ```
/// ```compile_fail
/// let _cannot_promote: fn(&mut eutheto_core::HeadlessSolveOutcome) =
///     |outcome| { let _ = &mut outcome.manifest; };
/// ```
pub struct HeadlessSolveOutcome {
    run_input: RunInputV1,
    manifest: RunManifestV1,
    pub compilation: HeadlessCompilationReport,
    pub routing: RoutingDecision,
    pub execution: RouterExecutionRecord,
    accepted: Option<AcceptedResult>,
    evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    alarm: Option<CorrectnessAlarm>,
    operation: SolveOperation,
    last_observed_elapsed: Duration,
}

/// An independently accepted file result staged under its original solve deadline.
pub struct PreparedHeadlessResultPublication {
    result_id: SolutionId,
    control: OperationControl,
    prepared: PreparedPublication,
}

impl PreparedHeadlessResultPublication {
    /// Performs one no-clobber filesystem commit. Successful publication wins over later cancellation.
    ///
    /// # Errors
    /// Returns an explicit interruption or sanitized filesystem error before successful publication.
    pub fn publish(self) -> Result<SolutionId, AppError> {
        self.control.check().map_err(operation_interrupted)?;
        self.prepared
            .publish_controlled(&self.control)
            .map_err(|error| publication_error(&error, &self.control))?;
        Ok(self.result_id)
    }
}

impl HeadlessService {
    /// Starts the whole operation before input acquisition. Request IDs are caller-stable;
    /// run IDs are allocated only for new work, never for a durable idempotent lookup.
    ///
    /// # Errors
    /// Rejects invalid options, deadline overflow or identity generation failure.
    pub fn begin_solve(
        &self,
        request_id: RequestId,
        options: SolveOptions,
        cancellation: CancellationToken,
    ) -> Result<SolveOperation, AppError> {
        options
            .validate()
            .map_err(|_| solve_error("solve.options_invalid", "Solve options are invalid."))?;
        let budget = ParentSolveBudget::new(
            options.time_limit_milliseconds,
            Arc::clone(&self.monotonic_clock),
            cancellation,
        )
        .map_err(|_| timing_error())?;
        let started_at = self.clock.now();
        let run_id = SolveRunId::new(self.ids.as_ref()).map_err(|_| {
            solve_error(
                "solve.id_generation_failed",
                "A solve identity could not be created.",
            )
        })?;
        let reserve = (options.time_limit_milliseconds.value() / 4).clamp(1, 1000);
        let acceptance_reserve = DurationMillis::new(reserve).map_err(|_| timing_error())?;
        Ok(SolveOperation {
            service: self.clone(),
            budget,
            started_at,
            request_id,
            run_id,
            options,
            acceptance_reserve,
        })
    }
}

impl SolveOperation {
    /// Shared control for bounded file reads, decoding and every later phase.
    #[must_use]
    pub fn control(&self) -> OperationControl {
        OperationControl::Solve(self.budget.phase_view())
    }

    /// Checks and compiles a standalone snapshot; referenced assets require the bundle path.
    ///
    /// # Errors
    /// Preserves validation/routing evidence without claiming infeasibility for invalid input.
    pub fn prepare(
        self,
        snapshot: ScenarioSnapshotV1,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        validate_standalone_scenario(&snapshot, &InspectionPolicy::default())
            .map_err(|error| preparation_failure(super::super::import_error(&error), &control))?;
        self.prepare_captured(snapshot, None)
    }

    /// Inspects the entire `ScenarioExport` bundle under the original operation deadline.
    ///
    /// # Errors
    /// Rejects integrity, shape, asset, migration and reference failures before compilation.
    pub fn prepare_bundle(
        self,
        bytes: &[u8],
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        let mut bundle = self
            .service
            .decode_scenario_bundle(bytes, &control)
            .map_err(|error| preparation_failure(error, &control))?;
        let snapshot = bundle
            .scenarios
            .pop()
            .ok_or_else(|| preparation_failure(solution_contract_error(), &control))?;
        self.prepare_captured(snapshot, Some(bundle))
    }

    // One consuming transition retains validation/routing evidence without cloning it on success.
    #[allow(clippy::too_many_lines)]
    fn prepare_captured(
        self,
        snapshot: ScenarioSnapshotV1,
        bundle: Option<InspectedBundle>,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        control
            .check()
            .map_err(|reason| preparation_failure(operation_interrupted(reason), &control))?;
        let validation = self
            .service
            .validate_full(&snapshot, &control)
            .map_err(|error| preparation_failure(error, &control))?;
        let mut routing = None;
        let prepared = (|| {
            let pack = available_pack(&snapshot.document, &self.service.packs)
                .ok_or_else(solution_contract_error)?;
            let limits = PlanningIrLimitsV1::DEFAULT.tightened_by(self.options.resource_limits);
            let compile_start = observed_elapsed(&self.budget)?;
            let problem = pack
                .compile(
                    &snapshot.document,
                    &CompileContext {
                        scenario_revision: snapshot.revision.value(),
                        semantic_metadata: BTreeMap::new(),
                        control: control.clone(),
                        planning_limits: limits,
                    },
                )
                .map_err(|error| {
                    domain_interruption(&error).unwrap_or_else(|| {
                        solve_error(
                            "solve.compilation_failed",
                            "The scenario could not be compiled.",
                        )
                    })
                })?;
            let compile_finished = observed_elapsed(&self.budget)?;
            let compile_elapsed = elapsed_between(compile_start, compile_finished)?;
            control.check().map_err(operation_interrupted)?;
            let decision = SolverRouter::new(&self.service.solvers).decide(&problem, &self.options);
            routing = Some(decision);
            let decision = routing.as_ref().ok_or_else(solution_contract_error)?;
            if decision.status == DecisionStatus::InvalidModel {
                return Err(solve_error(
                    "solve.model_invalid",
                    "The compiled model violates the planning contract.",
                ));
            }
            if decision.status != DecisionStatus::Ready {
                return Err(solve_error(
                    "solve.backend_unavailable",
                    "No compatible registered backend can execute this model.",
                ));
            }
            let backend_id = decision
                .chosen_backend
                .as_ref()
                .ok_or_else(solution_contract_error)?;
            let backend = self.service.solvers.get(backend_id).ok_or_else(|| {
                solve_error(
                    "solve.backend_unavailable",
                    "The selected backend has no verified runtime.",
                )
            })?;
            let identity = backend.runtime_identity().clone();
            let summary = decision
                .summary
                .clone()
                .ok_or_else(solution_contract_error)?;
            let document_hash = eutheto_domain_ir::blake3_hex(
                &serde_json::to_vec(&snapshot.document).map_err(|_| solution_contract_error())?,
            );
            let objective_policy_hash = canonical_objective_policy_hash(&problem, limits)
                .map_err(|_| solution_contract_error())?;
            if problem.metadata.scenario_id != snapshot.document.scenario_id
                || problem.metadata.scenario_revision != snapshot.revision.value()
                || problem.metadata.pack_id != snapshot.document.domain_pack.id
            {
                return Err(solution_contract_error());
            }
            control.check().map_err(operation_interrupted)?;
            Ok((
                problem,
                identity,
                summary,
                document_hash,
                objective_policy_hash,
                compile_elapsed,
                compile_finished,
            ))
        })();
        match prepared {
            Ok((
                problem,
                identity,
                summary,
                document_hash,
                objective_policy_hash,
                compile_elapsed,
                compile_finished,
            )) => {
                let mut operation = self;
                operation.options.backend =
                    BackendSelection::Specific(identity.backend_id().clone());
                let report = HeadlessCompilationReport {
                    validation,
                    summary,
                    compile_metadata: problem.metadata.compile_metadata.clone(),
                };
                Ok(PreparedSolve {
                    operation,
                    snapshot,
                    problem: Arc::new(problem),
                    report,
                    routing: routing
                        .ok_or_else(|| preparation_failure(solution_contract_error(), &control))?,
                    identity,
                    document_hash,
                    objective_policy_hash,
                    compile_elapsed,
                    compile_finished,
                    _bundle: bundle,
                })
            }
            Err(error) => {
                let mut failure = preparation_failure(error, &control);
                failure.validation = Some(validation);
                failure.routing = routing;
                Err(failure)
            }
        }
    }
}

impl PreparedSolve {
    #[must_use]
    pub fn compilation(&self) -> &HeadlessCompilationReport {
        &self.report
    }

    #[must_use]
    pub fn routing(&self) -> &RoutingDecision {
        &self.routing
    }

    /// Executes once without opening `SQLite`. Auto cannot fall back after immutable input binding.
    ///
    /// # Errors
    /// Rejects untrustworthy timing or inconsistent immutable/runtime/result evidence.
    pub async fn execute(
        self,
        progress: &mut dyn ProgressSink,
    ) -> Result<HeadlessSolveOutcome, AppError> {
        let snapshot_id =
            ScenarioSnapshotId::new(self.operation.service.ids.as_ref()).map_err(|_| {
                solve_error(
                    "solve.id_generation_failed",
                    "A snapshot identity could not be created.",
                )
            })?;
        let input = self.make_run_input(snapshot_id, self.operation.started_at)?;
        self.execute_bound(input, progress).await
    }

    fn make_run_input(
        &self,
        snapshot_id: ScenarioSnapshotId,
        snapshot_created_at: Rfc3339Timestamp,
    ) -> Result<RunInputV1, AppError> {
        RunInputV1::new(
            self.operation.run_id,
            self.operation.request_id,
            self.snapshot.document.scenario_id,
            self.snapshot.revision.value(),
            snapshot_id,
            self.document_hash.clone(),
            snapshot_created_at,
            self.snapshot.document.domain_pack.id.clone(),
            self.snapshot.document.domain_pack.schema_version,
            PLANNING_IR_SCHEMA_VERSION,
            self.problem.metadata.compiler_version.clone(),
            env!("CARGO_PKG_VERSION").to_owned(),
            self.identity.backend_id().clone(),
            self.identity.backend_version().to_owned(),
            self.identity.adapter_version().to_owned(),
            self.identity.worker_version().to_owned(),
            self.identity.solver_version().to_owned(),
            self.identity.protocol_major(),
            self.identity.protocol_minor(),
            self.report.summary.canonical_ir_hash.clone(),
            self.objective_policy_hash.clone(),
            self.operation.options.clone(),
            self.snapshot.document.settings.time_zone.clone(),
            None,
        )
        .map_err(|_| solution_contract_error())
    }

    // Candidate admission, deadline revocation and terminal milestones remain one transition.
    #[allow(clippy::too_many_lines)]
    async fn execute_bound(
        self,
        input: RunInputV1,
        progress: &mut dyn ProgressSink,
    ) -> Result<HeadlessSolveOutcome, AppError> {
        if input != self.make_run_input(input.snapshot_id, input.snapshot_created_at)? {
            return Err(solution_contract_error());
        }
        let pack = available_pack(&self.snapshot.document, &self.operation.service.packs)
            .ok_or_else(solution_contract_error)?;
        let clock = ParentVerificationClock::new(&self.operation.budget);
        let control = self.operation.control();
        let mut reviewer = RouterCandidateReviewer::new(
            pack,
            &self.snapshot.document,
            self.snapshot.revision.value(),
            &self.problem,
            &clock,
            self.operation.service.ids.as_ref(),
            &control,
        )
        .map_err(|_| solution_contract_error())?;
        let execution = SolverRouter::new(&self.operation.service.solvers)
            .execute(
                Arc::clone(&self.problem),
                self.operation.options.clone(),
                &self.operation.budget,
                self.operation.acceptance_reserve,
                progress,
                &mut reviewer,
            )
            .await;
        let decision = reviewer.into_decision();
        if clock.has_failed() {
            return Err(timing_error());
        }
        let observations = execution.observations.ok_or_else(timing_error)?;
        elapsed_between(self.compile_finished, observations.started_after)?;
        let backend_elapsed = if execution.record.invocation_count == 0 {
            None
        } else {
            Some(milliseconds(observations.backend_elapsed)?)
        };
        let mut phases =
            terminal_phase_timings(decision.as_ref(), self.compile_elapsed, backend_elapsed);
        let first_incumbent = observations
            .first_candidate_after
            .map(|offset| {
                observations
                    .started_after
                    .checked_add(offset)
                    .ok_or_else(timing_error)
                    .and_then(milliseconds)
            })
            .transpose()?;
        let first_verified = observations
            .first_verified_feasible_after
            .map(|offset| {
                observations
                    .started_after
                    .checked_add(offset)
                    .ok_or_else(timing_error)
                    .and_then(milliseconds)
            })
            .transpose()?;
        let mut accepted = None;
        let mut alarm = None;
        let runtime_matches =
            runtime_evidence_matches(&execution.record, &self.identity, &self.operation.options);
        let mut outcome = match (execution.record.terminal_reason, decision) {
            (
                ExecutionTerminalReason::CandidateVerified,
                Some(AcceptanceDecision::Accepted { result, .. }),
            ) if runtime_matches => {
                let outcome = RunTerminalOutcomeV1::Accepted {
                    status: execution.record.terminal_status,
                    solution_id: result.solution.solution_id,
                    accepted_result_checksum: result.checksum.clone(),
                    verification_checksum: result.verification.checksum.clone(),
                };
                accepted = Some(*result);
                outcome
            }
            (
                ExecutionTerminalReason::VerificationQuarantined,
                Some(AcceptanceDecision::Quarantined {
                    alarm: correctness_alarm,
                    ..
                }),
            ) => {
                if correctness_alarm.category == CorrectnessAlarmCategory::ClockFailed {
                    return Err(timing_error());
                }
                let outcome = RunTerminalOutcomeV1::VerificationAlarm {
                    diagnostic_code: correctness_alarm.diagnostic_code.clone(),
                };
                alarm = Some(correctness_alarm);
                outcome
            }
            (
                ExecutionTerminalReason::CandidateVerified
                | ExecutionTerminalReason::VerificationQuarantined,
                _,
            ) => no_result(SolveStatus::BackendFailed),
            _ => {
                let status = if matches!(
                    execution.record.terminal_status,
                    SolveStatus::Optimal | SolveStatus::Feasible
                ) || (execution.record.invocation_count > 0
                    && !runtime_matches
                    && matches!(
                        execution.record.terminal_status,
                        SolveStatus::Infeasible | SolveStatus::Unbounded
                    )) {
                    SolveStatus::BackendFailed
                } else {
                    execution.record.terminal_status
                };
                no_result(status)
            }
        };
        let evidence_started = observed_elapsed(&self.operation.budget)?;
        elapsed_between(observations.finished_after, evidence_started)?;
        let mut evidence = accepted.as_ref().map(accepted_evidence).unwrap_or_default();
        let evidence_finished = observed_elapsed(&self.operation.budget)?;
        phases.evidence_persistence_milliseconds =
            Some(elapsed_between(evidence_started, evidence_finished)?);
        if accepted.is_some()
            && let Err(reason) = control.check()
        {
            accepted = None;
            evidence.clear();
            outcome = no_result(interruption_status(reason));
        }
        let finished_at = self.operation.service.clock.now();
        let last_observed_elapsed = observed_elapsed(&self.operation.budget)?;
        elapsed_between(evidence_finished, last_observed_elapsed)?;
        let total = milliseconds(last_observed_elapsed)?;
        let wall_deadline = self
            .operation
            .started_at
            .as_timestamp()
            .checked_add(Duration::from_millis(
                self.operation.options.time_limit_milliseconds.value(),
            ))
            .map_err(|_| timing_error())?;
        if accepted.is_some()
            && (finished_at.as_timestamp() > wall_deadline
                || total > self.operation.options.time_limit_milliseconds)
        {
            accepted = None;
            evidence.clear();
            outcome = no_result(SolveStatus::NoSolutionWithinLimit);
        }
        let warnings = accepted
            .as_ref()
            .map(|result| result.verification.warnings.clone())
            .unwrap_or_default();
        let first_verified = accepted.as_ref().and(first_verified);
        let mut record = execution.record;
        if accepted.is_none() {
            record.selected_candidate = None;
        }
        let manifest = RunManifestV1::new(
            input.run_id,
            input.checksum.clone(),
            outcome,
            self.operation.started_at,
            finished_at,
            Some(total),
            first_incumbent,
            first_verified,
            phases,
            warnings,
        )
        .map_err(|_| timing_error())?;
        Ok(HeadlessSolveOutcome {
            run_input: input,
            manifest,
            compilation: self.report,
            routing: self.routing,
            execution: record,
            accepted,
            evidence,
            alarm,
            operation: self.operation,
            last_observed_elapsed,
        })
    }
}

impl HeadlessSolveOutcome {
    /// Immutable execution bindings cannot be substituted before file publication.
    #[must_use]
    pub fn run_input(&self) -> &RunInputV1 {
        &self.run_input
    }

    /// The independently established terminal classification, not a caller-authored claim.
    #[must_use]
    pub fn manifest(&self) -> &RunManifestV1 {
        &self.manifest
    }

    #[must_use]
    pub fn accepted_result(&self) -> Option<&AcceptedResult> {
        self.accepted.as_ref()
    }

    #[must_use]
    pub fn correctness_alarm(&self) -> Option<&CorrectnessAlarm> {
        self.alarm.as_ref()
    }

    /// Stages accepted output without publishing it or renewing the original solve deadline.
    ///
    /// # Errors
    /// Rejects unaccepted/invalid material, interruption and unsafe or failed output staging.
    pub fn prepare_publication(
        self,
        destination: &Path,
    ) -> Result<Option<PreparedHeadlessResultPublication>, AppError> {
        let control = self.control();
        let Some(portable) = self.into_portable()? else {
            return Ok(None);
        };
        control.check().map_err(operation_interrupted)?;
        let prepared = prepare_json_atomic_controlled(destination, &portable, &control)
            .map_err(|error| publication_error(&error, &control))?;
        control.check().map_err(operation_interrupted)?;
        Ok(Some(PreparedHeadlessResultPublication {
            result_id: portable.result_id,
            control,
            prepared,
        }))
    }

    #[must_use]
    pub fn control(&self) -> OperationControl {
        self.operation.control()
    }

    /// Materializes a portable accepted result without granting authority to imported reports.
    /// The caller must keep subsequent encoding and staging under [`Self::control`].
    ///
    /// # Errors
    /// Rejects expired/cancelled output preparation or inconsistent nested portable contracts.
    pub fn into_portable(mut self) -> Result<Option<PortableAcceptedResultV2>, AppError> {
        if self.accepted.is_none() {
            return Ok(None);
        }
        self.operation
            .control()
            .check()
            .map_err(operation_interrupted)?;
        self.refresh_admission()?;
        self.operation
            .control()
            .check()
            .map_err(operation_interrupted)?;
        let accepted = self
            .accepted
            .ok_or_else(|| operation_interrupted(OperationInterruption::DeadlineExceeded))?;
        let portable =
            PortableAcceptedResultV2::new(self.run_input, self.manifest, accepted, self.evidence)
                .map_err(|_| solution_contract_error())?;
        self.operation
            .control()
            .check()
            .map_err(operation_interrupted)?;
        Ok(Some(portable))
    }
}
impl HeadlessSolveOutcome {
    fn refresh_admission(&mut self) -> Result<(), AppError> {
        let interruption = self
            .operation
            .control()
            .check()
            .err()
            .map(interruption_status);
        let finished_at = self.operation.service.clock.now();
        let observed = observed_elapsed(&self.operation.budget)?;
        elapsed_between(self.last_observed_elapsed, observed)?;
        self.last_observed_elapsed = observed;
        let elapsed = milliseconds(observed)?;
        let deadline = self
            .operation
            .started_at
            .as_timestamp()
            .checked_add(Duration::from_millis(
                self.operation.options.time_limit_milliseconds.value(),
            ))
            .map_err(|_| timing_error())?;
        let status = interruption.or_else(|| {
            (elapsed > self.operation.options.time_limit_milliseconds
                || finished_at.as_timestamp() > deadline)
                .then_some(SolveStatus::NoSolutionWithinLimit)
        });
        let mut terminal = self.manifest.outcome.clone();
        if self.accepted.is_some()
            && let Some(status) = status
        {
            self.accepted = None;
            self.evidence.clear();
            self.execution.selected_candidate = None;
            terminal = no_result(status);
        }
        let warnings = self
            .accepted
            .as_ref()
            .map(|result| result.verification.warnings.clone())
            .unwrap_or_default();
        self.manifest = RunManifestV1::new(
            self.run_input.run_id,
            self.run_input.checksum.clone(),
            terminal,
            self.operation.started_at,
            finished_at,
            Some(elapsed),
            self.manifest.first_incumbent_milliseconds,
            self.accepted
                .as_ref()
                .and(self.manifest.first_verified_feasible_milliseconds),
            self.manifest.phase_timings,
            warnings,
        )
        .map_err(|_| timing_error())?;
        Ok(())
    }
}

fn observed_elapsed(budget: &ParentSolveBudget) -> Result<Duration, AppError> {
    budget.checked_elapsed().ok_or_else(timing_error)
}

fn milliseconds(duration: Duration) -> Result<DurationMillis, AppError> {
    u64::try_from(duration.as_millis())
        .ok()
        .and_then(|value| DurationMillis::new(value).ok())
        .ok_or_else(timing_error)
}

fn elapsed_between(start: Duration, finish: Duration) -> Result<DurationMillis, AppError> {
    finish
        .checked_sub(start)
        .ok_or_else(timing_error)
        .and_then(milliseconds)
}

fn timing_error() -> AppError {
    solve_error(
        "solve.clock_invalid",
        "The operation timing cannot be represented safely.",
    )
}

fn solve_error(code: &str, message: &str) -> AppError {
    protocol_error(code, message, false)
}

fn no_result(status: SolveStatus) -> RunTerminalOutcomeV1 {
    RunTerminalOutcomeV1::NoResult { status }
}

fn interruption_status(reason: OperationInterruption) -> SolveStatus {
    match reason {
        OperationInterruption::Cancelled => SolveStatus::Cancelled,
        OperationInterruption::DeadlineExceeded => SolveStatus::NoSolutionWithinLimit,
    }
}

fn preparation_failure(
    error: AppError,
    control: &OperationControl,
) -> Box<SolvePreparationFailure> {
    let status = match &error {
        AppError::Protocol(failure) if failure.code == "solve.backend_unavailable" => {
            SolveStatus::BackendUnavailable
        }
        AppError::Protocol(failure) if failure.code == "solve.clock_invalid" => {
            SolveStatus::BackendFailed
        }
        _ => SolveStatus::InvalidModel,
    };
    Box::new(SolvePreparationFailure {
        status: control.check().err().map_or(status, interruption_status),
        error,
        validation: None,
        routing: None,
    })
}

fn publication_error(error: &ExportError, control: &OperationControl) -> AppError {
    control
        .check()
        .err()
        .map_or_else(|| export_error(error), operation_interrupted)
}
