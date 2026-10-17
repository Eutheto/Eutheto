use super::super::{
    OrdinaryObservationBounds, OrdinarySolveLease, available_pack, domain_interruption,
    export_error, operation_interrupted, protocol_error, solution_contract_error,
};
use super::HeadlessService;
use crate::RouterCandidateReviewer;
use crate::infeasibility::{InertInfeasibilityEvidence, capture_original_infeasibility};
use crate::infeasibility_diagnostic::{
    InfeasibilityDiagnosticContext, InfeasibilityDiagnosticMaterial, diagnose_infeasibility,
};
use crate::verification::{
    ParentVerificationClock, accepted_evidence, runtime_evidence_matches, terminal_phase_timings,
};
use eutheto_domain_api::{CompileContext, DomainValidationReport};
use eutheto_domain_ir::{
    AcceptedResult, DomainEvidenceId, PortableAcceptedResult, RunInput, RunManifest,
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
    AppError, BackendSelection, CancellationToken, Clock, DurationMillis, ExplanationMode,
    KnownVerificationAlarmV1, OperationControl, OperationInterruption, ParentSolveBudget,
    RequestId, Rfc3339Timestamp, ScenarioSnapshotId, ScenarioSnapshotV1, SolutionId, SolveOptions,
    SolveRunId, SolveStatus, VerificationAlarmTerminalization, VerificationFailure,
};
use eutheto_verify::{AcceptanceDecision, CorrectnessAlarm, CorrectnessAlarmCategory};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

/// Bounded handoff from independent verification to the store owner. The router never waits
/// for `SQLite` or emits a result before this record is committed.
struct FirstAccepted {
    result: AcceptedResult,
    observed_after_milliseconds: DurationMillis,
    verified_after_milliseconds: DurationMillis,
    accepted_at: Rfc3339Timestamp,
}

/// Trusted in-process custody captured before any later timing or terminalization can fail.
#[derive(Clone, Debug)]
pub(crate) struct NativeAlarmCustody {
    pub(crate) run_id: SolveRunId,
    pub(crate) input_checksum: String,
    pub(crate) started_at: Rfc3339Timestamp,
    pub(crate) alarm: CorrectnessAlarm,
    pub(crate) trusted_elapsed_bound: Option<Duration>,
    /// Greatest trusted actual wall-clock sample observed for this run.
    pub(crate) trusted_wall_bound: Rfc3339Timestamp,
    /// An unrecovered execution-time wall regression; a later cleanup sample may recover trust.
    pub(crate) wall_observation_failed: bool,
}

#[derive(Clone, Copy, Debug)]
struct NativeWallObservations {
    latest: Rfc3339Timestamp,
    regressed: bool,
}

impl NativeWallObservations {
    fn new(started_at: Rfc3339Timestamp) -> Self {
        Self {
            latest: started_at,
            regressed: false,
        }
    }

    fn observe(&mut self, observed_at: Rfc3339Timestamp) {
        if observed_at < self.latest {
            self.regressed = true;
        } else {
            self.latest = observed_at;
            self.regressed = false;
        }
    }
}

impl NativeAlarmCustody {
    pub(crate) fn new(
        input: &RunInput,
        started_at: Rfc3339Timestamp,
        alarm: CorrectnessAlarm,
        trusted_elapsed_bound: Option<Duration>,
    ) -> Self {
        Self::from_identity(
            input.run_id,
            input.checksum.clone(),
            started_at,
            alarm,
            trusted_elapsed_bound,
        )
    }

    fn from_identity(
        run_id: SolveRunId,
        input_checksum: String,
        started_at: Rfc3339Timestamp,
        alarm: CorrectnessAlarm,
        trusted_elapsed_bound: Option<Duration>,
    ) -> Self {
        Self {
            run_id,
            input_checksum,
            started_at,
            alarm,
            trusted_elapsed_bound,
            trusted_wall_bound: started_at,
            wall_observation_failed: false,
        }
    }

    pub(crate) fn retain_elapsed_bound(&mut self, elapsed: Duration) {
        self.trusted_elapsed_bound = Some(
            self.trusted_elapsed_bound
                .map_or(elapsed, |previous| previous.max(elapsed)),
        );
    }

    fn retain_wall_observations(&mut self, observations: NativeWallObservations) {
        if observations.latest < self.trusted_wall_bound {
            self.wall_observation_failed = true;
        } else {
            self.trusted_wall_bound = observations.latest;
            self.wall_observation_failed = observations.regressed;
        }
    }

    fn retain_wall_observation(&mut self, observed_at: Rfc3339Timestamp) {
        if observed_at < self.trusted_wall_bound {
            self.wall_observation_failed = true;
        } else {
            self.trusted_wall_bound = observed_at;
            self.wall_observation_failed = false;
        }
    }
}

