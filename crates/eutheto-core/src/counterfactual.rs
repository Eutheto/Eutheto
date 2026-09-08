use crate::verification::{
    ParentVerificationClock, accepted_evidence, runtime_evidence_matches, terminal_phase_timings,
};
use crate::{AppEvent, RouterCandidateReviewer};
use eutheto_domain_api::{
    CompileContext, CounterfactualCompileContext, DomainPackError, DomainPackRegistry,
};
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
    COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1, CounterfactualCompilationBindingV1,
    CounterfactualConditionPayloadV1, CounterfactualConditionV1, CounterfactualFailureKind,
    CounterfactualJobErrorV1, CounterfactualJobRecordV1, CounterfactualJobRequestV1,
    CounterfactualJobState, CounterfactualRequestSemanticsV1, RunManifestV1, RunPhaseTimingsV1,
    RunTerminalOutcomeV1, VerificationValue, counterfactual_condition_satisfied,
};
use eutheto_explain::{
    CounterfactualInterpretation, interpret_counterfactual, validate_counterfactual_problem,
};
use eutheto_planning_ir::{PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, canonical_ir_hash};
use eutheto_solver_api::{
    BackendRuntimeIdentity, OutputError, ProgressSink, SolveProgressEvent, SolverRegistry,
};
use eutheto_solver_router::{
    ExecutionTerminalReason, RouterExecutionRecord, RouterObservations, SolverRouter,
};
use eutheto_store::{
    CandidateDiagnosticsV1, CounterfactualCancelOutcomeV1, CounterfactualJobTransitionV1,
    CounterfactualRunFinalizationV1, NewSolveRunV1, SqliteScenarioStore, StoreError,
    StoredAcceptedResultV2,
};
use eutheto_types::{
    AppError, BackendSelection, CancellationToken, Clock, CounterfactualJobId,
    CounterfactualProgressPhase, DurationMillis, EventContext, EventPayload, EventTopic,
    IdGenerator, MonotonicClock, ParentSolveBudget, ProtocolFailure, RequestId, ResourceRef,
    Revision, Rfc3339Timestamp, ScenarioId, SolutionId, SolveRunId, SolveStatus, SolverFailure,
    StorageFailure, ValidationIssue, ValidationReport, ValidationSeverity,
};
use eutheto_verify::AcceptanceDecision;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, btree_map::Entry};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Mutex, broadcast};

pub const COUNTERFACTUAL_API_SCHEMA_VERSION: u32 = 1;
pub const COUNTERFACTUAL_CONDITION_MISMATCH_DIAGNOSTIC: &str =
    "verification.counterfactual-condition-mismatch";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionStartCounterfactualRequestV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub base_solution_id: SolutionId,
    pub condition: CounterfactualConditionPayloadV1,
    pub total_budget_milliseconds: DurationMillis,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionCancelCounterfactualRequestV1 {
    pub schema_version: u32,
    pub cancel_request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub job_id: CounterfactualJobId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionStartCounterfactualDtoV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub current_revision: Revision,
    pub job: CounterfactualJobRecordV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionCancelCounterfactualDtoV1 {
    pub schema_version: u32,
    pub cancel_request_id: RequestId,
    pub current_revision: Revision,
    pub job: CounterfactualJobRecordV1,
}

pub(crate) struct CounterfactualRuntimeServices {
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) ids: Arc<dyn IdGenerator>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) events: broadcast::Sender<AppEvent>,
}

#[derive(Clone)]
pub(crate) struct CounterfactualRuntime {
    store: Arc<SqliteScenarioStore>,
    packs: Arc<DomainPackRegistry>,
    solvers: Arc<SolverRegistry>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    cancellation: CancellationToken,
    events: broadcast::Sender<AppEvent>,
    live: Arc<Mutex<BTreeMap<CounterfactualJobId, LiveJob>>>,
}

#[derive(Clone)]
struct LiveJob {
    cancellation: CancellationToken,
    state: Arc<StdMutex<LiveJobState>>,
}

#[derive(Default)]
struct LiveJobState {
    derived: Option<DerivedExecution>,
}

#[derive(Clone)]
struct DerivedExecution {
    input: eutheto_domain_ir::RunInputV1,
    started_at: Rfc3339Timestamp,
    budget: ParentSolveBudget,
}

struct FinalizeJobContext<'a> {
    request: &'a CounterfactualJobRequestV1,
    base: &'a StoredAcceptedResultV2,
    compilation: &'a CounterfactualCompilationBindingV1,
    input: &'a eutheto_domain_ir::RunInputV1,
    terminal: TerminalMaterial,
    budget: &'a ParentSolveBudget,
    previous_elapsed: Duration,
}