fn snapshot_wall_observations(
    observations: &Mutex<NativeWallObservations>,
) -> NativeWallObservations {
    match observations.lock() {
        Ok(observations) => *observations,
        Err(poisoned) => {
            let mut observations = *poisoned.into_inner();
            observations.regressed = true;
            observations
        }
    }
}

fn record_wall_observation(
    observations: &Mutex<NativeWallObservations>,
    observed_at: Rfc3339Timestamp,
) {
    match observations.lock() {
        Ok(mut observations) => observations.observe(observed_at),
        Err(poisoned) => {
            let mut observations = poisoned.into_inner();
            observations.observe(observed_at);
            observations.regressed = true;
        }
    }
}

#[path = "headless_stored.rs"]
mod stored;
pub use stored::*;

#[path = "headless_repair.rs"]
mod repair;
pub use repair::RepairSource;

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
    remaining_members: Option<u8>,
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
    objective_context: eutheto_domain_ir::RunObjectiveContext,
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
    run_input: RunInput,
    manifest: RunManifest,
    pub compilation: HeadlessCompilationReport,
    pub routing: RoutingDecision,
    pub execution: RouterExecutionRecord,
    accepted: Option<AcceptedResult>,
    evidence: BTreeMap<DomainEvidenceId, VerificationValue>,
    alarm: Option<CorrectnessAlarm>,
    infeasibility: Option<InertInfeasibilityEvidence>,
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
            remaining_members: None,
        })
    }

    /// Starts a solve operation with a parent budget anchored before native
    /// admission. The caller must provide the same budget duration encoded in
    /// `options`; the operation never creates a second deadline.
    ///
    /// # Errors
    /// Rejects invalid options or run-identity allocation failure.
    pub fn begin_solve_with_budget(
        &self,
        request_id: RequestId,
        options: SolveOptions,
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
    ) -> Result<SolveOperation, AppError> {
        options
            .validate()
            .map_err(|_| solve_error("solve.options_invalid", "Solve options are invalid."))?;
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
            remaining_members: None,
        })
    }
}

impl SolveOperation {
    fn dispatch_acceptance_reserve(&self) -> Result<DurationMillis, AppError> {
        let Some(remaining_members) = self.remaining_members else {
            return Ok(self.acceptance_reserve);
        };
        let remaining = self.budget.snapshot().remaining_milliseconds.value();
        let share = remaining.saturating_sub(self.acceptance_reserve.value())
            / u64::from(remaining_members);
        DurationMillis::new(remaining - share).map_err(|_| timing_error())
    }

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
        self.prepare_captured_with_objective_context(snapshot, bundle, None)
    }

    #[allow(clippy::too_many_lines)]
    fn prepare_captured_with_objective_context(
        self,
        snapshot: ScenarioSnapshotV1,
        bundle: Option<InspectedBundle>,
        objective_context: Option<eutheto_domain_ir::RunObjectiveContext>,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        control
            .check()
            .map_err(|reason| preparation_failure(operation_interrupted(reason), &control))?;
        let validation = self
            .service
            .validate_full(&snapshot, &control)
            .map_err(|error| preparation_failure(error, &control))?;
        let objective_context = match objective_context {
            Some(context) => context,
            None => eutheto_domain_ir::RunObjectiveContext::authored(
                snapshot.document.domain_pack.id.clone(),
            )
            .map_err(|_| preparation_failure(solution_contract_error(), &control))?,
        };
        objective_context
            .validate_for(
                &snapshot.document.domain_pack.id,
                snapshot.document.scenario_id,
            )
            .map_err(|_| preparation_failure(solution_contract_error(), &control))?;
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
                        objective_context: Some(objective_context.clone()),
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
                    objective_context,
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
        let mut alarm_custody = None;
        let result = self
            .execute_bound(input, progress, None, &mut alarm_custody, None)
            .await;
        match (result, alarm_custody.as_ref()) {
            (Err(error), Some(custody)) => Err(known_verification_alarm_error(
                custody,
                VerificationAlarmTerminalization::NotApplicable,
                safe_verification_secondary_cause(&error),
            )),
            (result, _) => result,
        }
    }

    fn make_run_input(
        &self,
        snapshot_id: ScenarioSnapshotId,
        snapshot_created_at: Rfc3339Timestamp,
    ) -> Result<RunInput, AppError> {
        RunInput::new(
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
            Some(self.objective_context.clone()),
        )
        .map_err(|_| solution_contract_error())
    }

    // Candidate admission, deadline revocation and terminal milestones remain one transition.
    async fn execute_bound(
        self,
        input: RunInput,
        progress: &mut dyn ProgressSink,
        first_accepted: Option<tokio::sync::mpsc::Sender<FirstAccepted>>,
        alarm_custody: &mut Option<NativeAlarmCustody>,
        ordinary_lease: Option<&OrdinarySolveLease>,
    ) -> Result<HeadlessSolveOutcome, AppError> {
        let clock = ParentVerificationClock::new(&self.operation.budget);
        let started_at = self.operation.started_at;
        let compile_finished = self.compile_finished;
        let wall_observations = Mutex::new(NativeWallObservations::new(started_at));
        let result = Box::pin(catch_future_poll(self.execute_bound_inner(
            input,
            progress,
            first_accepted,
            alarm_custody,
            &clock,
            ordinary_lease,
            &wall_observations,
        )))
        .await;
        if result.is_err()
            && let Some(lease) = ordinary_lease
            && let Some(elapsed) = clock.latest_elapsed_bound()
        {
            lease.observe_elapsed(elapsed);
        }
        if let Some(custody) = alarm_custody.as_mut() {
            custody.retain_elapsed_bound(compile_finished);
            if let Some(elapsed) = clock.latest_elapsed_bound() {
                custody.retain_elapsed_bound(elapsed);
            }
            custody.retain_wall_observations(snapshot_wall_observations(&wall_observations));
        }
        match result {
            Ok(result) => result,
            Err(payload) if alarm_custody.is_some() => {
                drop(payload);
                Err(solution_contract_error())
            }
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    // These distinct inputs form one execution transition; a one-use bundle adds no value.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn execute_bound_inner(
        self,
        input: RunInput,
        progress: &mut dyn ProgressSink,
        first_accepted: Option<tokio::sync::mpsc::Sender<FirstAccepted>>,
        alarm_custody: &mut Option<NativeAlarmCustody>,
        clock: &ParentVerificationClock,
        ordinary_lease: Option<&OrdinarySolveLease>,
        wall_observations: &Mutex<NativeWallObservations>,
    ) -> Result<HeadlessSolveOutcome, AppError> {
        if input != self.make_run_input(input.snapshot_id, input.snapshot_created_at)? {
            return Err(solution_contract_error());
        }
        let pack = available_pack(&self.snapshot.document, &self.operation.service.packs)
            .ok_or_else(solution_contract_error)?;
        if let Some(lease) = ordinary_lease {
            lease.observe_elapsed(self.compile_finished);
        }
        clock.retain_elapsed_bound(self.compile_finished);
        let control = self.operation.control();
        let mut reviewer = match RouterCandidateReviewer::new(
            pack,
            &self.snapshot.document,
            self.snapshot.revision.value(),
            &self.problem,
            input.objective_context.as_ref(),
            clock,
            self.operation.service.ids.as_ref(),
            &control,
        ) {
            Ok(reviewer) => reviewer,
            Err(alarm) if alarm.category != CorrectnessAlarmCategory::ClockFailed => {
                let mut custody = NativeAlarmCustody::new(
                    &input,
                    self.operation.started_at,
                    alarm,
                    clock.latest_elapsed_bound(),
                );
                custody.retain_wall_observations(snapshot_wall_observations(wall_observations));
                *alarm_custody = Some(custody);
                if let Some(lease) = ordinary_lease
                    && let Some(custody) = alarm_custody.as_ref()
                {
                    lease.capture_alarm(&custody.alarm);
                }
                return Err(solution_contract_error());
            }
            Err(_) => return Err(timing_error()),
        };
        let run_id = input.run_id;
        let input_checksum = input.checksum.as_str();
        let started_at = self.operation.started_at;
        let alarm_custody_ref = &mut *alarm_custody;
        let wall_observations_ref = wall_observations;
        let clock_ref = clock;
        let ordinary_lease_ref = ordinary_lease;
        reviewer = reviewer.with_after_review(move |decision| {
            let AcceptanceDecision::Quarantined { alarm, .. } = decision else {
                return;
            };
            if alarm.category == CorrectnessAlarmCategory::ClockFailed {
                return;
            }
            if alarm_custody_ref.is_none() {
                let mut custody = NativeAlarmCustody::from_identity(
                    run_id,
                    input_checksum.to_owned(),
                    started_at,
                    alarm.clone(),
                    clock_ref.latest_elapsed_bound(),
                );
                custody.retain_wall_observations(snapshot_wall_observations(wall_observations_ref));
                *alarm_custody_ref = Some(custody);
            }
            if let Some(lease) = ordinary_lease_ref {
                if let Some(custody) = alarm_custody_ref.as_ref() {
                    lease.capture_alarm(&custody.alarm);
                }
                if let Some(elapsed) = clock_ref.latest_elapsed_bound() {
                    lease.observe_elapsed(elapsed);
                }
            }
        });

        let first_accepted_error = first_accepted.as_ref().map(|_| Arc::new(Mutex::new(None)));
        if let Some(sender) = first_accepted {
            let wall_clock = Arc::clone(&self.operation.service.clock);
            let first_accepted_error_ref = first_accepted_error.as_ref().map(Arc::clone);
            let mut accepted_recorded = false;
            reviewer = reviewer.with_first_accepted(move |result, observed, verified| {
                let accepted_at = wall_clock.now();
                record_wall_observation(wall_observations, accepted_at);
                if !accepted_recorded {
                    if let Some(lease) = ordinary_lease
                        && let Err(error) = lease.record_first_accepted(
                            result.solution.solution_id,
                            &result.checksum,
                            accepted_at,
                        )
                    {
                        lease.observe_wall(accepted_at);
                        if let Some(error_slot) = first_accepted_error_ref.as_ref() {
                            *error_slot
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                        }
                        return;
                    }
                    accepted_recorded = true;
                } else if let Some(lease) = ordinary_lease {
                    lease.observe_wall(accepted_at);
                }
                let _ = sender.try_send(FirstAccepted {
                    result: result.clone(),
                    observed_after_milliseconds: observed,
                    verified_after_milliseconds: verified,
                    accepted_at,
                });
            });
        }
        // Fix the member's reserve once; router fallbacks consume the same parent and reserve.
        let acceptance_reserve = self.operation.dispatch_acceptance_reserve()?;
        let execution = SolverRouter::new(&self.operation.service.solvers)
            .execute(
                Arc::clone(&self.problem),
                self.operation.options.clone(),
                &self.operation.budget,
                acceptance_reserve,
                progress,
                &mut reviewer,
            )
            .await;
        let native_observations = Arc::new(
            crate::native_observations::capture(
                &input,
                &self.problem,
                &self.operation.service.solvers,
                &execution.record,
            )
            .map_err(|_| solution_contract_error())?,
        );
        if let Some(lease) = ordinary_lease {
            lease.retain_native_observations(&input, Arc::clone(&native_observations))?;
        }
        let review_timings = reviewer.total_review_timings();
        let decision = reviewer.into_decision();
        if let Some(elapsed) = execution.last_trusted_elapsed {
            clock.retain_elapsed_bound(elapsed);
        }
        if let Some(observations) = execution.observations.as_ref() {
            clock.retain_elapsed_bound(observations.started_after);
            clock.retain_elapsed_bound(observations.finished_after);
        }
        if let Some(lease) = ordinary_lease
            && let Some(elapsed) = execution.last_trusted_elapsed
        {
            lease.observe_elapsed(elapsed);
        }
        if let Some(custody) = alarm_custody.as_mut() {
            if let Some(elapsed) = execution.last_trusted_elapsed {
                custody.retain_elapsed_bound(elapsed);
            }
            if let Some(elapsed) = clock.latest_elapsed_bound() {
                custody.retain_elapsed_bound(elapsed);
            }
            custody.retain_wall_observations(snapshot_wall_observations(wall_observations));
        }
        let first_accepted_error = first_accepted_error.and_then(|error_slot| {
            error_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        });
        if let Some(error) = first_accepted_error {
            return Err(error);
        }
        let review_timings = review_timings.map_err(|()| timing_error())?;
        if clock.has_failed() {
            return Err(timing_error());
        }
        let runtime_matches =
            runtime_evidence_matches(&execution.record, &self.identity, &self.operation.options);
        let backend = self
            .operation
            .service
            .solvers
            .get(self.identity.backend_id())
            .ok_or_else(solution_contract_error)?;
        let original_candidate = runtime_matches
            && execution.record.terminal_status == SolveStatus::Infeasible
            && decision.is_none();
        let original_infeasibility = if original_candidate {
            let observed_at = self.operation.service.clock.now();
            record_wall_observation(wall_observations, observed_at);
            if let Some(lease) = ordinary_lease {
                lease.observe_wall(observed_at);
            }
            capture_original_infeasibility(
                &input,
                backend.as_ref(),
                &self.operation.budget,
                self.operation.started_at,
                observed_at,
            )?
        } else {
            None
        };
        let original_observation_late = original_candidate && original_infeasibility.is_none();
        let observations = execution.observations.ok_or_else(timing_error)?;
        elapsed_between(self.compile_finished, observations.started_after)?;
        let backend_elapsed = if execution.record.invocation_count == 0 {
            None
        } else {
            Some(milliseconds(observations.backend_elapsed)?)
        };
        let mut phases =
            terminal_phase_timings(review_timings, self.compile_elapsed, backend_elapsed);
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
        if original_observation_late {
            outcome = no_result(if self.operation.budget.is_cancelled() {
                SolveStatus::Cancelled
            } else {
                SolveStatus::NoSolutionWithinLimit
            });
        }
        let mut infeasibility = None;
        if let Some(original) = original_infeasibility
            && matches!(
                outcome,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible
                }
            )
        {
            let diagnostic_started =
                if self.operation.options.explanation_mode == ExplanationMode::None {
                    None
                } else {
                    let elapsed = clock.observe_elapsed().map_err(|()| timing_error())?;
                    if let Some(lease) = ordinary_lease {
                        lease.observe_elapsed(elapsed);
                    }
                    Some(elapsed)
                };
            let compile_context = CompileContext {
                objective_context: input.objective_context.clone(),
                scenario_revision: self.snapshot.revision.value(),
                semantic_metadata: BTreeMap::new(),
                control: control.clone(),
                planning_limits: PlanningIrLimitsV1::DEFAULT
                    .tightened_by(self.operation.options.resource_limits),
            };
            let material = diagnose_infeasibility(InfeasibilityDiagnosticContext {
                pack,
                document: &self.snapshot.document,
                base_problem: &self.problem,
                compile_context: &compile_context,
                backend: backend.as_ref(),
                parent_budget: &self.operation.budget,
                verification_clock: clock,
                ids: self.operation.service.ids.as_ref(),
                options: &self.operation.options,
                condition: None,
            })
            .await?;
            match material {
                InfeasibilityDiagnosticMaterial::Evidence {
                    infeasibility: evidence,
                    proof,
                } => {
                    infeasibility = Some(InertInfeasibilityEvidence {
                        original,
                        condition: None,
                        diagnostic: proof,
                        infeasibility: evidence,
                    });
                }
                InfeasibilityDiagnosticMaterial::Quarantined {
                    alarm: correctness_alarm,
                } => {
                    if correctness_alarm.category == CorrectnessAlarmCategory::ClockFailed {
                        return Err(timing_error());
                    }
                    if alarm_custody.is_none() {
                        let mut custody = NativeAlarmCustody::new(
                            &input,
                            self.operation.started_at,
                            correctness_alarm.clone(),
                            clock.latest_elapsed_bound(),
                        );
                        custody.retain_wall_observations(snapshot_wall_observations(
                            wall_observations,
                        ));
                        *alarm_custody = Some(custody);
                    }
                    if let Some(lease) = ordinary_lease
                        && let Some(custody) = alarm_custody.as_ref()
                    {
                        lease.capture_alarm(&custody.alarm);
                    }
                    outcome = RunTerminalOutcomeV1::VerificationAlarm {
                        diagnostic_code: correctness_alarm.diagnostic_code.clone(),
                    };
                    alarm = Some(correctness_alarm);
                }
            }
            if let Some(diagnostic_started) = diagnostic_started {
                let diagnostic_finished = clock.observe_elapsed().map_err(|()| timing_error())?;
                if let Some(lease) = ordinary_lease {
                    lease.observe_elapsed(diagnostic_finished);
                }
                phases.optional_explanation_milliseconds =
                    Some(elapsed_between(diagnostic_started, diagnostic_finished)?);
            }
        }
        let evidence_started = clock.observe_elapsed().map_err(|()| timing_error())?;
        if let Some(lease) = ordinary_lease {
            lease.observe_elapsed(evidence_started);
        }
        elapsed_between(observations.finished_after, evidence_started)?;
        let mut evidence = accepted.as_ref().map(accepted_evidence).unwrap_or_default();
        let evidence_finished = clock.observe_elapsed().map_err(|()| timing_error())?;
        if let Some(lease) = ordinary_lease {
            lease.observe_elapsed(evidence_finished);
        }
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
        record_wall_observation(wall_observations, finished_at);
        if let Some(lease) = ordinary_lease {
            lease.observe_wall(finished_at);
        }
        if let Some(custody) = alarm_custody.as_mut() {
            custody.retain_wall_observations(snapshot_wall_observations(wall_observations));
        }
        let last_observed_elapsed = clock.observe_elapsed().map_err(|()| timing_error())?;
        if let Some(lease) = ordinary_lease {
            lease.observe_elapsed(last_observed_elapsed);
        }
        if clock.has_failed() {
            return Err(timing_error());
        }
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
        if let Some(custody) = alarm_custody.as_mut() {
            custody.retain_elapsed_bound(last_observed_elapsed);
            custody.retain_wall_observations(snapshot_wall_observations(wall_observations));
            accepted = None;
            evidence.clear();
            infeasibility = None;
            outcome = RunTerminalOutcomeV1::VerificationAlarm {
                diagnostic_code: custody.alarm.diagnostic_code.clone(),
            };
            alarm = Some(custody.alarm.clone());
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
        let manifest = if let Some(custody) = alarm_custody.as_ref() {
            let elapsed_is_trusted = custody
                .trusted_elapsed_bound
                .is_some_and(|bound| last_observed_elapsed >= bound);
            if custody.wall_observation_failed
                || finished_at < custody.trusted_wall_bound
                || !elapsed_is_trusted
            {
                RunManifest::new_clock_failed_alarm(
                    custody.run_id,
                    custody.input_checksum.clone(),
                    custody.alarm.diagnostic_code.clone(),
                    custody.started_at,
                    finished_at,
                )
                .map_err(|_| timing_error())?
            } else {
                RunManifest::new(
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
                .map_err(|_| timing_error())?
            }
        } else {
            RunManifest::new(
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
            .map_err(|_| timing_error())?
        };
        let manifest = manifest
            .with_native_observations(Some(Arc::unwrap_or_clone(native_observations)))
            .map_err(|_| solution_contract_error())?;
        Ok(HeadlessSolveOutcome {
            run_input: input,
            manifest,
            compilation: self.report,
            routing: self.routing,
            execution: record,
            accepted,
            evidence,
            alarm,
            infeasibility,
            operation: self.operation,
            last_observed_elapsed,
        })
    }
}

impl HeadlessSolveOutcome {
    /// Immutable execution bindings cannot be substituted before file publication.
    #[must_use]
    pub fn run_input(&self) -> &RunInput {
        &self.run_input
    }

    /// The independently established terminal classification, not a caller-authored claim.
    #[must_use]
    pub fn manifest(&self) -> &RunManifest {
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
    pub fn into_portable(mut self) -> Result<Option<PortableAcceptedResult>, AppError> {
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
            PortableAcceptedResult::new(self.run_input, self.manifest, accepted, self.evidence)
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
        self.refresh_admission_with_lease(None)
    }

    fn refresh_admission_with_lease(
        &mut self,
        ordinary_lease: Option<&OrdinarySolveLease>,
    ) -> Result<(), AppError> {
        if self.alarm.is_some() {
            return Ok(());
        }
        let interruption = self
            .operation
            .control()
            .check()
            .err()
            .map(interruption_status);
        let finished_at = observed_wall(self.operation.service.clock.as_ref())?;
        if let Some(lease) = ordinary_lease {
            lease.observe_wall(finished_at);
        }
        let observed = observed_elapsed(&self.operation.budget)?;
        if let Some(lease) = ordinary_lease {
            lease.observe_elapsed(observed);
            if !ordinary_terminal_observation_is_trusted(
                lease.observation_bounds()?,
                finished_at,
                Some(observed),
            ) {
                return Err(timing_error());
            }
        }
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
        if self.infeasibility.is_none()
            && matches!(
                terminal,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible
                }
            )
            && let Some(status) = status
        {
            terminal = no_result(status);
        }
        let warnings = self
            .accepted
            .as_ref()
            .map(|result| result.verification.warnings.clone())
            .unwrap_or_default();
        self.manifest = RunManifest::new(
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
        .and_then(|manifest| {
            manifest.with_native_observations(self.manifest.native_observations.clone())
        })
        .map_err(|_| timing_error())?;
        Ok(())
    }
}

fn observed_wall(clock: &dyn Clock) -> Result<Rfc3339Timestamp, AppError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| clock.now())).map_err(|payload| {
        drop(payload);
        timing_error()
    })
}
fn observed_elapsed(budget: &ParentSolveBudget) -> Result<Duration, AppError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| budget.checked_elapsed()))
        .map_err(|payload| {
            drop(payload);
            timing_error()
        })?
        .ok_or_else(timing_error)
}