impl CounterfactualRuntime {
    pub(crate) fn new(
        store: Arc<SqliteScenarioStore>,
        packs: Arc<DomainPackRegistry>,
        solvers: Arc<SolverRegistry>,
        services: CounterfactualRuntimeServices,
    ) -> Self {
        Self {
            store,
            packs,
            solvers,
            clock: services.clock,
            ids: services.ids,
            cancellation: services.cancellation,
            events: services.events,
            live: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) async fn start(
        &self,
        request: SolutionStartCounterfactualRequestV1,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        let condition = validate_start_request(&request)?;
        let base = self
            .store
            .load_accepted_result(request.base_solution_id)
            .await
            .map_err(counterfactual_store_error)?;
        validate_base_request(&request, &base)?;
        let record = self
            .persist_start_request(&request, &base, condition)
            .await?;
        if record.state != CounterfactualJobState::Queued {
            return Ok(record);
        }
        if let Some(terminal) = self.preflight_start(&request, &base, &record).await? {
            return Ok(terminal);
        }
        let runtime = self.clone();
        let claim = tokio::spawn(async move {
            Box::pin(runtime.claim_and_spawn(record, base, monotonic_clock)).await
        });
        claim.await.map_err(counterfactual_join_error)?
    }

    async fn persist_start_request(
        &self,
        request: &SolutionStartCounterfactualRequestV1,
        base: &StoredAcceptedResultV2,
        condition: CounterfactualConditionV1,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        let portable = &base.portable;
        let semantics = CounterfactualRequestSemanticsV1 {
            schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            scenario_revision: request.expected_revision.value(),
            snapshot_id: portable.run_input.snapshot_id,
            snapshot_document_hash: portable.run_input.snapshot_document_hash.clone(),
            base: AcceptedResultRefV1 {
                solution_id: portable.accepted_result.solution.solution_id,
                result_checksum: portable.accepted_result.checksum.clone(),
            },
            base_run_id: portable.run_input.run_id,
            base_run_input_checksum: portable.run_input.checksum.clone(),
            base_model_hash: portable.run_input.model_hash.clone(),
            objective_policy_hash: portable.run_input.objective_policy_hash.clone(),
            condition_checksum: condition.checksum.clone(),
            total_budget_milliseconds: request.total_budget_milliseconds,
        };
        let job_id = CounterfactualJobId::new(self.ids.as_ref()).map_err(id_generation_error)?;
        let persisted_request = CounterfactualJobRequestV1::new(
            job_id,
            request.request_id,
            semantics,
            condition,
            self.clock.now(),
        )
        .map_err(|_| {
            invalid_request(
                "solution.counterfactual_invalid",
                "/request",
                "The counterfactual request is invalid.",
            )
        })?;
        let started = self
            .store
            .start_counterfactual_job(persisted_request)
            .await
            .map_err(counterfactual_store_error)?;
        if !started.reused {
            self.publish(
                &started.record.request,
                None,
                CounterfactualProgressPhase::Queued,
            );
        }
        Ok(started.record)
    }

    async fn preflight_start(
        &self,
        request: &SolutionStartCounterfactualRequestV1,
        base: &StoredAcceptedResultV2,
        record: &CounterfactualJobRecordV1,
    ) -> Result<Option<CounterfactualJobRecordV1>, AppError> {
        let current = self
            .store
            .get_project(request.scenario_id)
            .await
            .map_err(counterfactual_store_error)?;
        let failure = if current.summary.revision == request.expected_revision {
            let backend = self.solvers.get(&base.portable.run_input.backend_id);
            match backend {
                None => Some(CounterfactualFailureKind::BackendUnavailable),
                Some(backend)
                    if !base_runtime_matches(
                        &base.portable.run_input,
                        backend.runtime_identity(),
                    ) =>
                {
                    Some(CounterfactualFailureKind::InvalidBinding)
                }
                Some(_) => None,
            }
        } else {
            Some(CounterfactualFailureKind::StaleRevision)
        };
        let Some(failure) = failure else {
            return Ok(None);
        };
        self.fail_before_run(record.request.job_id, failure)
            .await
            .map(Some)
    }

    async fn claim_and_spawn(
        &self,
        record: CounterfactualJobRecordV1,
        base: StoredAcceptedResultV2,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        let live = LiveJob {
            cancellation: self.cancellation.child(),
            state: Arc::new(StdMutex::new(LiveJobState::default())),
        };
        if !self.reserve_live(record.request.job_id, &live).await {
            let authority = self
                .store
                .load_counterfactual_job(record.request.job_id)
                .await
                .map_err(counterfactual_store_error)?;
            return Ok(authority);
        }
        let Ok(budget) = ParentSolveBudget::new(
            record.request.semantics.total_budget_milliseconds,
            monotonic_clock,
            live.cancellation.clone(),
        ) else {
            self.live.lock().await.remove(&record.request.job_id);
            let failed = self
                .fail_before_run(
                    record.request.job_id,
                    CounterfactualFailureKind::InvalidBinding,
                )
                .await?;
            return Ok(failed);
        };
        let running = match self
            .store
            .transition_counterfactual_job(
                record.request.job_id,
                CounterfactualJobTransitionV1::Running {
                    started_at: self.clock.now(),
                },
            )
            .await
        {
            Ok(running) => running,
            Err(StoreError::CounterfactualTransitionConflict(_)) => {
                self.live.lock().await.remove(&record.request.job_id);
                let authority = self
                    .store
                    .load_counterfactual_job(record.request.job_id)
                    .await
                    .map_err(counterfactual_store_error)?;
                return Ok(authority);
            }
            Err(error) => {
                self.live.lock().await.remove(&record.request.job_id);
                return Err(counterfactual_store_error(error));
            }
        };
        self.publish(
            &running.request,
            None,
            CounterfactualProgressPhase::Compiling,
        );
        self.spawn_supervised(&running, base, budget, &live);
        Ok(running)
    }

    async fn reserve_live(&self, job_id: CounterfactualJobId, live: &LiveJob) -> bool {
        let mut jobs = self.live.lock().await;
        match jobs.entry(job_id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(live.clone());
                true
            }
        }
    }

    fn spawn_supervised(
        &self,
        running: &CounterfactualJobRecordV1,
        base: StoredAcceptedResultV2,
        budget: ParentSolveBudget,
        live: &LiveJob,
    ) {
        let worker_runtime = self.clone();
        let worker_live = live.clone();
        let worker_record = running.clone();
        let worker = tokio::spawn(async move {
            Box::pin(worker_runtime.execute_job(worker_record, base, budget, worker_live.clone()))
                .await
        });
        let supervisor_runtime = self.clone();
        let supervisor_live = live.clone();
        let supervised_job_id = running.request.job_id;
        let supervisor = tokio::spawn(async move {
            let joined = worker.await;
            if joined.is_err() || joined.is_ok_and(|result| result.is_err()) {
                supervisor_runtime
                    .ensure_terminal(supervised_job_id, &supervisor_live)
                    .await;
            }
            supervisor_runtime
                .live
                .lock()
                .await
                .remove(&supervised_job_id);
        });
        drop(supervisor);
    }

    pub(crate) async fn cancel(
        &self,
        request: SolutionCancelCounterfactualRequestV1,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        if request.schema_version != COUNTERFACTUAL_API_SCHEMA_VERSION {
            return Err(schema_error(request.schema_version));
        }
        let job = self
            .store
            .load_counterfactual_job(request.job_id)
            .await
            .map_err(counterfactual_store_error)?;
        if job.request.semantics.scenario_id != request.scenario_id
            || job.request.semantics.scenario_revision != request.expected_revision.value()
        {
            return Err(invalid_request(
                "solution.counterfactual_cancel_binding",
                "/request",
                "The cancellation request does not match the immutable counterfactual job binding.",
            ));
        }
        let outcome = self
            .store
            .request_counterfactual_cancel(
                request.job_id,
                request.cancel_request_id,
                self.clock.now(),
            )
            .await
            .map_err(counterfactual_store_error)?;
        let record = match outcome {
            CounterfactualCancelOutcomeV1::Requested { record, reused } => {
                if let Some(live) = self.live.lock().await.get(&request.job_id).cloned() {
                    live.cancellation.cancel();
                }
                if !reused && record.state == CounterfactualJobState::Cancelled {
                    self.publish(
                        &record.request,
                        None,
                        CounterfactualProgressPhase::Cancelled,
                    );
                }
                record
            }
            CounterfactualCancelOutcomeV1::AlreadyTerminal { record } => record,
        };
        Ok(record)
    }

    async fn fail_before_run(
        &self,
        job_id: CounterfactualJobId,
        kind: CounterfactualFailureKind,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        match self
            .store
            .transition_counterfactual_job(
                job_id,
                CounterfactualJobTransitionV1::Failed {
                    finished_at: self.clock.now(),
                    error: CounterfactualJobErrorV1 { kind },
                },
            )
            .await
        {
            Ok(record) => {
                self.publish(&record.request, None, CounterfactualProgressPhase::Failed);
                Ok(record)
            }
            Err(StoreError::CounterfactualTransitionConflict(_)) => self
                .store
                .load_counterfactual_job(job_id)
                .await
                .map_err(counterfactual_store_error),
            Err(error) => Err(counterfactual_store_error(error)),
        }
    }