fn milliseconds(duration: Duration) -> Result<DurationMillis, AppError> {
    u64::try_from(duration.as_millis())
        .ok()
        .and_then(|value| DurationMillis::new(value).ok())
        .ok_or_else(timing_error)
}

fn ordinary_terminal_observation_is_trusted(
    bounds: OrdinaryObservationBounds,
    finished_at: Rfc3339Timestamp,
    elapsed: Option<Duration>,
) -> bool {
    !bounds.unreliable
        && bounds
            .latest_wall
            .is_none_or(|latest| finished_at >= latest)
        && bounds
            .latest_elapsed
            .is_none_or(|latest| elapsed.is_some_and(|elapsed| elapsed >= latest))
}

fn elapsed_between(start: Duration, finish: Duration) -> Result<DurationMillis, AppError> {
    finish
        .checked_sub(start)
        .ok_or_else(timing_error)
        .and_then(milliseconds)
}
async fn catch_future_poll<F: Future>(
    future: F,
) -> Result<F::Output, Box<dyn std::any::Any + Send>> {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|context| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            future.as_mut().poll(context)
        })) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    })
    .await
}

pub(crate) fn known_verification_alarm_metadata(
    custody: &NativeAlarmCustody,
    terminalization: VerificationAlarmTerminalization,
    secondary_cause: Option<&str>,
) -> Option<KnownVerificationAlarmV1> {
    KnownVerificationAlarmV1::new(
        custody.run_id,
        &custody.input_checksum,
        &custody.alarm.diagnostic_code,
        terminalization,
        secondary_cause,
    )
    .ok()
}