    // The worker is intentionally one linear authority pipeline: each checkpoint orders durable
    // state, compilation, dispatch, review, and finalization under the same parent budget.
    #[allow(clippy::too_many_lines)]
    async fn execute_job(
        &self,
        record: CounterfactualJobRecordV1,
        base: StoredAcceptedResultV2,
        budget: ParentSolveBudget,
        live: LiveJob,
    ) -> Result<(), ()> {
        let request = &record.request;
        let total = request.semantics.total_budget_milliseconds;
        if self.stop_before_run(request.job_id, &budget).await? {
            return Ok(());
        }
        let compile_started = elapsed(&budget).map_err(|_| ())?;
        let Ok(pack) = self.packs.require(&base.portable.run_input.pack_id) else {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        };
        let compile_context = CompileContext {
            scenario_revision: request.semantics.scenario_revision,
            semantic_metadata: BTreeMap::new(),
            control: eutheto_types::OperationControl::Solve(budget.phase_view()),
            planning_limits: PlanningIrLimitsV1::DEFAULT,
        };
        let base_problem = match pack.compile(&base.document, &compile_context) {
            Ok(problem) => problem,
            Err(error) => {
                self.finish_compile_error(request.job_id, &error).await?;
                return Ok(());
            }
        };
        if self.stop_before_run(request.job_id, &budget).await? {
            return Ok(());
        }
        let Ok(base_hash) = canonical_ir_hash(&base_problem, PlanningIrLimitsV1::DEFAULT) else {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidModel)
                .await?;
            return Ok(());
        };
        if base_hash != request.semantics.base_model_hash
            || base_problem.metadata.compiler_version != base.portable.run_input.compiler_version
            || base_problem.metadata.pack_id != base.portable.run_input.pack_id
            || base_problem.metadata.scenario_id != request.semantics.scenario_id
            || base_problem.metadata.scenario_revision != request.semantics.scenario_revision
        {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        }
        let derived_problem = match pack.compile_counterfactual(
            &base.document,
            &request.condition,
            &CounterfactualCompileContext {
                base_problem: &base_problem,
                compile_context: &compile_context,
                budget: budget.phase_view(),
            },
        ) {
            Ok(problem) => problem,
            Err(error) => {
                self.finish_compile_error(request.job_id, &error).await?;
                return Ok(());
            }
        };
        if self.stop_before_run(request.job_id, &budget).await? {
            return Ok(());
        }
        let compilation = validate_counterfactual_problem(
            &base_problem,
            &derived_problem,
            &request.condition,
            &request.semantics.objective_policy_hash,
            PlanningIrLimitsV1::DEFAULT,
        );
        let Ok(compilation) = compilation else {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        };
        if self.stop_before_run(request.job_id, &budget).await? {
            return Ok(());
        }
        if derived_problem.metadata.compiler_version != base.portable.run_input.compiler_version {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        }
        let Some(backend) = self.solvers.get(&base.portable.run_input.backend_id) else {
            self.fail_running_before_run(
                request.job_id,
                CounterfactualFailureKind::BackendUnavailable,
            )
            .await?;
            return Ok(());
        };
        let compile_finished = elapsed(&budget).map_err(|_| ())?;
        let compile_elapsed = subtract_duration(compile_finished, compile_started)
            .and_then(observed_milliseconds)
            .map_err(|_| ())?;
        let identity = backend.runtime_identity().clone();
        if !base_runtime_matches(&base.portable.run_input, &identity) {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        }
        let mut options = base.portable.run_input.solve_options.clone();
        options.backend = BackendSelection::Specific(base.portable.run_input.backend_id.clone());
        options.time_limit_milliseconds = total;
        let Ok(run_id) = SolveRunId::new(self.ids.as_ref()) else {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        };
        let Ok(run_request_id) = RequestId::new(self.ids.as_ref()) else {
            self.fail_running_before_run(request.job_id, CounterfactualFailureKind::InvalidBinding)
                .await?;
            return Ok(());
        };
        if self.stop_before_run(request.job_id, &budget).await? {
            return Ok(());
        }
        let started_at = record.started_at.ok_or(())?;
        let started = match self
            .store
            .start_counterfactual_run(
                request.job_id,
                NewSolveRunV1 {
                    run_id,
                    request_id: run_request_id,
                    scenario_id: request.semantics.scenario_id,
                    expected_revision: Revision::new(request.semantics.scenario_revision),
                    planning_ir_schema_version: PLANNING_IR_SCHEMA_VERSION,
                    compiler_version: derived_problem.metadata.compiler_version.clone(),
                    application_version: env!("CARGO_PKG_VERSION").to_owned(),
                    backend_id: identity.backend_id().clone(),
                    backend_version: identity.backend_version().to_owned(),
                    adapter_version: identity.adapter_version().to_owned(),
                    worker_version: identity.worker_version().to_owned(),
                    solver_version: identity.solver_version().to_owned(),
                    protocol_major: identity.protocol_major(),
                    protocol_minor: identity.protocol_minor(),
                    model_hash: compilation.derived_model_hash.clone(),
                    objective_policy_hash: compilation.objective_policy_hash.clone(),
                    solve_options: options.clone(),
                    temporary_condition_hash: Some(compilation.condition_checksum.clone()),
                    started_at,
                },
            )
            .await
        {
            Ok(started) => started,
            Err(StoreError::Conflict { .. }) => {
                self.fail_running_before_run(
                    request.job_id,
                    CounterfactualFailureKind::StaleRevision,
                )
                .await?;
                return Ok(());
            }
            Err(_) => return Err(()),
        };
        lock_live_state(&live.state).derived = Some(DerivedExecution {
            input: started.input.clone(),
            started_at: started.started_at,
            budget: budget.clone(),
        });
        let loaded = self
            .store
            .load_solve_input(started.input.run_id)
            .await
            .map_err(|_| ())?;
        let persisted_job = self
            .store
            .load_counterfactual_job(request.job_id)
            .await
            .map_err(|_| ())?;
        if loaded.input != started.input
            || loaded.document != base.document
            || persisted_job.request != *request
            || persisted_job.state != CounterfactualJobState::Running
            || persisted_job.cancel_request_id.is_some()
        {
            return Err(());
        }
        self.publish(
            request,
            Some(loaded.input.run_id),
            CounterfactualProgressPhase::Solving,
        );

        let derived_problem = Arc::new(derived_problem);
        let verification_clock = ParentVerificationClock::new(&budget);
        let event_runtime = self.clone();
        let event_request = request.clone();
        let event_run_id = loaded.input.run_id;
        let mut verifying_emitted = false;
        let mut reviewer = RouterCandidateReviewer::new(
            pack,
            &loaded.document,
            request.semantics.scenario_revision,
            derived_problem.as_ref(),
            &verification_clock,
            self.ids.as_ref(),
            &compile_context.control,
        )
        .map_err(|_| ())?
        .with_before_review(move || {
            if !verifying_emitted {
                event_runtime.publish(
                    &event_request,
                    Some(event_run_id),
                    CounterfactualProgressPhase::Verifying,
                );
                verifying_emitted = true;
            }
        });
        let mut progress = IgnoreSolverProgress;
        let execution = SolverRouter::new(&self.solvers)
            .execute(
                Arc::clone(&derived_problem),
                options.clone(),
                &budget,
                DurationMillis::ZERO,
                &mut progress,
                &mut reviewer,
            )
            .await;
        let decision = reviewer.into_decision();
        if verification_clock.has_failed() {
            return Err(());
        }
        let finished_at = self.clock.now();
        let elapsed_total = elapsed(&budget).map_err(|_| ())?;
        subtract_duration(elapsed_total, compile_finished).map_err(|_| ())?;
        let observations = execution.observations.as_ref().ok_or(())?;
        subtract_duration(observations.started_after, compile_finished).map_err(|_| ())?;
        subtract_duration(elapsed_total, observations.finished_after).map_err(|_| ())?;
        let terminal = build_terminal(TerminalBuildContext {
            record: &execution.record,
            observations,
            decision: decision.as_ref(),
            identity: &identity,
            options: &options,
            compile_elapsed,
            elapsed_total: observed_milliseconds(elapsed_total).map_err(|_| ())?,
            started_at: started.started_at,
            finished_at,
            input: &started.input,
            condition: &request.condition,
        })
        .map_err(|_| ())?;
        self.publish(
            request,
            Some(started.input.run_id),
            CounterfactualProgressPhase::Finalizing,
        );
        self.finalize(FinalizeJobContext {
            request,
            base: &base,
            compilation: &compilation,
            input: &started.input,
            terminal,
            budget: &budget,
            previous_elapsed: elapsed_total,
        })
        .await
        .map(|_| ())
        .map_err(|_| ())
    }

    async fn finalize(
        &self,
        context: FinalizeJobContext<'_>,
    ) -> Result<CounterfactualJobRecordV1, StoreError> {
        let FinalizeJobContext {
            request,
            base,
            compilation,
            input,
            terminal,
            budget,
            previous_elapsed,
        } = context;
        let evidence_started = elapsed(budget)?;
        subtract_duration(evidence_started, previous_elapsed)?;
        let prepared_evidence = terminal.alternative.as_ref().map(accepted_evidence);
        let evidence_finished = elapsed(budget)?;
        let evidence_preparation_elapsed =
            observed_milliseconds(subtract_duration(evidence_finished, evidence_started)?)?;
        let terminal = terminal.retime(
            observed_milliseconds(evidence_finished)?,
            self.clock.now(),
            budget.is_expired(),
            evidence_preparation_elapsed,
        )?;
        let (finalization, manifest_started_at) = prepare_finalization(FinalizationContext {
            request,
            base,
            compilation,
            input,
            terminal,
            prepared_evidence,
        })?;
        match self
            .store
            .finalize_counterfactual_run(request.job_id, finalization)
            .await
        {
            Ok(record) => {
                self.publish_terminal(&record, Some(input.run_id));
                Ok(record)
            }
            Err(StoreError::CounterfactualTransitionConflict(_)) => {
                let current = self.store.load_counterfactual_job(request.job_id).await?;
                if current.state == CounterfactualJobState::Running
                    && current.cancel_request_id.is_some()
                {
                    let cancelled_elapsed = elapsed(budget)?;
                    subtract_duration(cancelled_elapsed, evidence_finished)?;
                    let manifest = cancelled_manifest(
                        input,
                        manifest_started_at,
                        self.clock.now(),
                        observed_milliseconds(cancelled_elapsed)?,
                    )?;
                    let record = self
                        .store
                        .finalize_counterfactual_run(
                            request.job_id,
                            CounterfactualRunFinalizationV1::Cancelled { manifest },
                        )
                        .await?;
                    self.publish_terminal(&record, Some(input.run_id));
                    Ok(record)
                } else if is_terminal(current.state) {
                    Ok(current)
                } else {
                    Err(StoreError::CounterfactualTransitionConflict(request.job_id))
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn stop_before_run(
        &self,
        job_id: CounterfactualJobId,
        budget: &ParentSolveBudget,
    ) -> Result<bool, ()> {
        if budget.is_cancelled() {
            self.finish_cancelled_before_run(job_id).await?;
            return Ok(true);
        }
        if budget.is_expired() {
            self.fail_running_before_run(job_id, CounterfactualFailureKind::BudgetExhausted)
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn finish_compile_error(
        &self,
        job_id: CounterfactualJobId,
        error: &DomainPackError,
    ) -> Result<(), ()> {
        match error {
            DomainPackError::Cancelled => self.finish_cancelled_before_run(job_id).await,
            DomainPackError::BudgetExpired => {
                self.fail_running_before_run(job_id, CounterfactualFailureKind::BudgetExhausted)
                    .await
            }
            DomainPackError::Contract(_) => {
                self.fail_running_before_run(job_id, CounterfactualFailureKind::InvalidModel)
                    .await
            }
            _ => {
                self.fail_running_before_run(job_id, CounterfactualFailureKind::CompilationFailed)
                    .await
            }
        }
    }

    async fn fail_running_before_run(
        &self,
        job_id: CounterfactualJobId,
        kind: CounterfactualFailureKind,
    ) -> Result<(), ()> {
        let record = self
            .store
            .transition_counterfactual_job(
                job_id,
                CounterfactualJobTransitionV1::Failed {
                    finished_at: self.clock.now(),
                    error: CounterfactualJobErrorV1 { kind },
                },
            )
            .await
            .map_err(|_| ())?;
        self.publish_terminal(&record, None);
        Ok(())
    }

    async fn finish_cancelled_before_run(&self, job_id: CounterfactualJobId) -> Result<(), ()> {
        let current = self
            .store
            .load_counterfactual_job(job_id)
            .await
            .map_err(|_| ())?;
        let transition = if current.cancel_request_id.is_some() {
            CounterfactualJobTransitionV1::Cancelled {
                finished_at: self.clock.now(),
            }
        } else {
            CounterfactualJobTransitionV1::Interrupted {
                finished_at: self.clock.now(),
            }
        };
        let record = self
            .store
            .transition_counterfactual_job(job_id, transition)
            .await
            .map_err(|_| ())?;
        self.publish_terminal(&record, None);
        Ok(())
    }

    async fn ensure_terminal(&self, job_id: CounterfactualJobId, live: &LiveJob) {
        let Ok(current) = self.store.load_counterfactual_job(job_id).await else {
            return;
        };
        if is_terminal(current.state) {
            return;
        }
        let derived = lock_live_state(&live.state).derived.clone();
        let result = if let Some(derived) = &derived {
            if current.cancel_request_id.is_some() {
                // An unmeasurable cancellation must remain recoverable, not acquire invented time.
                match elapsed(&derived.budget)
                    .and_then(observed_milliseconds)
                    .and_then(|elapsed| {
                        cancelled_manifest(
                            &derived.input,
                            derived.started_at,
                            self.clock.now(),
                            elapsed,
                        )
                    }) {
                    Ok(manifest) => {
                        self.store
                            .finalize_counterfactual_run(
                                job_id,
                                CounterfactualRunFinalizationV1::Cancelled { manifest },
                            )
                            .await
                    }
                    Err(error) => Err(error),
                }
            } else {
                match interrupted_manifest(&derived.input, derived.started_at, self.clock.now()) {
                    Ok(manifest) => {
                        self.store
                            .finalize_counterfactual_run(
                                job_id,
                                CounterfactualRunFinalizationV1::Interrupted { manifest },
                            )
                            .await
                    }
                    Err(error) => Err(error),
                }
            }
        } else {
            let transition = if current.cancel_request_id.is_some() {
                CounterfactualJobTransitionV1::Cancelled {
                    finished_at: self.clock.now(),
                }
            } else {
                CounterfactualJobTransitionV1::Interrupted {
                    finished_at: self.clock.now(),
                }
            };
            self.store
                .transition_counterfactual_job(job_id, transition)
                .await
        };
        if let Ok(record) = result {
            self.publish_terminal(&record, derived.map(|value| value.input.run_id));
        }
    }

    fn publish_terminal(&self, record: &CounterfactualJobRecordV1, run_id: Option<SolveRunId>) {
        let phase = match record.state {
            CounterfactualJobState::Completed => CounterfactualProgressPhase::Completed,
            CounterfactualJobState::Failed => CounterfactualProgressPhase::Failed,
            CounterfactualJobState::Cancelled => CounterfactualProgressPhase::Cancelled,
            CounterfactualJobState::Interrupted => CounterfactualProgressPhase::Interrupted,
            CounterfactualJobState::Queued | CounterfactualJobState::Running => return,
        };
        self.publish(&record.request, run_id, phase);
    }

    fn publish(
        &self,
        request: &CounterfactualJobRequestV1,
        run_id: Option<SolveRunId>,
        phase: CounterfactualProgressPhase,
    ) {
        let _ = self.events.send(AppEvent {
            topic: EventTopic::CounterfactualProgress,
            payload: EventPayload::CounterfactualProgress {
                context: EventContext {
                    event_version: 1,
                    timestamp: self.clock.now(),
                    request_id: Some(request.request_id),
                    scenario_id: Some(request.semantics.scenario_id),
                    revision: Some(Revision::new(request.semantics.scenario_revision)),
                    solve_run_id: run_id,
                },
                job_id: request.job_id,
                phase,
            },
        });
    }
}

struct FinalizationContext<'a> {
    request: &'a CounterfactualJobRequestV1,
    base: &'a StoredAcceptedResultV2,
    compilation: &'a CounterfactualCompilationBindingV1,
    input: &'a eutheto_domain_ir::RunInputV1,
    terminal: TerminalMaterial,
    prepared_evidence: Option<BTreeMap<eutheto_domain_ir::DomainEvidenceId, VerificationValue>>,
}

fn prepare_finalization(
    context: FinalizationContext<'_>,
) -> Result<(CounterfactualRunFinalizationV1, Rfc3339Timestamp), StoreError> {
    let FinalizationContext {
        request,
        base,
        compilation,
        input,
        terminal,
        prepared_evidence,
    } = context;
    let (manifest, alternative, intended) = terminal.into_parts();
    let interpretation = interpret_counterfactual(
        request,
        &base.portable.run_input,
        &base.portable.run_manifest,
        &base.portable.accepted_result,
        compilation,
        input,
        &manifest,
        alternative.as_ref(),
    );
    let manifest_started_at = manifest.started_at;
    let manifest_elapsed = manifest
        .elapsed_milliseconds
        .unwrap_or(DurationMillis::ZERO);
    let finalization = match (intended, interpretation) {
        (IntendedTerminal::Accepted, CounterfactualInterpretation::Completed(result)) => {
            let accepted =
                alternative.ok_or(StoreError::CounterfactualTransitionConflict(request.job_id))?;
            CounterfactualRunFinalizationV1::CompletedAccepted {
                evidence: prepared_evidence
                    .ok_or(StoreError::CounterfactualTransitionConflict(request.job_id))?,
                accepted_result: Box::new(accepted),
                manifest,
                result,
            }
        }
        (IntendedTerminal::NoResult, CounterfactualInterpretation::Completed(result)) => {
            CounterfactualRunFinalizationV1::CompletedNoResult { manifest, result }
        }
        (IntendedTerminal::Quarantined, CounterfactualInterpretation::Failed(error))
            if error.kind == CounterfactualFailureKind::InvalidCandidate =>
        {
            CounterfactualRunFinalizationV1::Quarantined {
                manifest,
                candidate_diagnostics: CandidateDiagnosticsV1::default(),
                error,
            }
        }
        (IntendedTerminal::Cancelled, _) => CounterfactualRunFinalizationV1::Cancelled { manifest },
        (IntendedTerminal::Failed(error), _) => {
            CounterfactualRunFinalizationV1::Failed { manifest, error }
        }
        (_, CounterfactualInterpretation::Failed(_)) => {
            let error = CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::InvalidBinding,
            };
            let replacement = failed_manifest(
                input,
                manifest.started_at,
                manifest.finished_at,
                manifest_elapsed,
                error.kind,
            )?;
            CounterfactualRunFinalizationV1::Failed {
                manifest: replacement,
                error,
            }
        }
        _ => return Err(StoreError::CounterfactualTransitionConflict(request.job_id)),
    };
    Ok((finalization, manifest_started_at))
}

struct IgnoreSolverProgress;
impl ProgressSink for IgnoreSolverProgress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}

enum IntendedTerminal {
    Accepted,
    NoResult,
    Quarantined,
    Cancelled,
    Failed(CounterfactualJobErrorV1),
}

struct TerminalMaterial {
    manifest: RunManifestV1,
    alternative: Option<AcceptedResult>,
    intended: IntendedTerminal,
}

impl TerminalMaterial {
    fn into_parts(self) -> (RunManifestV1, Option<AcceptedResult>, IntendedTerminal) {
        (self.manifest, self.alternative, self.intended)
    }

    fn retime(
        self,
        elapsed: DurationMillis,
        finished_at: Rfc3339Timestamp,
        expired: bool,
        evidence_preparation_elapsed: DurationMillis,
    ) -> Result<Self, StoreError> {
        let force_limit = expired
            && matches!(
                &self.intended,
                IntendedTerminal::Accepted | IntendedTerminal::NoResult
            );
        let outcome = if force_limit {
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::NoSolutionWithinLimit,
            }
        } else {
            self.manifest.outcome.clone()
        };
        let mut phase_timings = self.manifest.phase_timings;
        phase_timings.evidence_persistence_milliseconds = Some(evidence_preparation_elapsed);
        let manifest = RunManifestV1::new(
            self.manifest.run_id,
            self.manifest.run_input_checksum,
            outcome,
            self.manifest.started_at,
            finished_at,
            Some(elapsed),
            self.manifest.first_incumbent_milliseconds,
            if force_limit {
                None
            } else {
                self.manifest.first_verified_feasible_milliseconds
            },
            phase_timings,
            self.manifest.verification_warnings,
        )
        .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
        Ok(Self {
            manifest,
            alternative: if force_limit { None } else { self.alternative },
            intended: if force_limit {
                IntendedTerminal::NoResult
            } else {
                self.intended
            },
        })
    }
}

#[derive(Clone, Copy)]
struct TerminalBuildContext<'a> {
    record: &'a RouterExecutionRecord,
    decision: Option<&'a AcceptanceDecision>,
    identity: &'a BackendRuntimeIdentity,
    options: &'a eutheto_types::SolveOptions,
    observations: &'a RouterObservations,
    compile_elapsed: DurationMillis,
    elapsed_total: DurationMillis,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    input: &'a eutheto_domain_ir::RunInputV1,
    condition: &'a CounterfactualConditionV1,
}