pub(crate) fn known_verification_alarm_error(
    custody: &NativeAlarmCustody,
    terminalization: VerificationAlarmTerminalization,
    secondary_cause: Option<&str>,
) -> AppError {
    match known_verification_alarm_metadata(custody, terminalization, secondary_cause) {
        Some(known_alarm) => {
            AppError::Verification(VerificationFailure::from_known_alarm(known_alarm, None))
        }
        None => AppError::Verification(VerificationFailure {
            code: custody.alarm.diagnostic_code.clone(),
            message: VerificationFailure::KNOWN_ALARM_MESSAGE.to_owned(),
            retryable: false,
            diagnostic_id: None,
            known_alarm: None,
        }),
    }
}

pub(crate) fn safe_verification_secondary_cause(error: &AppError) -> Option<&'static str> {
    match error {
        AppError::Protocol(failure) => match failure.code.as_str() {
            "solve.clock_invalid" => Some("solve.clock_invalid"),
            "operation.cancelled" => Some("operation.cancelled"),
            "operation.deadline_exceeded" => Some("operation.deadline_exceeded"),
            "operation.resource_limit" => Some("operation.resource_limit"),
            _ => None,
        },
        AppError::Storage(failure) => Some(if failure.code == "storage.private_path_unsafe" {
            "storage.private_path_unsafe"
        } else {
            "storage.operation_failed"
        }),
        AppError::Verification(failure) => failure
            .known_alarm
            .as_ref()
            .and_then(KnownVerificationAlarmV1::secondary_cause_code)
            .map(eutheto_types::KnownVerificationSecondaryCauseCode::as_str),
        _ => None,
    }
}