fn build_terminal(context: TerminalBuildContext<'_>) -> Result<TerminalMaterial, StoreError> {
    let TerminalBuildContext {
        record,
        decision,
        identity,
        options,
        observations,
        compile_elapsed,
        elapsed_total,
        started_at,
        finished_at,
        input,
        condition,
    } = context;
    let attempt = record.attempts.last();
    let backend_elapsed = (record.invocation_count > 0)
        .then(|| observed_milliseconds(observations.backend_elapsed))
        .transpose()?;
    let first_incumbent = observations
        .first_candidate_after
        .map(|offset| {
            observations
                .started_after
                .checked_add(offset)
                .ok_or_else(|| StoreError::InvalidPersistedRun("parent clock overflow".to_owned()))
                .and_then(observed_milliseconds)
        })
        .transpose()?;
    let first_verified = observations
        .first_verified_feasible_after
        .map(|offset| {
            observations
                .started_after
                .checked_add(offset)
                .ok_or_else(|| StoreError::InvalidPersistedRun("parent clock overflow".to_owned()))
                .and_then(observed_milliseconds)
        })
        .transpose()?;
    let phase_timings = terminal_phase_timings(decision, compile_elapsed, backend_elapsed);
    let (outcome, alternative, intended, verified_timing) = select_terminal(
        record,
        decision,
        identity,
        options,
        condition,
        attempt.is_some_and(|attempt| attempt.outcome.is_some()),
        first_verified,
    );
    let manifest = RunManifestV1::new(
        input.run_id,
        input.checksum.clone(),
        outcome,
        started_at,
        finished_at,
        Some(elapsed_total),
        first_incumbent,
        verified_timing,
        phase_timings,
        Vec::new(),
    )
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
    Ok(TerminalMaterial {
        manifest,
        alternative,
        intended,
    })
}