pub(in crate::application) fn timing_error() -> AppError {
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
        AppError::Protocol(failure) if failure.code == "operation.resource_limit" => {
            SolveStatus::NoSolutionWithinLimit
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
    if matches!(
        error,
        ExportError::Cancelled | ExportError::DeadlineExceeded
    ) {
        return export_error(error);
    }
    control
        .check()
        .err()
        .map_or_else(|| export_error(error), operation_interrupted)
}

#[cfg(test)]
mod known_alarm_error_tests {
    use super::*;

    #[test]
    fn file_alarm_error_keeps_fixed_verification_identity_and_safe_cause()
    -> Result<(), Box<dyn std::error::Error>> {
        let run_id: SolveRunId = "018f47f2-e880-7000-8000-000000000010".parse()?;
        let custody = NativeAlarmCustody {
            run_id,
            input_checksum: "ab".repeat(32),
            started_at: Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?,
            alarm: CorrectnessAlarm {
                category: CorrectnessAlarmCategory::ProjectionFailed,
                diagnostic_code: CorrectnessAlarmCategory::ProjectionFailed.code().to_owned(),
            },
            trusted_elapsed_bound: Some(Duration::from_millis(12)),
            trusted_wall_bound: Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?,
            wall_observation_failed: false,
        };
        let AppError::Verification(failure) = known_verification_alarm_error(
            &custody,
            VerificationAlarmTerminalization::NotApplicable,
            Some("solve.clock_invalid"),
        ) else {
            return Err(std::io::Error::other(
                "known alarm did not retain Verification classification",
            )
            .into());
        };
        assert_eq!(
            failure.code,
            CorrectnessAlarmCategory::ProjectionFailed.code()
        );
        assert_eq!(failure.message, VerificationFailure::KNOWN_ALARM_MESSAGE);
        let expected_checksum = "ab".repeat(32);
        let alarm = failure
            .known_alarm
            .ok_or_else(|| std::io::Error::other("known alarm context was missing"))?;
        assert_eq!(alarm.run_id(), run_id);
        assert_eq!(alarm.input_checksum(), expected_checksum.as_str());
        assert_eq!(
            alarm.diagnostic_code().as_str(),
            "verification.projection_failed"
        );
        assert_eq!(
            alarm.terminalization(),
            VerificationAlarmTerminalization::NotApplicable
        );
        assert_eq!(
            alarm
                .secondary_cause_code()
                .map(eutheto_types::KnownVerificationSecondaryCauseCode::as_str),
            Some("solve.clock_invalid")
        );
        Ok(())
    }

    #[test]
    fn delayed_checkpoint_event_bound_rejects_stale_non_alarm_final_stamp()
    -> Result<(), Box<dyn std::error::Error>> {
        let accepted_at = Rfc3339Timestamp::parse("2026-01-01T00:00:01Z")?;
        let execute_finished_at = Rfc3339Timestamp::parse("2026-01-01T00:00:02Z")?;
        let event_at = Rfc3339Timestamp::parse("2026-01-01T00:00:03Z")?;
        let final_at = Rfc3339Timestamp::parse("2026-01-01T00:00:02.500Z")?;
        assert!(accepted_at <= execute_finished_at);
        assert!(execute_finished_at < event_at);
        assert!(accepted_at <= final_at && final_at < event_at);

        let event_bounds = OrdinaryObservationBounds {
            latest_wall: Some(event_at),
            latest_elapsed: Some(Duration::from_millis(200)),
            unreliable: false,
        };
        assert!(!ordinary_terminal_observation_is_trusted(
            event_bounds,
            final_at,
            Some(Duration::from_millis(250)),
        ));
        let elapsed_regressed_bounds = OrdinaryObservationBounds {
            latest_elapsed: Some(Duration::from_millis(300)),
            ..event_bounds
        };
        assert!(!ordinary_terminal_observation_is_trusted(
            elapsed_regressed_bounds,
            Rfc3339Timestamp::parse("2026-01-01T00:00:04Z")?,
            Some(Duration::from_millis(250)),
        ));
        let regressed_bounds = OrdinaryObservationBounds {
            unreliable: true,
            ..event_bounds
        };
        assert!(!ordinary_terminal_observation_is_trusted(
            regressed_bounds,
            Rfc3339Timestamp::parse("2026-01-01T00:00:04Z")?,
            Some(Duration::from_millis(400)),
        ));
        Ok(())
    }
}

#[cfg(test)]
mod alternative_member_budget_tests {
    use super::*;
    use eutheto_types::{
        FixedMonotonicClock, PreservationPolicy, ReproducibilityMode, ResourceLimits, SolveMode,
        SystemClock, SystemIdGenerator, WorkerThreadPolicy,
    };

    fn options() -> Result<SolveOptions, Box<dyn std::error::Error>> {
        Ok(SolveOptions {
            backend: BackendSelection::Auto,
            mode: SolveMode::Balanced,
            time_limit_milliseconds: DurationMillis::new(10_000)?,
            memory_limit_bytes: None,
            worker_threads: WorkerThreadPolicy::Exact(1),
            random_seed: 1,
            solution_limit: None,
            stop_after_first_feasible: false,
            collect_intermediate_solutions: false,
            explanation_mode: ExplanationMode::None,
            preserve_existing: PreservationPolicy::None,
            reproducibility: ReproducibilityMode::Deterministic,
            resource_limits: ResourceLimits {
                max_entities: 100,
                max_rules: 100,
                max_variables: 100,
                max_constraints: 100,
            },
        })
    }

    #[test]
    fn member_reserve_divides_only_remaining_worker_time() -> Result<(), Box<dyn std::error::Error>>
    {
        let clock = Arc::new(FixedMonotonicClock::default());
        let service = HeadlessService::new(
            Arc::new(SystemClock),
            clock.clone(),
            Arc::new(SystemIdGenerator),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let mut operation = service
            .begin_solve(
                RequestId::new(&SystemIdGenerator)?,
                options()?,
                CancellationToken::new(),
            )
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        assert_eq!(operation.acceptance_reserve.value(), 1_000);
        clock.advance(Duration::from_millis(1))?;
        for remaining_members in 1..=4 {
            operation.remaining_members = Some(remaining_members);
            let reserve = operation
                .dispatch_acceptance_reserve()
                .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
            assert_eq!(
                9_999 - reserve.value(),
                (9_999 - 1_000) / u64::from(remaining_members),
            );
        }
        for elapsed in [9_000, 9_001, 10_000] {
            clock.set(Duration::from_millis(elapsed))?;
            let remaining = operation.budget.snapshot().remaining_milliseconds;
            for remaining_members in 1..=4 {
                operation.remaining_members = Some(remaining_members);
                assert_eq!(
                    operation
                        .dispatch_acceptance_reserve()
                        .map_err(|error| std::io::Error::other(format!("{error:?}")))?,
                    remaining,
                );
            }
        }
        Ok(())
    }

    #[test]
    fn fixed_member_reserve_consumes_parent_without_changing_ordinary_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(FixedMonotonicClock::default());
        let service = HeadlessService::new(
            Arc::new(SystemClock),
            clock.clone(),
            Arc::new(SystemIdGenerator),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let original_options = options()?;
        let parent = ParentSolveBudget::new(
            original_options.time_limit_milliseconds,
            clock.clone(),
            CancellationToken::new(),
        )?;
        let started_at = SystemClock.now();
        let mut operation = service
            .begin_solve_with_budget(
                RequestId::new(&SystemIdGenerator)?,
                original_options.clone(),
                parent.clone(),
                started_at,
            )
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        clock.advance(Duration::from_secs(1))?;
        assert_eq!(
            operation
                .dispatch_acceptance_reserve()
                .map_err(|error| std::io::Error::other(format!("{error:?}")))?
                .value(),
            1_000,
        );
        operation.remaining_members = Some(4);
        let fixed_reserve = operation
            .dispatch_acceptance_reserve()
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        assert_eq!(fixed_reserve.value(), 7_000);
        // All later attempts subtract this fixed reserve, never allocate another member share.
        for (advance, expected_worker_time) in [(500, 1_500), (1_500, 0), (1, 0)] {
            clock.advance(Duration::from_millis(advance))?;
            assert_eq!(
                operation
                    .budget
                    .snapshot()
                    .remaining_milliseconds
                    .value()
                    .saturating_sub(fixed_reserve.value()),
                expected_worker_time,
            );
            assert_eq!(operation.budget.snapshot(), parent.snapshot());
        }
        operation.remaining_members = None;
        assert_eq!(
            operation
                .dispatch_acceptance_reserve()
                .map_err(|error| std::io::Error::other(format!("{error:?}")))?
                .value(),
            1_000,
        );
        assert_eq!(operation.options, original_options);
        assert_eq!(operation.started_at, started_at);
        assert_eq!(parent.checked_elapsed(), Some(Duration::from_millis(3_001)));
        Ok(())
    }
}