type TerminalSelection = (
    RunTerminalOutcomeV1,
    Option<AcceptedResult>,
    IntendedTerminal,
    Option<DurationMillis>,
);

fn select_terminal(
    record: &RouterExecutionRecord,
    decision: Option<&AcceptanceDecision>,
    identity: &BackendRuntimeIdentity,
    options: &eutheto_types::SolveOptions,
    condition: &CounterfactualConditionV1,
    attempt_has_outcome: bool,
    first_verified: Option<DurationMillis>,
) -> TerminalSelection {
    let requires_runtime_evidence = matches!(
        record.terminal_reason,
        ExecutionTerminalReason::CandidateVerified
            | ExecutionTerminalReason::VerificationQuarantined
            | ExecutionTerminalReason::CandidateAwaitingVerification
    ) || (record.terminal_reason
        == ExecutionTerminalReason::BackendTerminated
        && attempt_has_outcome);
    if requires_runtime_evidence && !runtime_evidence_matches(record, identity, options) {
        return (
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::BackendFailed,
            },
            None,
            IntendedTerminal::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::InvalidBinding,
            }),
            None,
        );
    }
    match decision {
        Some(AcceptanceDecision::Accepted { result, .. })
            if record.terminal_reason == ExecutionTerminalReason::CandidateVerified =>
        {
            if !counterfactual_condition_satisfied(condition, &result.solution) {
                return (
                    RunTerminalOutcomeV1::VerificationAlarm {
                        diagnostic_code: COUNTERFACTUAL_CONDITION_MISMATCH_DIAGNOSTIC.to_owned(),
                    },
                    None,
                    IntendedTerminal::Quarantined,
                    None,
                );
            }
            let status = if record.terminal_status == SolveStatus::Optimal {
                SolveStatus::Optimal
            } else {
                SolveStatus::Feasible
            };
            (
                RunTerminalOutcomeV1::Accepted {
                    status,
                    solution_id: result.solution.solution_id,
                    accepted_result_checksum: result.checksum.clone(),
                    verification_checksum: result.verification.checksum.clone(),
                },
                Some((**result).clone()),
                IntendedTerminal::Accepted,
                first_verified,
            )
        }
        Some(AcceptanceDecision::Quarantined { alarm, .. })
            if record.terminal_reason == ExecutionTerminalReason::VerificationQuarantined =>
        {
            (
                RunTerminalOutcomeV1::VerificationAlarm {
                    diagnostic_code: alarm.diagnostic_code.clone(),
                },
                None,
                IntendedTerminal::Quarantined,
                None,
            )
        }
        _ => status_terminal(record.terminal_status),
    }
}

fn status_terminal(
    status: SolveStatus,
) -> (
    RunTerminalOutcomeV1,
    Option<AcceptedResult>,
    IntendedTerminal,
    Option<DurationMillis>,
) {
    match status {
        SolveStatus::Infeasible | SolveStatus::NoSolutionWithinLimit => (
            RunTerminalOutcomeV1::NoResult { status },
            None,
            IntendedTerminal::NoResult,
            None,
        ),
        SolveStatus::Cancelled => (
            RunTerminalOutcomeV1::NoResult { status },
            None,
            IntendedTerminal::Cancelled,
            None,
        ),
        SolveStatus::BackendUnavailable => (
            RunTerminalOutcomeV1::NoResult { status },
            None,
            IntendedTerminal::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::BackendUnavailable,
            }),
            None,
        ),
        SolveStatus::BackendFailed => (
            RunTerminalOutcomeV1::NoResult { status },
            None,
            IntendedTerminal::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::BackendFailed,
            }),
            None,
        ),
        SolveStatus::InvalidModel | SolveStatus::Unbounded => (
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::InvalidModel,
            },
            None,
            IntendedTerminal::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::InvalidModel,
            }),
            None,
        ),
        SolveStatus::Optimal | SolveStatus::Feasible => (
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::BackendFailed,
            },
            None,
            IntendedTerminal::Failed(CounterfactualJobErrorV1 {
                kind: CounterfactualFailureKind::InvalidBinding,
            }),
            None,
        ),
    }
}

fn subtract_duration(later: Duration, earlier: Duration) -> Result<Duration, StoreError> {
    later
        .checked_sub(earlier)
        .ok_or_else(|| StoreError::InvalidPersistedRun("parent clock moved backwards".to_owned()))
}

fn base_runtime_matches(
    input: &eutheto_domain_ir::RunInputV1,
    identity: &BackendRuntimeIdentity,
) -> bool {
    input.backend_id == *identity.backend_id()
        && input.backend_version == identity.backend_version()
        && input.adapter_version == identity.adapter_version()
        && input.worker_version == identity.worker_version()
        && input.solver_version == identity.solver_version()
        && input.protocol_major == identity.protocol_major()
        && input.protocol_minor == identity.protocol_minor()
}

fn observed_milliseconds(duration: Duration) -> Result<DurationMillis, StoreError> {
    u64::try_from(duration.as_millis())
        .ok()
        .and_then(|milliseconds| DurationMillis::new(milliseconds).ok())
        .ok_or_else(|| StoreError::InvalidPersistedRun("parent clock overflow".to_owned()))
}

fn elapsed(budget: &ParentSolveBudget) -> Result<Duration, StoreError> {
    let duration = budget.checked_elapsed().ok_or_else(|| {
        StoreError::InvalidPersistedRun("parent clock moved backwards".to_owned())
    })?;
    if duration.as_millis() > u128::from(DurationMillis::MAX.value()) {
        return Err(StoreError::InvalidPersistedRun(
            "parent clock overflow".to_owned(),
        ));
    }
    Ok(duration)
}

fn cancelled_manifest(
    input: &eutheto_domain_ir::RunInputV1,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    elapsed: DurationMillis,
) -> Result<RunManifestV1, StoreError> {
    RunManifestV1::new(
        input.run_id,
        input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Cancelled,
        },
        started_at,
        finished_at,
        Some(elapsed),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
}

fn interrupted_manifest(
    input: &eutheto_domain_ir::RunInputV1,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
) -> Result<RunManifestV1, StoreError> {
    RunManifestV1::new(
        input.run_id,
        input.checksum.clone(),
        RunTerminalOutcomeV1::Interrupted,
        started_at,
        finished_at,
        None,
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
}

fn failed_manifest(
    input: &eutheto_domain_ir::RunInputV1,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    elapsed: DurationMillis,
    kind: CounterfactualFailureKind,
) -> Result<RunManifestV1, StoreError> {
    let status = match kind {
        CounterfactualFailureKind::BackendUnavailable => SolveStatus::BackendUnavailable,
        CounterfactualFailureKind::InvalidModel => SolveStatus::InvalidModel,
        _ => SolveStatus::BackendFailed,
    };
    RunManifestV1::new(
        input.run_id,
        input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult { status },
        started_at,
        finished_at,
        Some(elapsed),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
}

fn validate_start_request(
    request: &SolutionStartCounterfactualRequestV1,
) -> Result<CounterfactualConditionV1, AppError> {
    if request.schema_version != COUNTERFACTUAL_API_SCHEMA_VERSION {
        return Err(schema_error(request.schema_version));
    }
    let condition = CounterfactualConditionV1::new(request.condition.clone()).map_err(|_| {
        invalid_request(
            "solution.counterfactual_condition_invalid",
            "/condition",
            "The counterfactual condition is invalid.",
        )
    })?;
    if request.total_budget_milliseconds == DurationMillis::ZERO
        || request.total_budget_milliseconds.value()
            > COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1
    {
        return Err(invalid_request(
            "solution.counterfactual_budget_invalid",
            "/totalBudgetMilliseconds",
            "The total counterfactual budget must be between 1 and 30000 milliseconds.",
        ));
    }
    Ok(condition)
}

fn validate_base_request(
    request: &SolutionStartCounterfactualRequestV1,
    base: &StoredAcceptedResultV2,
) -> Result<(), AppError> {
    let input = &base.portable.run_input;
    let accepted = &base.portable.accepted_result;
    if input.scenario_id != request.scenario_id
        || accepted.solution.scenario_id != request.scenario_id
    {
        return Err(invalid_request(
            "solution.counterfactual_base_scenario",
            "/baseSolutionId",
            "The base solution does not belong to the requested scenario.",
        ));
    }
    if input.scenario_revision != request.expected_revision.value()
        || accepted.solution.scenario_revision != request.expected_revision.value()
    {
        return Err(invalid_request(
            "solution.counterfactual_base_revision",
            "/expectedRevision",
            "The expected revision must equal the immutable base-solution revision.",
        ));
    }
    Ok(())
}

fn is_terminal(state: CounterfactualJobState) -> bool {
    matches!(
        state,
        CounterfactualJobState::Completed
            | CounterfactualJobState::Failed
            | CounterfactualJobState::Cancelled
            | CounterfactualJobState::Interrupted
    )
}

fn lock_live_state(state: &StdMutex<LiveJobState>) -> std::sync::MutexGuard<'_, LiveJobState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn schema_error(version: u32) -> AppError {
    invalid_request(
        "solution.counterfactual_schema_unsupported",
        "/schemaVersion",
        &format!("Unsupported counterfactual schema version {version}."),
    )
}

fn invalid_request(code: &str, path: &str, message: &str) -> AppError {
    AppError::Validation(ValidationReport {
        issues: vec![ValidationIssue {
            code: code.to_owned(),
            severity: ValidationSeverity::Error,
            message: message.to_owned(),
            field_path: Some(path.to_owned()),
            resource: None,
        }],
    })
}

fn id_generation_error(_: eutheto_types::IdGenerationError) -> AppError {
    AppError::Protocol(ProtocolFailure {
        code: "solution.counterfactual_identity_unavailable".to_owned(),
        message: "A counterfactual identity could not be allocated safely.".to_owned(),
        retryable: true,
        diagnostic_id: None,
    })
}

fn counterfactual_join_error(_: tokio::task::JoinError) -> AppError {
    AppError::Protocol(ProtocolFailure {
        code: "solution.counterfactual_claim_failed".to_owned(),
        message: "The counterfactual claim task failed before reporting durable authority."
            .to_owned(),
        retryable: true,
        diagnostic_id: None,
    })
}

#[allow(clippy::needless_pass_by_value)]
fn counterfactual_store_error(error: StoreError) -> AppError {
    match error {
        StoreError::ScenarioNotFound(id) => AppError::NotFound(ResourceRef::Scenario(id)),
        StoreError::AcceptedResultNotFound(id) => AppError::NotFound(ResourceRef::Solution(id)),
        StoreError::CounterfactualJobNotFound(_) => invalid_request(
            "solution.counterfactual_not_found",
            "/jobId",
            "The counterfactual job was not found.",
        ),
        StoreError::Conflict { expected, actual } => AppError::Conflict {
            expected_revision: expected,
            actual_revision: actual,
        },
        StoreError::CounterfactualRequestIdConflict { .. } => invalid_request(
            "solution.counterfactual_request_conflict",
            "/requestId",
            "The request identity is already bound to different counterfactual semantics.",
        ),
        StoreError::CounterfactualCancelRequestIdConflict { .. } => invalid_request(
            "solution.counterfactual_cancel_request_conflict",
            "/cancelRequestId",
            "The cancellation request identity conflicts with durable state.",
        ),
        StoreError::CounterfactualCapacityExceeded { .. } => AppError::Solver(SolverFailure {
            code: "solution.counterfactual_capacity".to_owned(),
            message: "Counterfactual diagnostic capacity is currently full.".to_owned(),
            retryable: true,
            diagnostic_id: None,
        }),
        StoreError::CounterfactualJobCollision(_)
        | StoreError::CounterfactualTransitionConflict(_) => invalid_request(
            "solution.counterfactual_state_conflict",
            "/jobId",
            "The counterfactual job changed concurrently.",
        ),
        _ => AppError::Storage(StorageFailure {
            code: "storage.counterfactual_operation_failed".to_owned(),
            message: "The local counterfactual operation failed safely.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_ir::{
        AssignmentValue, CounterfactualConditionPayloadV1, DomainAssignmentId,
    };
    use eutheto_verify::AcceptancePhaseTimings;
    use eutheto_verify::VerificationClock;
    use serde_json::Value;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct ReturningClock(AtomicU64);

    impl MonotonicClock for ReturningClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Relaxed))
        }
    }

    #[test]
    fn phase_spans_reject_backwards_time_before_quantizing_the_completed_span()
    -> Result<(), Box<dyn std::error::Error>> {
        let started = Duration::from_nanos(999_999);
        let finished = Duration::from_nanos(1_000_001);
        let span = subtract_duration(finished, started)?;
        assert_eq!(span, Duration::from_nanos(2));
        assert_eq!(observed_milliseconds(span)?, DurationMillis::ZERO);
        // Both samples round to zero milliseconds; the backwards span must still fail.
        assert!(matches!(
            subtract_duration(Duration::from_nanos(1), started),
            Err(StoreError::InvalidPersistedRun(_))
        ));
        assert!(matches!(
            observed_milliseconds(Duration::from_secs(u64::MAX)),
            Err(StoreError::InvalidPersistedRun(_))
        ));
        Ok(())
    }

    #[test]
    fn invalid_parent_samples_fail_and_verification_failure_survives_clock_recovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(ReturningClock(AtomicU64::new(100)));
        let budget = ParentSolveBudget::new(
            DurationMillis::new(1_000)?,
            clock.clone(),
            CancellationToken::default(),
        )?;
        let verification_clock = ParentVerificationClock::new(&budget);
        clock.0.store(99, Ordering::Relaxed);
        assert!(matches!(
            elapsed(&budget),
            Err(StoreError::InvalidPersistedRun(_))
        ));
        assert_eq!(verification_clock.now_milliseconds(), DurationMillis::MAX);
        assert!(verification_clock.has_failed());

        clock.0.store(110, Ordering::Relaxed);
        assert_eq!(elapsed(&budget)?, Duration::from_millis(10));
        assert_eq!(
            verification_clock.now_milliseconds(),
            DurationMillis::new(10)?
        );
        assert!(verification_clock.has_failed());

        clock.0.store(u64::MAX, Ordering::Relaxed);
        assert!(matches!(
            elapsed(&budget),
            Err(StoreError::InvalidPersistedRun(_))
        ));
        Ok(())
    }

    fn start_request() -> Result<SolutionStartCounterfactualRequestV1, Box<dyn std::error::Error>> {
        Ok(SolutionStartCounterfactualRequestV1 {
            schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
            request_id: "018f47f2-e880-7000-8000-000000000001".parse()?,
            scenario_id: "018f47f2-e880-7000-8000-000000000002".parse()?,
            expected_revision: Revision::new(1),
            base_solution_id: "018f47f2-e880-7000-8000-000000000003".parse()?,
            condition: CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.assignment")?,
                value: AssignmentValue::Boolean(true),
            },
            total_budget_milliseconds: DurationMillis::new(1_000)?,
        })
    }

    #[test]
    fn public_counterfactual_requests_are_camel_case_strict_and_versioned()
    -> Result<(), Box<dyn std::error::Error>> {
        let start = start_request()?;
        let encoded = serde_json::to_value(&start)?;
        assert!(encoded.get("schemaVersion").is_some());
        assert!(encoded.get("requestId").is_some());
        assert!(encoded.get("baseSolutionId").is_some());
        assert!(encoded.get("totalBudgetMilliseconds").is_some());
        assert!(encoded.pointer("/condition/type").is_some());
        assert!(encoded.pointer("/condition/schemaVersion").is_none());
        assert!(encoded.pointer("/condition/checksum").is_none());
        assert!(encoded.get("schema_version").is_none());
        assert_eq!(
            serde_json::from_value::<SolutionStartCounterfactualRequestV1>(encoded.clone())?,
            start
        );
        let mut unknown = encoded;
        unknown
            .as_object_mut()
            .ok_or("start request was not an object")?
            .insert("unknown".to_owned(), Value::Bool(true));
        assert!(serde_json::from_value::<SolutionStartCounterfactualRequestV1>(unknown).is_err());

        let cancel = SolutionCancelCounterfactualRequestV1 {
            schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
            cancel_request_id: "018f47f2-e880-7000-8000-000000000004".parse()?,
            scenario_id: start.scenario_id,
            expected_revision: start.expected_revision,
            job_id: "018f47f2-e880-7000-8000-000000000005".parse()?,
        };
        let encoded = serde_json::to_value(&cancel)?;
        assert!(encoded.get("cancelRequestId").is_some());
        assert!(encoded.get("jobId").is_some());
        assert_eq!(
            serde_json::from_value::<SolutionCancelCounterfactualRequestV1>(encoded.clone())?,
            cancel
        );
        let mut unknown = encoded;
        unknown
            .as_object_mut()
            .ok_or("cancel request was not an object")?
            .insert("unknown".to_owned(), Value::Bool(true));
        assert!(serde_json::from_value::<SolutionCancelCounterfactualRequestV1>(unknown).is_err());
        Ok(())
    }

    #[test]
    fn start_request_rejects_wrong_schema_and_out_of_range_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut request = start_request()?;
        request.schema_version = COUNTERFACTUAL_API_SCHEMA_VERSION + 1;
        assert!(matches!(
            validate_start_request(&request),
            Err(AppError::Validation(_))
        ));
        request.schema_version = COUNTERFACTUAL_API_SCHEMA_VERSION;
        request.total_budget_milliseconds = DurationMillis::ZERO;
        assert!(matches!(
            validate_start_request(&request),
            Err(AppError::Validation(_))
        ));
        request.total_budget_milliseconds =
            DurationMillis::new(COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1 + 1)?;
        let Err(AppError::Validation(report)) = validate_start_request(&request) else {
            return Err("oversized budget returned the wrong error category".into());
        };
        assert_eq!(
            report.issues[0].code,
            "solution.counterfactual_budget_invalid"
        );
        Ok(())
    }

    #[test]
    fn router_interruption_cannot_resurrect_a_stale_reviewer_quarantine()
    -> Result<(), Box<dyn std::error::Error>> {
        use eutheto_solver_router::{
            DecisionStatus, OverrideHandling, RoutingDecision, SplitDisposition,
        };
        use eutheto_types::*;
        let options = SolveOptions {
            backend: BackendSelection::Auto,
            mode: SolveMode::Balanced,
            time_limit_milliseconds: DurationMillis::new(1000)?,
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
        };
        let identity = BackendRuntimeIdentity::new(
            "tests.backend".parse()?,
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            "1.0.0".to_owned(),
            1,
            0,
        )?;
        let condition = CounterfactualConditionV1::new(start_request()?.condition)?;
        let decision = AcceptanceDecision::Quarantined {
            alarm: eutheto_verify::CorrectnessAlarm {
                category: eutheto_verify::CorrectnessAlarmCategory::RequiredRuleRejected,
                diagnostic_code: "verification.required_rule_rejected".to_owned(),
            },
            timings: AcceptancePhaseTimings::default(),
        };
        for (status, reason) in [
            (SolveStatus::Cancelled, ExecutionTerminalReason::Cancelled),
            (
                SolveStatus::NoSolutionWithinLimit,
                ExecutionTerminalReason::ParentDeadlineExceeded,
            ),
        ] {
            let record = RouterExecutionRecord {
                decision: RoutingDecision {
                    policy_version: 1,
                    profile: None,
                    profile_contract_version: 1,
                    requested_backend: BackendSelection::Auto,
                    status: DecisionStatus::Ready,
                    summary: None,
                    component_analysis: eutheto_planning_ir::ComponentAnalysis {
                        components: Vec::new(),
                        edge_count: 0,
                        component_hash: "0".repeat(64),
                    },
                    split: SplitDisposition::SingleComponent,
                    considered_backends: Vec::new(),
                    chosen_backend: Some(identity.backend_id().clone()),
                    override_handling: OverrideHandling::Automatic,
                    fallback_order: Vec::new(),
                    diagnostics: Vec::new(),
                },
                solve_options: options.clone(),
                attempts: Vec::new(),
                invocation_count: 1,
                terminal_status: status,
                terminal_reason: reason,
                selected_candidate: None,
                first_verified_feasible_milliseconds: None,
                diagnostics: Vec::new(),
            };
            let (outcome, alternative, _, first_verified) = select_terminal(
                &record,
                Some(&decision),
                &identity,
                &options,
                &condition,
                false,
                None,
            );
            assert_eq!(outcome, RunTerminalOutcomeV1::NoResult { status });
            assert!(alternative.is_none());
            assert!(first_verified.is_none());
        }
        Ok(())
    }
}
