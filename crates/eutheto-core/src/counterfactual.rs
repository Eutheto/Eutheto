use crate::infeasibility::{InertInfeasibilityEvidence, capture_original_infeasibility};
use crate::infeasibility_diagnostic::{
    InfeasibilityDiagnosticContext, InfeasibilityDiagnosticMaterial, diagnose_infeasibility,
};
use crate::verification::{
    ParentVerificationClock, accepted_evidence, runtime_evidence_matches, terminal_phase_timings,
};
use crate::{AppEvent, RouterCandidateReviewer};
use eutheto_domain_api::{
    CompileContext, CounterfactualCompileContext, DomainPackError, DomainPackRegistry,
};
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, AssignmentValue,
    COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
    COUNTERFACTUAL_TOTAL_BUDGET_MAX_MILLISECONDS_V1, CounterfactualCompilationBindingV1,
    CounterfactualConditionPayloadV1, CounterfactualConditionV1, CounterfactualFailureKind,
    CounterfactualJobErrorV1, CounterfactualJobRecordV1, CounterfactualJobRequestV1,
    CounterfactualJobState, CounterfactualRequestSemanticsV1, CounterfactualTerminalAuthorityV1,
    RunManifest, RunPhaseTimingsV1, RunTerminalOutcomeV1, VerificationValue,
    counterfactual_condition_satisfied,
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
    CandidateDiagnosticsV1, CounterfactualCancelOutcomeV1, CounterfactualCancellationObservationV1,
    CounterfactualJobListCursorV1, CounterfactualJobTransitionV1, CounterfactualRunFinalizationV1,
    MAX_ACTIVE_COUNTERFACTUAL_JOBS, MAX_COUNTERFACTUAL_JOB_LIST_LIMIT, NewSolveRunV1,
    SqliteScenarioStore, StoreError, StoredAcceptedResultV2,
};
use eutheto_types::{
    AppError, BackendSelection, CancellationToken, Clock, CounterfactualJobId,
    CounterfactualProgressPhase, DurationMillis, EventContext, EventPayload, EventTopic,
    IdGenerator, KnownVerificationAlarmV1, KnownVerificationSecondaryCauseCode, MonotonicClock,
    ParentSolveBudget, ProtocolFailure, RequestId, ResourceRef, Revision, Rfc3339Timestamp,
    ScenarioId, SolutionId, SolveRunId, SolveStatus, SolverFailure, StorageFailure,
    ValidationIssue, ValidationReport, ValidationSeverity, VerificationAlarmTerminalization,
    VerificationFailure,
};
use eutheto_verify::{AcceptanceDecision, CorrectnessAlarm};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
pub(crate) struct CounterfactualPublicationHandle {
    state: Arc<StdMutex<LiveJobState>>,
    scenario_id: ScenarioId,
    source_revision: Revision,
    run_id: SolveRunId,
    input_checksum: String,
}

impl CounterfactualPublicationHandle {
    pub(crate) fn recipient_retained_bytes(
        &self,
        control: &eutheto_types::OperationControl,
    ) -> Result<usize, AppError> {
        let mut bytes = crate::application::recipient_memory::SourceBytes::new(control)?;
        let state = lock_live_state(&self.state);
        let LiveJobState {
            owner,
            derived,
            alarm,
            settlement: _,
            claim: _,
            startup_budget,
            trusted_elapsed: _,
            trusted_wall: _,
            secondary_cause: _,
        } = &*state;
        bytes.add(size_of::<Self>())?;
        bytes.strings([&self.input_checksum])?;
        bytes.arc::<StdMutex<LiveJobState>>()?;
        let owner = owner
            .as_ref()
            .ok_or_else(crate::application::solution_contract_error)?;
        // Same-logical owner replacement clones only this closed condition/reference record.
        // String::clone allocates its length, not the incoming spare capacity. Charging the
        // current owner twice covers the clone even while it overlaps the replaced owner.
        recipient_owner_bytes(&mut bytes, owner)?;
        recipient_owner_bytes(&mut bytes, owner)?;
        if startup_budget.is_some() {
            recipient_budget_bytes(&mut bytes)?;
        }
        if let Some(derived) = derived {
            let DerivedExecution {
                input,
                started_at: _,
                budget: _,
                native_observations,
            } = derived;
            bytes.input(input)?;
            recipient_budget_bytes(&mut bytes)?;
            if let Some(observations) = native_observations {
                bytes.observations(observations)?;
            } else {
                bytes.pending_observations()?;
            }
        }
        if let Some(alarm) = alarm {
            let OwnedAlarm {
                run_id: _,
                input_checksum,
                diagnostic_code,
                public_context,
            } = alarm;
            bytes.strings([input_checksum, diagnostic_code])?;
            if let Some(context) = public_context {
                // Private checked alarm context owns just this canonical checksum String.
                bytes.add(context.input_checksum().len())?;
            }
        } else {
            // capture_alarm stores a cloned canonical checksum, a to_owned bounded code and
            // one checked public context checksum. All other alarm/state fields are inline.
            bytes.add(64 + eutheto_domain_ir::MAX_RUN_DIAGNOSTIC_CODE_BYTES + 64)?;
        }
        Ok(bytes.finish())
    }

    pub(crate) fn admit(&self) -> Result<(), AppError> {
        let state = lock_live_state(&self.state);
        let Some(owner) = state.owner.as_ref() else {
            return Ok(());
        };
        if owner.semantics.scenario_id != self.scenario_id
            || owner.semantics.scenario_revision != self.source_revision.value()
        {
            return Ok(());
        }
        let Some(alarm) = state.alarm.as_ref().filter(|alarm| {
            alarm.run_id == self.run_id && alarm.input_checksum == self.input_checksum
        }) else {
            return Ok(());
        };
        Err(known_alarm_error(
            alarm,
            if state.settlement == AlarmSettlement::SettledCompatible {
                VerificationAlarmTerminalization::Confirmed
            } else {
                VerificationAlarmTerminalization::Unconfirmed
            },
            state.secondary_cause,
        ))
    }

    pub(crate) fn matches_input(&self, candidate: &eutheto_domain_ir::RunInput) -> bool {
        let state = lock_live_state(&self.state);
        let Some(owner) = state.owner.as_ref() else {
            return false;
        };
        // The store has admitted the complete candidate. Keep its native input binding
        // after settlement releases the heavier derived execution.
        owner.semantics.scenario_id == self.scenario_id
            && owner.semantics.scenario_revision == self.source_revision.value()
            && candidate.scenario_id == self.scenario_id
            && candidate.scenario_revision == self.source_revision.value()
            && candidate.run_id == self.run_id
            && candidate.checksum == self.input_checksum
            && state
                .derived
                .as_ref()
                .is_none_or(|derived| &derived.input == candidate)
    }
}

#[derive(Clone)]
struct LiveJob {
    cancellation: Option<CancellationToken>,
    startup: Arc<Mutex<()>>,
    state: Arc<StdMutex<LiveJobState>>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum AlarmSettlement {
    #[default]
    Active,
    SettledUnresolved,
    SettledCompatible,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum LiveClaimState {
    #[default]
    Available,
    Starting,
    Spawned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClaimDisposition {
    New,
    Recover,
    AlreadySpawned,
}

#[derive(Default)]
struct LiveJobState {
    owner: Option<CounterfactualJobRequestV1>,
    derived: Option<DerivedExecution>,
    /// The first locally established non-clock alarm wins and is never inferred from storage.
    alarm: Option<OwnedAlarm>,
    settlement: AlarmSettlement,
    claim: LiveClaimState,
    startup_budget: Option<ParentSolveBudget>,
    trusted_elapsed: Option<Duration>,
    trusted_wall: Option<Rfc3339Timestamp>,
    secondary_cause: Option<KnownVerificationSecondaryCauseCode>,
}

#[derive(Clone)]
struct OwnedAlarm {
    run_id: SolveRunId,
    input_checksum: String,
    diagnostic_code: String,
    public_context: Option<KnownVerificationAlarmV1>,
}

#[derive(Clone)]
struct DerivedExecution {
    input: eutheto_domain_ir::RunInput,
    started_at: Rfc3339Timestamp,
    budget: ParentSolveBudget,
    native_observations: Option<Arc<eutheto_domain_ir::RunNativeObservations>>,
}

fn recipient_budget_bytes(
    bytes: &mut crate::application::recipient_memory::SourceBytes<'_>,
) -> Result<(), AppError> {
    // Native-only: app/root token and injected monotonic clock stay independently app-owned.
    // budget.rs SolveBudgetState owns two Durations, Arc<dyn MonotonicClock> and a token;
    // provisional_live creates exactly one source child CancellationNode (AtomicBool + parent
    // Arc). Bound both private layouts by rounding every field up to a pointer-aligned slot,
    // including two-word Arc headers. No arbitrary injected clock/ancestor-heap claim is made.
    let word = size_of::<usize>();
    let duration = size_of::<Duration>();
    let aligned_duration = duration
        .checked_add(word - 1)
        .map(|size| size / word * word)
        .ok_or_else(crate::application::resource_limit_error)?;
    let state = aligned_duration
        .checked_mul(2)
        .and_then(|size| size.checked_add(5 * word))
        .ok_or_else(crate::application::resource_limit_error)?;
    bytes.add(state)?;
    bytes.add(4 * word)
}

fn recipient_owner_bytes(
    bytes: &mut crate::application::recipient_memory::SourceBytes<'_>,
    owner: &CounterfactualJobRequestV1,
) -> Result<(), AppError> {
    let CounterfactualJobRequestV1 {
        schema_version: _,
        job_id: _,
        request_id: _,
        semantics,
        condition,
        request_hash,
        created_at: _,
    } = owner;
    let CounterfactualRequestSemanticsV1 {
        schema_version: _,
        scenario_id: _,
        scenario_revision: _,
        snapshot_id: _,
        snapshot_document_hash,
        base,
        base_run_id: _,
        base_run_input_checksum,
        base_model_hash,
        objective_policy_hash,
        condition_checksum,
        total_budget_milliseconds: _,
    } = semantics;
    bytes.strings([
        snapshot_document_hash,
        base_run_input_checksum,
        base_model_hash,
        objective_policy_hash,
        condition_checksum,
        request_hash,
    ])?;
    bytes.accepted(base)?;
    let CounterfactualConditionV1 {
        schema_version: _,
        condition,
        checksum,
    } = condition;
    bytes.strings([checksum])?;
    match condition {
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value,
        }
        | CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value,
        } => {
            // AssignmentValue is closed and entirely inline today.
            match value {
                AssignmentValue::Boolean(_)
                | AssignmentValue::Integer(_)
                | AssignmentValue::Interval(_)
                | AssignmentValue::Absent => {}
            }
            bytes.add(assignment_id.heap_capacity())
        }
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id,
            replaced_assignment_id,
        } => {
            bytes.add(target_assignment_id.heap_capacity())?;
            bytes.add(replaced_assignment_id.heap_capacity())
        }
    }
}

struct FinalizeJobContext<'a> {
    request: &'a CounterfactualJobRequestV1,
    base: &'a StoredAcceptedResultV2,
    compilation: &'a CounterfactualCompilationBindingV1,
    input: &'a eutheto_domain_ir::RunInput,
    terminal: TerminalMaterial,
    budget: &'a ParentSolveBudget,
    live: &'a LiveJob,
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

    // Preserve the native reservation, durable request and replay admission as one lifecycle.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn start(
        &self,
        request: SolutionStartCounterfactualRequestV1,
        base: StoredAcceptedResultV2,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        let condition = validate_counterfactual_source(&request, &base)?;
        let persisted = self.build_start_request(&request, &base, condition)?;
        let provisional_live = self.provisional_live(&persisted);
        let Some((live, provisional)) = self
            .reserve_or_reuse_live(persisted.job_id, &persisted, &provisional_live)
            .await
        else {
            if self
                .find_existing_request(persisted.semantics.scenario_id, persisted.request_id)
                .await
                .map_err(counterfactual_store_error)?
                .is_some()
            {
                let started = self
                    .store
                    .start_counterfactual_job(persisted, base)
                    .await
                    .map_err(counterfactual_store_error)?;
                return Ok(started.record);
            }
            return Err(counterfactual_store_error(
                StoreError::CounterfactualCapacityExceeded {
                    maximum: MAX_ACTIVE_COUNTERFACTUAL_JOBS,
                },
            ));
        };
        if let Some(error) = owned_alarm_error(&live) {
            return Err(error);
        }

        let started = match self
            .store
            .start_counterfactual_job(persisted.clone(), base.clone())
            .await
        {
            Ok(started) => started,
            Err(error) => {
                if provisional && is_definite_no_start(&error) {
                    self.retire_live(persisted.job_id, &live).await;
                }
                if let Some(alarm_error) = CounterfactualRuntime::storage_failure_alarm_error(&live)
                {
                    return Err(alarm_error);
                }
                return Err(counterfactual_store_error(error));
            }
        };
        let was_reused = started.reused;
        if let Some(error) = owned_alarm_error(&live) {
            return Err(error);
        }

        let record = started.record;
        let live = if was_reused {
            self.rekey_live(record.request.job_id, &live, &record.request)
                .await
                .ok_or_else(|| {
                    invalid_request(
                        "solution.counterfactual_state_conflict",
                        "/jobId",
                        "The counterfactual job changed concurrently.",
                    )
                })?
        } else {
            live
        };
        {
            let mut state = lock_live_state(&live.state);
            state.owner = Some(record.request.clone());
        }
        if let Some(error) = owned_alarm_error(&live) {
            return Err(error);
        }
        let startup_recovery_pending =
            lock_live_state(&live.state).claim == LiveClaimState::Starting;
        if record.state == CounterfactualJobState::Running && startup_recovery_pending {
            let runtime = self.clone();
            let claim_live = live.clone();
            let claim = tokio::spawn(async move {
                Box::pin(runtime.claim_and_spawn(record, base, monotonic_clock, claim_live)).await
            });
            let claim_result = match claim.await {
                Ok(result) => result,
                Err(error) => Err(counterfactual_join_error(error)),
            };
            return match claim_result {
                Ok(record) => owned_alarm_error(&live).map_or(Ok(record), Err),
                Err(error) => {
                    Err(CounterfactualRuntime::storage_failure_alarm_error(&live).unwrap_or(error))
                }
            };
        }
        if record.state != CounterfactualJobState::Queued {
            if record.state != CounterfactualJobState::Running && !has_alarm(&live) {
                self.retire_live(record.request.job_id, &live).await;
            }
            return Ok(record);
        }
        if !was_reused {
            self.publish_live(
                &live,
                &record.request,
                None,
                CounterfactualProgressPhase::Queued,
            );
        }
        let preflight = match self.preflight_start(&request, &base, &record).await {
            Ok(preflight) => preflight,
            Err(error) => {
                return Err(
                    CounterfactualRuntime::storage_failure_alarm_error(&live).unwrap_or(error)
                );
            }
        };
        if let Some(error) = owned_alarm_error(&live) {
            return Err(error);
        }
        if let Some(terminal) = preflight {
            if !has_alarm(&live) {
                self.retire_live(record.request.job_id, &live).await;
            }
            return Ok(terminal);
        }
        let runtime = self.clone();
        let claim_live = live.clone();
        let claim = tokio::spawn(async move {
            Box::pin(runtime.claim_and_spawn(record, base, monotonic_clock, claim_live)).await
        });
        let claim_result = match claim.await {
            Ok(result) => result,
            Err(error) => Err(counterfactual_join_error(error)),
        };
        match claim_result {
            Ok(record) => owned_alarm_error(&live).map_or(Ok(record), Err),
            Err(error) => {
                Err(CounterfactualRuntime::storage_failure_alarm_error(&live).unwrap_or(error))
            }
        }
    }

    fn build_start_request(
        &self,
        request: &SolutionStartCounterfactualRequestV1,
        base: &StoredAcceptedResultV2,
        condition: CounterfactualConditionV1,
    ) -> Result<CounterfactualJobRequestV1, AppError> {
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
        CounterfactualJobRequestV1::new(
            CounterfactualJobId::new(self.ids.as_ref()).map_err(id_generation_error)?,
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
        })
    }

    fn provisional_live(&self, request: &CounterfactualJobRequestV1) -> LiveJob {
        LiveJob {
            cancellation: Some(self.cancellation.child()),
            startup: Arc::new(Mutex::new(())),
            state: Arc::new(StdMutex::new(LiveJobState {
                owner: Some(request.clone()),
                ..LiveJobState::default()
            })),
        }
    }

    async fn find_existing_request(
        &self,
        scenario_id: ScenarioId,
        request_id: RequestId,
    ) -> Result<Option<CounterfactualJobRecordV1>, StoreError> {
        let mut cursor: Option<CounterfactualJobListCursorV1> = None;
        loop {
            let page = self
                .store
                .list_counterfactual_jobs(scenario_id, cursor, MAX_COUNTERFACTUAL_JOB_LIST_LIMIT)
                .await?;
            if let Some(item) = page.jobs.iter().find(|item| item.request_id == request_id) {
                return self
                    .store
                    .load_counterfactual_job(item.job_id)
                    .await
                    .map(Some);
            }
            let Some(next) = page.next_cursor else {
                return Ok(None);
            };
            cursor = Some(next);
        }
    }

    async fn rekey_live(
        &self,
        job_id: CounterfactualJobId,
        live: &LiveJob,
        request: &CounterfactualJobRequestV1,
    ) -> Option<LiveJob> {
        let mut jobs = self.live.lock().await;
        rekey_live_owner_map(&mut jobs, job_id, live, request)
    }

    async fn retire_live(&self, job_id: CounterfactualJobId, live: &LiveJob) {
        let mut jobs = self.live.lock().await;
        if jobs
            .get(&job_id)
            .is_some_and(|current| Arc::ptr_eq(&current.state, &live.state))
        {
            jobs.remove(&job_id);
        }
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

    // Recovery readback and the single spawn claim share the same locked native owner.
    #[allow(clippy::too_many_lines)]
    async fn claim_and_spawn(
        &self,
        record: CounterfactualJobRecordV1,
        base: StoredAcceptedResultV2,
        monotonic_clock: Arc<dyn MonotonicClock>,
        live: LiveJob,
    ) -> Result<CounterfactualJobRecordV1, AppError> {
        let _startup = live.startup.lock().await;
        let disposition = match self.claim_reserved(record.request.job_id, &live).await {
            Some(ClaimDisposition::New) => ClaimDisposition::New,
            Some(ClaimDisposition::Recover) => ClaimDisposition::Recover,
            Some(ClaimDisposition::AlreadySpawned) | None => {
                let authority = self
                    .store
                    .load_counterfactual_job(record.request.job_id)
                    .await
                    .map_err(|error| {
                        CounterfactualRuntime::storage_failure_alarm_error(&live)
                            .unwrap_or_else(|| counterfactual_store_error(error))
                    })?;
                if let Some(error) = owned_alarm_error(&live) {
                    return Err(error);
                }
                if authority.request != record.request {
                    return Err(invalid_request(
                        "solution.counterfactual_state_conflict",
                        "/jobId",
                        "The counterfactual job changed concurrently.",
                    ));
                }
                return Ok(authority);
            }
        };

        let reconciled_running = if disposition == ClaimDisposition::Recover {
            let authority = self
                .store
                .load_counterfactual_job(record.request.job_id)
                .await
                .map_err(|error| {
                    CounterfactualRuntime::storage_failure_alarm_error(&live)
                        .unwrap_or_else(|| counterfactual_store_error(error))
                })?;
            if authority.request != record.request {
                return Err(invalid_request(
                    "solution.counterfactual_state_conflict",
                    "/jobId",
                    "The counterfactual job changed concurrently.",
                ));
            }
            match authority.state {
                CounterfactualJobState::Running => Some(authority),
                CounterfactualJobState::Queued => None,
                CounterfactualJobState::Completed
                | CounterfactualJobState::Failed
                | CounterfactualJobState::Cancelled
                | CounterfactualJobState::Interrupted => {
                    if let Some(error) = owned_alarm_error(&live) {
                        return Err(error);
                    }
                    self.retire_live(record.request.job_id, &live).await;
                    return Ok(authority);
                }
            }
        } else {
            None
        };
        if let Some(error) = owned_alarm_error(&live) {
            return Err(error);
        }

        let Some(cancellation) = live.cancellation.clone() else {
            if disposition == ClaimDisposition::New {
                release_live_claim(&live);
            }
            return Err(invalid_request(
                "solution.counterfactual_state_conflict",
                "/jobId",
                "The counterfactual job changed concurrently.",
            ));
        };
        let budget = match disposition {
            ClaimDisposition::New => {
                let Ok(budget) = ParentSolveBudget::new(
                    record.request.semantics.total_budget_milliseconds,
                    monotonic_clock,
                    cancellation,
                ) else {
                    let failed = match self
                        .fail_before_run(
                            record.request.job_id,
                            CounterfactualFailureKind::InvalidBinding,
                        )
                        .await
                    {
                        Ok(failed) => failed,
                        Err(error) => {
                            release_live_claim(&live);
                            return Err(error);
                        }
                    };
                    self.retire_live(record.request.job_id, &live).await;
                    return Ok(failed);
                };
                retain_startup_budget(&live, &budget);
                budget
            }
            ClaimDisposition::Recover => {
                let Some(budget) = startup_budget_for_recovery(&live) else {
                    return Err(invalid_request(
                        "solution.counterfactual_state_conflict",
                        "/jobId",
                        "The counterfactual job changed concurrently.",
                    ));
                };
                budget
            }
            ClaimDisposition::AlreadySpawned => unreachable!(),
        };

        let running = if let Some(running) = reconciled_running {
            running
        } else {
            match self
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
                Err(error) => {
                    let authority = match self
                        .store
                        .load_counterfactual_job(record.request.job_id)
                        .await
                    {
                        Ok(authority) => authority,
                        Err(read_error) => {
                            return Err(CounterfactualRuntime::storage_failure_alarm_error(&live)
                                .unwrap_or_else(|| counterfactual_store_error(read_error)));
                        }
                    };
                    if authority.request != record.request {
                        return Err(invalid_request(
                            "solution.counterfactual_state_conflict",
                            "/jobId",
                            "The counterfactual job changed concurrently.",
                        ));
                    }
                    match authority.state {
                        CounterfactualJobState::Running => authority,
                        CounterfactualJobState::Queued => {
                            return Err(counterfactual_store_error(error));
                        }
                        CounterfactualJobState::Completed
                        | CounterfactualJobState::Failed
                        | CounterfactualJobState::Cancelled
                        | CounterfactualJobState::Interrupted => {
                            if let Some(alarm_error) = owned_alarm_error(&live) {
                                return Err(alarm_error);
                            }
                            self.retire_live(record.request.job_id, &live).await;
                            return Ok(authority);
                        }
                    }
                }
            }
        };
        if running.request != record.request {
            return Err(invalid_request(
                "solution.counterfactual_state_conflict",
                "/jobId",
                "The counterfactual job changed concurrently.",
            ));
        }
        self.publish_live(
            &live,
            &running.request,
            None,
            CounterfactualProgressPhase::Compiling,
        );
        self.spawn_supervised(&running, base, budget, &live);
        mark_live_claim_spawned(&live);
        Ok(running)
    }

    async fn reserve_or_reuse_live(
        &self,
        job_id: CounterfactualJobId,
        request: &CounterfactualJobRequestV1,
        provisional: &LiveJob,
    ) -> Option<(LiveJob, bool)> {
        let mut jobs = self.live.lock().await;
        for existing in jobs.values() {
            let state = lock_live_state(&existing.state);
            if state.owner.as_ref().is_some_and(|owner| {
                owner.request_id == request.request_id
                    && owner.semantics == request.semantics
                    && owner.condition == request.condition
            }) {
                return Some((existing.clone(), false));
            }
        }
        if jobs.contains_key(&job_id)
            || jobs
                .values()
                .filter(|owner| counts_toward_native_owner_limit(owner))
                .count()
                >= usize::try_from(MAX_ACTIVE_COUNTERFACTUAL_JOBS).unwrap_or(usize::MAX)
        {
            return None;
        }
        jobs.insert(job_id, provisional.clone());
        Some((provisional.clone(), true))
    }

    async fn claim_reserved(
        &self,
        job_id: CounterfactualJobId,
        live: &LiveJob,
    ) -> Option<ClaimDisposition> {
        let jobs = self.live.lock().await;
        let current = jobs.get(&job_id)?;
        if !Arc::ptr_eq(&current.state, &live.state) {
            return None;
        }
        let mut state = lock_live_state(&live.state);
        claim_live_state(&mut state)
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
            let failed = !matches!(joined, Ok(Ok(())));
            if has_alarm(&supervisor_live) {
                supervisor_runtime
                    .settle_owned_alarm(supervised_job_id, &supervisor_live)
                    .await;
            } else if failed {
                supervisor_runtime
                    .ensure_terminal(supervised_job_id, &supervisor_live)
                    .await;
            }
            if has_alarm(&supervisor_live) {
                supervisor_runtime
                    .retain_settled_owner(supervised_job_id, &supervisor_live)
                    .await;
            } else {
                supervisor_runtime
                    .retire_live(supervised_job_id, &supervisor_live)
                    .await;
            }
        });
        drop(supervisor);
    }

    // Isolate the durable cancellation state from every command-dispatch future.
    pub(crate) fn cancel<'a>(
        &'a self,
        request: &'a SolutionCancelCounterfactualRequestV1,
    ) -> impl Future<Output = Result<CounterfactualJobRecordV1, AppError>> + 'a {
        Box::pin(async move {
            if request.schema_version != COUNTERFACTUAL_API_SCHEMA_VERSION {
                return Err(schema_error(request.schema_version));
            }
            self.guard_cancel_publication(
                request.scenario_id,
                request.expected_revision,
                request.job_id,
            )
            .await?;
            let job = match self.store.load_counterfactual_job(request.job_id).await {
                Ok(job) => job,
                Err(error) => {
                    return Err(self
                        .storage_failure_alarm_for_job(
                            request.scenario_id,
                            request.expected_revision,
                            request.job_id,
                        )
                        .await
                        .unwrap_or_else(|| counterfactual_store_error(error)));
                }
            };
            if job.request.semantics.scenario_id != request.scenario_id
                || job.request.semantics.scenario_revision != request.expected_revision.value()
            {
                return Err(counterfactual_cancel_binding_error());
            }
            self.guard_job_publication(request.scenario_id, request.job_id)
                .await?;
            let outcome = match self
                .store
                .request_counterfactual_cancel(
                    request.job_id,
                    request.cancel_request_id,
                    self.clock.now(),
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Err(self
                        .storage_failure_alarm_for_job(
                            request.scenario_id,
                            request.expected_revision,
                            request.job_id,
                        )
                        .await
                        .unwrap_or_else(|| counterfactual_store_error(error)));
                }
            };
            let record = match outcome {
                CounterfactualCancelOutcomeV1::Requested { record, reused } => {
                    if let Some(live) = self.live_for_owner(request.job_id).await {
                        if let Some(cancellation) = &live.cancellation {
                            cancellation.cancel();
                        }
                        if !reused && record.state == CounterfactualJobState::Cancelled {
                            self.publish_live(
                                &live,
                                &record.request,
                                None,
                                CounterfactualProgressPhase::Cancelled,
                            );
                        }
                    } else if !reused && record.state == CounterfactualJobState::Cancelled {
                        // Only a first queued cancellation commits Cancelled without a run owner.
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
            self.guard_job_publication(request.scenario_id, request.job_id)
                .await?;
            Ok(record)
        })
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
            objective_context: base.portable.run_input.objective_context.clone(),
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
                    objective_context: base.portable.run_input.objective_context.clone(),
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
        {
            let mut state = lock_live_state(&live.state);
            state.derived = Some(DerivedExecution {
                input: started.input.clone(),
                started_at: started.started_at,
                budget: budget.clone(),
                native_observations: None,
            });
            if state
                .trusted_elapsed
                .is_none_or(|previous| compile_finished >= previous)
            {
                state.trusted_elapsed = Some(compile_finished);
            }
            let owner_created_at = state.owner.as_ref().map(|owner| owner.created_at);
            retain_publication_wall(&mut state, owner_created_at, started.started_at);
        }
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
        self.publish_live(
            &live,
            request,
            Some(loaded.input.run_id),
            CounterfactualProgressPhase::Solving,
        );

        let derived_problem = Arc::new(derived_problem);
        let verification_clock = ParentVerificationClock::new(&budget);
        let event_runtime = self.clone();
        let event_request = request.clone();
        let event_live = live.clone();
        let event_run_id = loaded.input.run_id;
        let review_live = live.clone();
        let review_input = &started.input;
        let review_condition = &request.condition;
        let review_clock = &verification_clock;
        let mut verifying_emitted = false;
        let mut reviewer = RouterCandidateReviewer::new(
            pack,
            &loaded.document,
            request.semantics.scenario_revision,
            derived_problem.as_ref(),
            started.input.objective_context.as_ref(),
            &verification_clock,
            self.ids.as_ref(),
            &compile_context.control,
        )
        .map_err(|_| ())?
        .with_before_review(move || {
            if !verifying_emitted {
                event_runtime.publish_live(
                    &event_live,
                    &event_request,
                    Some(event_run_id),
                    CounterfactualProgressPhase::Verifying,
                );
                verifying_emitted = true;
            }
        })
        .with_after_review(move |decision| {
            if let Some(elapsed_bound) = review_clock.latest_elapsed_bound() {
                CounterfactualRuntime::retain_elapsed_value(&review_live, elapsed_bound);
            }
            let diagnostic_code = match decision {
                AcceptanceDecision::Quarantined { alarm, .. }
                    if alarm.category != eutheto_verify::CorrectnessAlarmCategory::ClockFailed =>
                {
                    Some(alarm.diagnostic_code.as_str())
                }
                AcceptanceDecision::Accepted { result, .. }
                    if !counterfactual_condition_satisfied(review_condition, &result.solution) =>
                {
                    Some(COUNTERFACTUAL_CONDITION_MISMATCH_DIAGNOSTIC)
                }
                _ => None,
            };
            if let Some(diagnostic_code) = diagnostic_code {
                CounterfactualRuntime::capture_alarm(&review_live, review_input, diagnostic_code);
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
        let native_observations = Arc::new(
            crate::native_observations::capture(
                &started.input,
                &derived_problem,
                &self.solvers,
                &execution.record,
            )
            .map_err(|_| ())?,
        );
        {
            let mut state = lock_live_state(&live.state);
            let derived = state.derived.as_mut().ok_or(())?;
            if derived.input.checksum != started.input.checksum
                || derived.native_observations.is_some()
            {
                return Err(());
            }
            derived.native_observations = Some(Arc::clone(&native_observations));
        }
        if let Some(elapsed_bound) = execution.last_trusted_elapsed {
            CounterfactualRuntime::retain_elapsed_value(&live, elapsed_bound);
        }
        if let Some(elapsed_bound) = verification_clock.latest_elapsed_bound() {
            CounterfactualRuntime::retain_elapsed_value(&live, elapsed_bound);
        }
        let established_alarm_code = reviewer
            .established_alarm()
            .filter(|alarm| alarm.category != eutheto_verify::CorrectnessAlarmCategory::ClockFailed)
            .map(|alarm| alarm.diagnostic_code.as_str())
            .or_else(|| match reviewer.decision() {
                Some(AcceptanceDecision::Accepted { result, .. })
                    if !counterfactual_condition_satisfied(
                        &request.condition,
                        &result.solution,
                    ) =>
                {
                    Some(COUNTERFACTUAL_CONDITION_MISMATCH_DIAGNOSTIC)
                }
                _ => None,
            });
        if let Some(code) = established_alarm_code {
            CounterfactualRuntime::capture_alarm(&live, &started.input, code);
            self.observe_alarm_sample(
                &live,
                &budget,
                execution
                    .observations
                    .as_ref()
                    .map(|observations| observations.finished_after),
            );
            return Err(());
        }
        if has_alarm(&live) {
            self.observe_alarm_sample(
                &live,
                &budget,
                execution
                    .observations
                    .as_ref()
                    .map(|observations| observations.finished_after),
            );
            return Err(());
        }
        let review_timings = reviewer.total_review_timings()?;
        let decision = reviewer.into_decision();
        if verification_clock.has_failed() {
            return Err(());
        }
        let observations = execution.observations.as_ref().ok_or(())?;
        CounterfactualRuntime::retain_elapsed_value(&live, observations.finished_after);
        subtract_duration(observations.started_after, compile_finished).map_err(|_| ())?;
        // Capture the derived run's actual infeasibility before any optional explanation work.
        // This observation is deliberately made from the routed execution, not inferred from
        // a later deadline/cancellation state.
        let original_candidate = decision.is_none()
            && runtime_evidence_matches(&execution.record, &identity, &options)
            && matches!(execution.record.terminal_status, SolveStatus::Infeasible);
        let original_observed_at = original_candidate.then(|| self.clock.now());
        if let Some(observed_at) = original_observed_at {
            CounterfactualRuntime::observe_wall(&live, observed_at);
        }
        let original_infeasibility = if let Some(observed_at) = original_observed_at {
            capture_original_infeasibility(
                &started.input,
                backend.as_ref(),
                &budget,
                started.started_at,
                observed_at,
            )
            .map_err(|_| ())?
        } else {
            None
        };
        let original_observation_late =
            if original_candidate && original_infeasibility.is_none() && !budget.is_cancelled() {
                let observed_at = original_observed_at.ok_or(())?;
                let deadline = started
                    .started_at
                    .as_timestamp()
                    .checked_add(Duration::from_millis(
                        started.input.solve_options.time_limit_milliseconds.value(),
                    ))
                    .map_err(|_| ())?;
                observed_at.as_timestamp() >= deadline
            } else {
                false
            };

        // Diagnostic compilation uses the immutable original document and the exact
        // condition-bound ordinary model; it never mutates that admitted derived model.
        let mut inert_infeasibility = None;
        let mut diagnostic_alarm: Option<CorrectnessAlarm> = None;
        let mut optional_explanation_elapsed = None;
        if let Some(original) = original_infeasibility {
            let diagnostic_started =
                CounterfactualRuntime::observe_elapsed(&live, &budget).map_err(|_| ())?;
            let material = diagnose_infeasibility(InfeasibilityDiagnosticContext {
                pack,
                document: &base.document,
                base_problem: derived_problem.as_ref(),
                compile_context: &compile_context,
                backend: backend.as_ref(),
                parent_budget: &budget,
                verification_clock: &verification_clock,
                ids: self.ids.as_ref(),
                options: &options,
                condition: Some(&request.condition),
            })
            .await
            .map_err(|_| ())?;
            match material {
                InfeasibilityDiagnosticMaterial::Evidence {
                    infeasibility,
                    proof,
                } => {
                    inert_infeasibility = Some(InertInfeasibilityEvidence {
                        original,
                        condition: Some(request.condition.clone()),
                        diagnostic: proof,
                        infeasibility,
                    });
                }
                InfeasibilityDiagnosticMaterial::Quarantined { alarm } => {
                    if alarm.category == eutheto_verify::CorrectnessAlarmCategory::ClockFailed {
                        return Err(());
                    }
                    if CounterfactualRuntime::capture_alarm(
                        &live,
                        &started.input,
                        &alarm.diagnostic_code,
                    ) {
                        self.observe_alarm_sample(
                            &live,
                            &budget,
                            Some(observations.finished_after),
                        );
                        return Err(());
                    }
                    diagnostic_alarm = Some(alarm);
                }
            }
            let diagnostic_finished =
                CounterfactualRuntime::observe_elapsed(&live, &budget).map_err(|_| ())?;
            optional_explanation_elapsed = Some(
                observed_milliseconds(
                    subtract_duration(diagnostic_finished, diagnostic_started).map_err(|_| ())?,
                )
                .map_err(|_| ())?,
            );
        }

        let finished_at = self.clock.now();
        CounterfactualRuntime::observe_wall(&live, finished_at);
        let elapsed_total =
            CounterfactualRuntime::observe_elapsed(&live, &budget).map_err(|_| ())?;
        subtract_duration(elapsed_total, compile_finished).map_err(|_| ())?;
        subtract_duration(elapsed_total, observations.finished_after).map_err(|_| ())?;
        let mut terminal = build_terminal(TerminalBuildContext {
            record: &execution.record,
            observations,
            decision: decision.as_ref(),
            identity: &identity,
            options: &options,
            review_timings,
            compile_elapsed,
            elapsed_total: observed_milliseconds(elapsed_total).map_err(|_| ())?,
            started_at: started.started_at,
            finished_at,
            input: &started.input,
            condition: &request.condition,
            infeasibility: inert_infeasibility,
            diagnostic_alarm,
            optional_explanation_elapsed,
            original_observation_late,
        })
        .map_err(|_| ())?;
        terminal.manifest = terminal
            .manifest
            .with_native_observations(Some(Arc::unwrap_or_clone(native_observations)))
            .map_err(|_| ())?;
        if let RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code } =
            &terminal.manifest.outcome
            && CounterfactualRuntime::capture_alarm(&live, &started.input, diagnostic_code)
        {
            self.observe_alarm_sample(&live, &budget, Some(observations.finished_after));
            return Err(());
        }
        self.publish_live(
            &live,
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
            live: &live,
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
            live,
            previous_elapsed,
        } = context;
        // Load the one durable cancellation marker before refreshing any final timing.  The
        // marker is passed unchanged to the transaction; it is never reconstructed from clocks.
        let current = self.store.load_counterfactual_job(request.job_id).await?;
        let expected_cancellation = cancellation_observation(&current)?;

        let evidence_started = CounterfactualRuntime::observe_elapsed(live, budget)?;
        subtract_duration(evidence_started, previous_elapsed)?;
        let prepared_evidence = terminal.alternative.as_ref().map(accepted_evidence);
        let evidence_finished = CounterfactualRuntime::observe_elapsed(live, budget)?;
        let evidence_preparation_elapsed =
            observed_milliseconds(subtract_duration(evidence_finished, evidence_started)?)?;
        let finished_at = self.clock.now();
        CounterfactualRuntime::observe_wall(live, finished_at);
        let terminal = terminal.retime(
            observed_milliseconds(evidence_finished)?,
            finished_at,
            budget.is_expired(),
            evidence_preparation_elapsed,
        )?;
        let finalization = prepare_finalization(FinalizationContext {
            request,
            base,
            compilation,
            input,
            terminal,
            prepared_evidence,
        })?;
        match self
            .store
            .finalize_counterfactual_run(request.job_id, finalization, expected_cancellation)
            .await
        {
            Ok(record) => {
                self.publish_terminal_live(&record, Some(input.run_id), live);
                Ok(record)
            }
            Err(StoreError::StaleCounterfactualFinalizationObservation {
                cancellation,
                finalization,
            }) => {
                // The store returned ownership of the untouched material and performed no
                // writes. Re-observe once, causally after the returned marker, then resubmit
                // that same material with the exact marker. This is not a retry loop.
                let recovered_elapsed = CounterfactualRuntime::observe_elapsed(live, budget)?;
                let recovered_finished_at = self.clock.now();
                CounterfactualRuntime::observe_wall(live, recovered_finished_at);
                if recovered_finished_at.as_timestamp()
                    < cancellation.cancel_requested_at.as_timestamp()
                {
                    return Err(StoreError::InvalidPersistedRun(
                        "causal cancellation observation moved backwards".to_owned(),
                    ));
                }
                let recovery_evidence_preparation_elapsed =
                    observed_milliseconds(subtract_duration(recovered_elapsed, evidence_started)?)?;
                let recovered = retime_stale_finalization(
                    *finalization,
                    request,
                    base,
                    compilation,
                    input,
                    observed_milliseconds(recovered_elapsed)?,
                    recovered_finished_at,
                    budget.is_expired(),
                    recovery_evidence_preparation_elapsed,
                    &cancellation,
                )?;
                let record = self
                    .store
                    .finalize_counterfactual_run(request.job_id, recovered, Some(cancellation))
                    .await?;
                self.publish_terminal_live(&record, Some(input.run_id), live);
                Ok(record)
            }
            Err(StoreError::CounterfactualTransitionConflict(_)) => {
                let current = self.store.load_counterfactual_job(request.job_id).await?;
                if is_terminal(current.state) {
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
        if has_alarm(live) {
            self.settle_owned_alarm(job_id, live).await;
            return;
        }
        let Ok(current) = self.store.load_counterfactual_job(job_id).await else {
            return;
        };
        if has_alarm(live) {
            self.settle_owned_alarm(job_id, live).await;
            return;
        }
        if is_terminal(current.state) {
            return;
        }
        let expected_cancellation = cancellation_observation(&current).ok().flatten();
        let derived = lock_live_state(&live.state).derived.clone();
        let result = match derived.as_ref() {
            Some(derived) if current.cancel_request_id.is_some() => {
                match elapsed(&derived.budget)
                    .and_then(observed_milliseconds)
                    .and_then(|elapsed| {
                        cancelled_manifest(
                            &derived.input,
                            derived.started_at,
                            self.clock.now(),
                            elapsed,
                        )
                        .and_then(|manifest| {
                            manifest
                                .with_native_observations(
                                    derived
                                        .native_observations
                                        .as_ref()
                                        .map(|observations| observations.as_ref().clone()),
                                )
                                .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
                        })
                    }) {
                    Ok(manifest) => {
                        self.store
                            .finalize_counterfactual_run(
                                job_id,
                                CounterfactualRunFinalizationV1::Cancelled { manifest },
                                expected_cancellation,
                            )
                            .await
                    }
                    Err(error) => Err(error),
                }
            }
            Some(derived) => {
                match interrupted_manifest(&derived.input, derived.started_at, self.clock.now())
                    .and_then(|manifest| {
                        manifest
                            .with_native_observations(
                                derived
                                    .native_observations
                                    .as_ref()
                                    .map(|observations| observations.as_ref().clone()),
                            )
                            .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
                    }) {
                    Ok(manifest) => {
                        self.store
                            .finalize_counterfactual_run(
                                job_id,
                                CounterfactualRunFinalizationV1::Interrupted { manifest },
                                expected_cancellation,
                            )
                            .await
                    }
                    Err(error) => Err(error),
                }
            }
            None => {
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
            }
        };
        if let Ok(record) = result {
            self.publish_terminal_live(
                &record,
                derived.as_ref().map(|value| value.input.run_id),
                live,
            );
        }
    }

    async fn live_for_owner(&self, job_id: CounterfactualJobId) -> Option<LiveJob> {
        let jobs = self.live.lock().await;
        jobs.values()
            .find(|live| {
                let state = lock_live_state(&live.state);
                state
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.job_id == job_id)
                    && state.alarm.is_some()
            })
            .or_else(|| {
                jobs.values().find(|live| {
                    lock_live_state(&live.state)
                        .owner
                        .as_ref()
                        .is_some_and(|owner| owner.job_id == job_id)
                })
            })
            .cloned()
    }

    pub(crate) async fn diagnostic_run_handle(
        &self,
        scenario_id: ScenarioId,
        source_revision: Revision,
        run_id: SolveRunId,
    ) -> Option<CounterfactualPublicationHandle> {
        let jobs = self.live.lock().await;
        jobs.values().find_map(|live| {
            let state = lock_live_state(&live.state);
            let owner = state.owner.as_ref()?;
            if owner.semantics.scenario_id != scenario_id
                || owner.semantics.scenario_revision != source_revision.value()
            {
                return None;
            }
            let input_checksum = if let Some(derived) = &state.derived {
                let input = &derived.input;
                if input.scenario_id != scenario_id
                    || input.scenario_revision != source_revision.value()
                    || input.run_id != run_id
                    || state.alarm.as_ref().is_some_and(|alarm| {
                        alarm.run_id != run_id || alarm.input_checksum != input.checksum
                    })
                {
                    return None;
                }
                input.checksum.clone()
            } else if let Some(alarm) = &state.alarm {
                if alarm.run_id != run_id {
                    return None;
                }
                alarm.input_checksum.clone()
            } else {
                return None;
            };
            Some(CounterfactualPublicationHandle {
                state: Arc::clone(&live.state),
                scenario_id,
                source_revision,
                run_id,
                input_checksum,
            })
        })
    }

    pub(crate) async fn guard_cancel_publication(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        job_id: CounterfactualJobId,
    ) -> Result<(), AppError> {
        let Some(live) = self.live_for_owner(job_id).await else {
            return Ok(());
        };
        let state = lock_live_state(&live.state);
        let Some(owner) = state.owner.as_ref() else {
            return Ok(());
        };
        if !cancel_owner_matches(owner, scenario_id, expected_revision, job_id) {
            return Err(counterfactual_cancel_binding_error());
        }
        if let Some(alarm) = state.alarm.as_ref() {
            return Err(known_alarm_error(
                alarm,
                if state.settlement == AlarmSettlement::SettledCompatible {
                    VerificationAlarmTerminalization::Confirmed
                } else {
                    VerificationAlarmTerminalization::Unconfirmed
                },
                state.secondary_cause,
            ));
        }
        Ok(())
    }

    pub(crate) async fn guard_job_publication(
        &self,
        scenario_id: ScenarioId,
        job_id: CounterfactualJobId,
    ) -> Result<(), AppError> {
        let live = self.live.lock().await.get(&job_id).cloned();
        let Some(live) = live else {
            return Ok(());
        };
        let state = lock_live_state(&live.state);
        let Some(owner) = state.owner.as_ref() else {
            return Ok(());
        };
        if owner.job_id != job_id || owner.semantics.scenario_id != scenario_id {
            return Ok(());
        }
        let Some(alarm) = state.alarm.as_ref() else {
            return Ok(());
        };
        Err(known_alarm_error(
            alarm,
            if state.settlement == AlarmSettlement::SettledCompatible {
                VerificationAlarmTerminalization::Confirmed
            } else {
                VerificationAlarmTerminalization::Unconfirmed
            },
            state.secondary_cause,
        ))
    }

    pub(crate) async fn guard_run_publication(
        &self,
        scenario_id: ScenarioId,
        run_id: SolveRunId,
    ) -> Result<(), AppError> {
        let jobs = self.live.lock().await;
        for live in jobs.values() {
            let state = lock_live_state(&live.state);
            let Some(owner) = state.owner.as_ref() else {
                continue;
            };
            if owner.semantics.scenario_id != scenario_id
                || (state
                    .derived
                    .as_ref()
                    .is_none_or(|derived| derived.input.run_id != run_id)
                    && state
                        .alarm
                        .as_ref()
                        .is_none_or(|alarm| alarm.run_id != run_id))
            {
                continue;
            }
            let Some(alarm) = state.alarm.as_ref() else {
                return Ok(());
            };
            return Err(known_alarm_error(
                alarm,
                if state.settlement == AlarmSettlement::SettledCompatible {
                    VerificationAlarmTerminalization::Confirmed
                } else {
                    VerificationAlarmTerminalization::Unconfirmed
                },
                state.secondary_cause,
            ));
        }
        Ok(())
    }

    pub(crate) async fn guard_job_explanation_publication(
        &self,
        scenario_id: ScenarioId,
        source_revision: Revision,
        job_id: CounterfactualJobId,
        base: &AcceptedResultRefV1,
    ) -> Result<(), AppError> {
        let live = self.live.lock().await.get(&job_id).cloned();
        let Some(live) = live else {
            return Ok(());
        };
        let state = lock_live_state(&live.state);
        let Some(owner) = state.owner.as_ref() else {
            return Ok(());
        };
        if owner.job_id != job_id
            || owner.semantics.scenario_id != scenario_id
            || owner.semantics.scenario_revision != source_revision.value()
            || &owner.semantics.base != base
        {
            return Err(invalid_request(
                "solution.counterfactual_explanation_binding",
                "/subject",
                "The counterfactual explanation reference does not match its immutable source.",
            ));
        }
        let Some(alarm) = state.alarm.as_ref() else {
            return Ok(());
        };
        Err(known_alarm_error(
            alarm,
            if state.settlement == AlarmSettlement::SettledCompatible {
                VerificationAlarmTerminalization::Confirmed
            } else {
                VerificationAlarmTerminalization::Unconfirmed
            },
            state.secondary_cause,
        ))
    }

    fn capture_alarm(
        live: &LiveJob,
        input: &eutheto_domain_ir::RunInput,
        diagnostic_code: &str,
    ) -> bool {
        if diagnostic_code == eutheto_verify::CorrectnessAlarmCategory::ClockFailed.code() {
            return false;
        }
        let mut state = lock_live_state(&live.state);
        if state.alarm.is_none() {
            let public_context = KnownVerificationAlarmV1::new(
                input.run_id,
                &input.checksum,
                diagnostic_code,
                VerificationAlarmTerminalization::Unconfirmed,
                None,
            )
            .ok();
            state.alarm = Some(OwnedAlarm {
                run_id: input.run_id,
                input_checksum: input.checksum.clone(),
                diagnostic_code: diagnostic_code.to_owned(),
                public_context,
            });
        }
        true
    }

    fn observe_alarm_sample(
        &self,
        live: &LiveJob,
        budget: &ParentSolveBudget,
        router_finished: Option<Duration>,
    ) {
        if let Some(router_finished) = router_finished
            && router_finished.as_millis() <= u128::from(DurationMillis::MAX.value())
        {
            CounterfactualRuntime::retain_elapsed_value(live, router_finished);
        }
        if let Ok(elapsed) = elapsed(budget) {
            CounterfactualRuntime::retain_elapsed_value(live, elapsed);
        }
        let wall = self.clock.now();
        CounterfactualRuntime::observe_wall(live, wall);
    }

    fn retain_elapsed_value(live: &LiveJob, observed: Duration) {
        let mut state = lock_live_state(&live.state);
        if state
            .trusted_elapsed
            .is_none_or(|previous| observed >= previous)
        {
            state.trusted_elapsed = Some(observed);
        }
    }

    fn observe_elapsed(live: &LiveJob, budget: &ParentSolveBudget) -> Result<Duration, StoreError> {
        let observed = elapsed(budget)?;
        let mut state = lock_live_state(&live.state);
        if state
            .trusted_elapsed
            .is_none_or(|previous| observed >= previous)
        {
            state.trusted_elapsed = Some(observed);
        }
        Ok(observed)
    }

    fn observe_wall(live: &LiveJob, observed: Rfc3339Timestamp) {
        let mut state = lock_live_state(&live.state);
        let started_at = state.derived.as_ref().map(|derived| derived.started_at);
        if started_at.is_some_and(|started_at| observed >= started_at)
            && state
                .trusted_wall
                .is_none_or(|previous| observed >= previous)
        {
            state.trusted_wall = Some(observed);
        }
    }

    // Keep the ordered durable-alarm reconciliation in one source-owning heap boundary.
    #[allow(clippy::too_many_lines)]
    fn settle_owned_alarm<'a>(
        &'a self,
        job_id: CounterfactualJobId,
        live: &'a LiveJob,
    ) -> impl Future<Output = ()> + 'a {
        Box::pin(async move {
            let (owner, derived, alarm, trusted_elapsed, trusted_wall) = {
                let state = lock_live_state(&live.state);
                (
                    state.owner.clone(),
                    state.derived.clone(),
                    state.alarm.clone(),
                    state.trusted_elapsed,
                    state.trusted_wall,
                )
            };
            let (Some(owner), Some(derived), Some(alarm)) = (owner, derived, alarm) else {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            };
            let Ok(current) = self.store.load_counterfactual_job(job_id).await else {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            };
            if current.request != owner {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            }
            if is_terminal(current.state) {
                let compatible = self
                    .compatible_alarm_readback(&owner, &alarm, &derived, &current)
                    .await
                    .unwrap_or(false);
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    if compatible {
                        AlarmSettlement::SettledCompatible
                    } else {
                        AlarmSettlement::SettledUnresolved
                    },
                    (!compatible)
                        .then_some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                if compatible {
                    self.publish_confirmed_alarm(&current, alarm.run_id, live);
                }
                return;
            }
            if current.state != CounterfactualJobState::Running
                || current.result.is_some()
                || current.started_at != Some(derived.started_at)
            {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            }
            let Ok(Some(run)) = self
                .store
                .load_solve_run(owner.semantics.scenario_id, alarm.run_id)
                .await
            else {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            };
            if run.input != derived.input
                || run.started_at != derived.started_at
                || run.manifest.is_some()
                || run.input.run_id != alarm.run_id
                || run.input.checksum != alarm.input_checksum
                || run.input.scenario_id != owner.semantics.scenario_id
                || run.input.scenario_revision != owner.semantics.scenario_revision
                || run.input.temporary_condition_hash.as_ref() != Some(&owner.condition.checksum)
            {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            }
            let evidence = self
                .store
                .load_run_infeasibility_evidence(alarm.run_id)
                .await;
            let checkpoint = self.store.load_verified_checkpoint(alarm.run_id).await;
            if !matches!(evidence, Ok(None)) || !matches!(checkpoint, Ok(None)) {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            }
            let Ok(cancellation) = cancellation_observation(&current) else {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            };

            // This is the sole terminal timing sample. Invalid final clocks preserve only their
            // actual wall observation in the all-unmeasured marked alarm.
            let finished_at = self.clock.now();
            let elapsed_sample = derived.budget.checked_elapsed();
            let elapsed_milliseconds = trusted_alarm_timing_sample(
                elapsed_sample,
                trusted_elapsed,
                finished_at,
                derived.started_at,
                trusted_wall,
                cancellation.as_ref(),
            );
            let valid_sample = elapsed_milliseconds.is_some();
            if !valid_sample {
                CounterfactualRuntime::set_alarm_cause(
                    live,
                    KnownVerificationSecondaryCauseCode::SolveClockInvalid,
                );
            }
            let manifest = if valid_sample {
                alarm_manifest(
                    &derived.input,
                    derived.started_at,
                    finished_at,
                    elapsed_milliseconds.unwrap_or(DurationMillis::ZERO),
                    &alarm.diagnostic_code,
                )
            } else {
                RunManifest::new_clock_failed_alarm(
                    alarm.run_id,
                    alarm.input_checksum.clone(),
                    alarm.diagnostic_code.clone(),
                    derived.started_at,
                    finished_at,
                )
                .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
            };
            let manifest = manifest.and_then(|manifest| {
                manifest
                    .with_native_observations(
                        derived
                            .native_observations
                            .as_ref()
                            .map(|observations| observations.as_ref().clone()),
                    )
                    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
            });
            let Ok(manifest) = manifest else {
                CounterfactualRuntime::set_alarm_settlement(
                    live,
                    AlarmSettlement::SettledUnresolved,
                    Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
                );
                return;
            };
            let finalization = CounterfactualRunFinalizationV1::Quarantined {
                manifest,
                candidate_diagnostics: CandidateDiagnosticsV1::default(),
                error: CounterfactualJobErrorV1 {
                    kind: CounterfactualFailureKind::InvalidCandidate,
                },
            };
            let persisted = self
                .store
                .finalize_counterfactual_run(job_id, finalization, cancellation)
                .await;
            if persisted.is_err() {
                CounterfactualRuntime::set_alarm_cause(
                    live,
                    KnownVerificationSecondaryCauseCode::StorageOperationFailed,
                );
            }
            let readback = self.store.load_counterfactual_job(job_id).await;
            let compatible = match readback {
                Ok(record) => self
                    .compatible_alarm_readback(&owner, &alarm, &derived, &record)
                    .await
                    .unwrap_or(false)
                    .then_some(record),
                Err(_) => None,
            };
            CounterfactualRuntime::set_alarm_settlement(
                live,
                if compatible.is_some() {
                    AlarmSettlement::SettledCompatible
                } else {
                    AlarmSettlement::SettledUnresolved
                },
                compatible
                    .is_none()
                    .then_some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
            );
            if let Some(record) = compatible {
                self.publish_confirmed_alarm(&record, alarm.run_id, live);
            }
        })
    }

    async fn compatible_alarm_readback(
        &self,
        owner: &CounterfactualJobRequestV1,
        alarm: &OwnedAlarm,
        derived: &DerivedExecution,
        record: &CounterfactualJobRecordV1,
    ) -> Result<bool, StoreError> {
        if record.request != *owner
            || record.state != CounterfactualJobState::Failed
            || record.result.is_some()
            || record.error
                != Some(CounterfactualJobErrorV1 {
                    kind: CounterfactualFailureKind::InvalidCandidate,
                })
            || record.started_at != Some(derived.started_at)
        {
            return Ok(false);
        }
        let run = self
            .store
            .load_solve_run(owner.semantics.scenario_id, alarm.run_id)
            .await?;
        let Some(run) = run else {
            return Ok(false);
        };
        let Some(manifest) = run.manifest.as_ref() else {
            return Ok(false);
        };
        let terminal_authority_matches = matches!(
            &record.terminal_authority,
            Some(CounterfactualTerminalAuthorityV1::VerificationAlarm {
                run_id,
                run_manifest_checksum,
                clock_failure,
            }) if *run_id == alarm.run_id
                && run_manifest_checksum == &manifest.checksum
                && clock_failure == &manifest.clock_failure
        );
        let code_matches = matches!(
            &manifest.outcome,
            RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }
                if diagnostic_code == &alarm.diagnostic_code
        );
        if !terminal_authority_matches
            || !code_matches
            || run.input != derived.input
            || run.started_at != derived.started_at
            || run.input.run_id != alarm.run_id
            || run.input.checksum != alarm.input_checksum
            || run.input.scenario_id != owner.semantics.scenario_id
            || run.input.scenario_revision != owner.semantics.scenario_revision
            || run.input.temporary_condition_hash.as_ref() != Some(&owner.condition.checksum)
            || manifest.run_id != alarm.run_id
            || manifest.run_input_checksum != alarm.input_checksum
            || manifest.started_at != derived.started_at
            || record.finished_at != Some(manifest.finished_at)
        {
            return Ok(false);
        }
        Ok(self
            .store
            .load_run_infeasibility_evidence(alarm.run_id)
            .await?
            .is_none()
            && self
                .store
                .load_verified_checkpoint(alarm.run_id)
                .await?
                .is_none())
    }

    fn set_alarm_cause(live: &LiveJob, cause: KnownVerificationSecondaryCauseCode) {
        let mut state = lock_live_state(&live.state);
        state.secondary_cause.get_or_insert(cause);
    }

    fn storage_failure_alarm_error(live: &LiveJob) -> Option<AppError> {
        if !has_alarm(live) {
            return None;
        }
        CounterfactualRuntime::set_alarm_cause(
            live,
            KnownVerificationSecondaryCauseCode::StorageOperationFailed,
        );
        owned_alarm_error(live)
    }

    async fn storage_failure_alarm_for_job(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        job_id: CounterfactualJobId,
    ) -> Option<AppError> {
        let live = self.live_for_owner(job_id).await?;
        let mut state = lock_live_state(&live.state);
        let owner = state.owner.as_ref()?;
        if !cancel_owner_matches(owner, scenario_id, expected_revision, job_id) {
            return None;
        }
        state
            .secondary_cause
            .get_or_insert(KnownVerificationSecondaryCauseCode::StorageOperationFailed);
        let alarm = state.alarm.as_ref()?;
        Some(known_alarm_error(
            alarm,
            if state.settlement == AlarmSettlement::SettledCompatible {
                VerificationAlarmTerminalization::Confirmed
            } else {
                VerificationAlarmTerminalization::Unconfirmed
            },
            state.secondary_cause,
        ))
    }

    fn set_alarm_settlement(
        live: &LiveJob,
        settlement: AlarmSettlement,
        cause: Option<KnownVerificationSecondaryCauseCode>,
    ) {
        let mut state = lock_live_state(&live.state);
        transition_alarm_state(&mut state, settlement, cause);
    }

    async fn retain_settled_owner(&self, job_id: CounterfactualJobId, live: &LiveJob) {
        {
            let mut state = lock_live_state(&live.state);
            state.derived = None;
        }
        let mut jobs = self.live.lock().await;
        if let Some(owner) = jobs.get_mut(&job_id)
            && Arc::ptr_eq(&owner.state, &live.state)
        {
            owner.cancellation = None;
        }
    }

    fn publish_live(
        &self,
        live: &LiveJob,
        request: &CounterfactualJobRequestV1,
        run_id: Option<SolveRunId>,
        phase: CounterfactualProgressPhase,
    ) {
        let mut state = lock_live_state(&live.state);
        if state.alarm.is_some() || state.settlement != AlarmSettlement::Active {
            return;
        }
        let timestamp = self.clock.now();
        let owner_created_at = state.owner.as_ref().map(|owner| owner.created_at);
        retain_publication_wall(&mut state, owner_created_at, timestamp);
        self.publish_at(request, run_id, phase, timestamp);
    }

    fn publish_terminal_live(
        &self,
        record: &CounterfactualJobRecordV1,
        run_id: Option<SolveRunId>,
        live: &LiveJob,
    ) {
        let mut state = lock_live_state(&live.state);
        if state.alarm.is_some() || state.settlement != AlarmSettlement::Active {
            return;
        }
        let timestamp = self.clock.now();
        let owner_created_at = state.owner.as_ref().map(|owner| owner.created_at);
        retain_publication_wall(&mut state, owner_created_at, timestamp);
        self.publish_terminal_at(record, run_id, timestamp);
    }

    fn publish_confirmed_alarm(
        &self,
        record: &CounterfactualJobRecordV1,
        run_id: SolveRunId,
        live: &LiveJob,
    ) {
        let state = lock_live_state(&live.state);
        if state.settlement != AlarmSettlement::SettledCompatible || state.alarm.is_none() {
            return;
        }
        if let Some(finished_at) = record.finished_at {
            self.publish_terminal_at(record, Some(run_id), finished_at);
        }
    }

    fn publish_terminal(&self, record: &CounterfactualJobRecordV1, run_id: Option<SolveRunId>) {
        self.publish_terminal_at(record, run_id, self.clock.now());
    }

    fn publish_terminal_at(
        &self,
        record: &CounterfactualJobRecordV1,
        run_id: Option<SolveRunId>,
        timestamp: Rfc3339Timestamp,
    ) {
        let phase = match record.state {
            CounterfactualJobState::Completed => CounterfactualProgressPhase::Completed,
            CounterfactualJobState::Failed => CounterfactualProgressPhase::Failed,
            CounterfactualJobState::Cancelled => CounterfactualProgressPhase::Cancelled,
            CounterfactualJobState::Interrupted => CounterfactualProgressPhase::Interrupted,
            CounterfactualJobState::Queued | CounterfactualJobState::Running => return,
        };
        self.publish_at(&record.request, run_id, phase, timestamp);
    }

    fn publish(
        &self,
        request: &CounterfactualJobRequestV1,
        run_id: Option<SolveRunId>,
        phase: CounterfactualProgressPhase,
    ) {
        self.publish_at(request, run_id, phase, self.clock.now());
    }

    fn publish_at(
        &self,
        request: &CounterfactualJobRequestV1,
        run_id: Option<SolveRunId>,
        phase: CounterfactualProgressPhase,
        timestamp: Rfc3339Timestamp,
    ) {
        let _ = self.events.send(AppEvent {
            topic: EventTopic::CounterfactualProgress,
            payload: EventPayload::CounterfactualProgress {
                context: EventContext {
                    event_version: 1,
                    timestamp,
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
    input: &'a eutheto_domain_ir::RunInput,
    terminal: TerminalMaterial,
    prepared_evidence: Option<BTreeMap<eutheto_domain_ir::DomainEvidenceId, VerificationValue>>,
}

fn prepare_finalization(
    context: FinalizationContext<'_>,
) -> Result<CounterfactualRunFinalizationV1, StoreError> {
    let FinalizationContext {
        request,
        base,
        compilation,
        input,
        terminal,
        prepared_evidence,
    } = context;
    let (manifest, alternative, intended, inert_infeasibility) = terminal.into_parts();
    let infeasibility = inert_infeasibility
        .map(|inert| {
            inert.bind(input, &manifest).map_err(|_| {
                StoreError::InvalidPersistedRun(
                    "infeasibility evidence binding is invalid".to_owned(),
                )
            })
        })
        .transpose()?
        .flatten();
    let interpretation = interpret_counterfactual(
        request,
        &base.portable.run_input,
        &base.portable.run_manifest,
        &base.portable.accepted_result,
        compilation,
        input,
        &manifest,
        alternative.as_ref(),
        infeasibility.as_ref(),
    );
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
            CounterfactualRunFinalizationV1::CompletedNoResult {
                manifest,
                result,
                infeasibility_evidence: infeasibility.map(Box::new),
            }
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
    Ok(finalization)
}

fn cancellation_observation(
    record: &CounterfactualJobRecordV1,
) -> Result<Option<CounterfactualCancellationObservationV1>, StoreError> {
    match (record.cancel_request_id, record.cancel_requested_at) {
        (None, None) => Ok(None),
        (Some(cancel_request_id), Some(cancel_requested_at)) => {
            Ok(Some(CounterfactualCancellationObservationV1 {
                cancel_request_id,
                cancel_requested_at,
            }))
        }
        _ => Err(StoreError::InvalidPersistedRun(
            "counterfactual cancellation marker is only partially present".to_owned(),
        )),
    }
}

fn cancellation_precedes_or_ties(
    cancellation: &CounterfactualCancellationObservationV1,
    observed_at: Rfc3339Timestamp,
) -> bool {
    cancellation.cancel_requested_at.as_timestamp() <= observed_at.as_timestamp()
}

fn retime_manifest(
    manifest: RunManifest,
    outcome: RunTerminalOutcomeV1,
    elapsed: DurationMillis,
    finished_at: Rfc3339Timestamp,
    evidence_preparation_elapsed: DurationMillis,
    preserve_verified_timing: bool,
) -> Result<RunManifest, StoreError> {
    if manifest
        .elapsed_milliseconds
        .is_some_and(|previous| elapsed < previous)
        || finished_at.as_timestamp() < manifest.finished_at.as_timestamp()
    {
        return Err(StoreError::InvalidPersistedRun(
            "causal finalization retime moved backwards".to_owned(),
        ));
    }
    let mut phase_timings = manifest.phase_timings;
    phase_timings.evidence_persistence_milliseconds = Some(evidence_preparation_elapsed);
    RunManifest::new(
        manifest.run_id,
        manifest.run_input_checksum,
        outcome,
        manifest.started_at,
        finished_at,
        Some(elapsed),
        manifest.first_incumbent_milliseconds,
        preserve_verified_timing
            .then_some(manifest.first_verified_feasible_milliseconds)
            .flatten(),
        phase_timings,
        manifest.verification_warnings,
    )
    .and_then(|retimed| retimed.with_native_observations(manifest.native_observations))
    .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))
}

// Rebind every terminal variant once to the same actual cancellation observation.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn retime_stale_finalization(
    finalization: CounterfactualRunFinalizationV1,
    request: &CounterfactualJobRequestV1,
    base: &StoredAcceptedResultV2,
    compilation: &CounterfactualCompilationBindingV1,
    input: &eutheto_domain_ir::RunInput,
    elapsed: DurationMillis,
    finished_at: Rfc3339Timestamp,
    expired: bool,
    evidence_preparation_elapsed: DurationMillis,
    cancellation: &CounterfactualCancellationObservationV1,
) -> Result<CounterfactualRunFinalizationV1, StoreError> {
    match finalization {
        CounterfactualRunFinalizationV1::CompletedNoResult {
            manifest,
            result: _,
            infeasibility_evidence,
        } => {
            let Some(evidence) = infeasibility_evidence.map(|evidence| *evidence) else {
                let manifest = retime_manifest(
                    manifest,
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::Cancelled,
                    },
                    elapsed,
                    finished_at,
                    evidence_preparation_elapsed,
                    false,
                )?;
                return Ok(CounterfactualRunFinalizationV1::Cancelled { manifest });
            };
            if cancellation_precedes_or_ties(cancellation, evidence.original.observed_at) {
                let manifest = retime_manifest(
                    manifest,
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::Cancelled,
                    },
                    elapsed,
                    finished_at,
                    evidence_preparation_elapsed,
                    false,
                )?;
                return Ok(CounterfactualRunFinalizationV1::Cancelled { manifest });
            }
            if expired
                && !matches!(
                    &manifest.outcome,
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::Infeasible
                    }
                )
            {
                return Err(StoreError::CounterfactualTransitionConflict(request.job_id));
            }
            let inert = InertInfeasibilityEvidence {
                original: evidence.original,
                condition: evidence.condition,
                diagnostic: evidence.diagnostic,
                infeasibility: evidence.infeasibility,
            };
            let manifest = retime_manifest(
                manifest,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible,
                },
                elapsed,
                finished_at,
                evidence_preparation_elapsed,
                false,
            )?;
            prepare_finalization(FinalizationContext {
                request,
                base,
                compilation,
                input,
                terminal: TerminalMaterial {
                    manifest,
                    alternative: None,
                    intended: IntendedTerminal::NoResult,
                    infeasibility: Some(inert),
                    original_observation_late: false,
                },
                prepared_evidence: None,
            })
        }
        CounterfactualRunFinalizationV1::Quarantined {
            manifest,
            candidate_diagnostics,
            error,
        } => {
            let outcome = manifest.outcome.clone();
            let manifest = retime_manifest(
                manifest,
                outcome,
                elapsed,
                finished_at,
                evidence_preparation_elapsed,
                false,
            )?;
            Ok(CounterfactualRunFinalizationV1::Quarantined {
                manifest,
                candidate_diagnostics,
                error,
            })
        }
        CounterfactualRunFinalizationV1::Cancelled { manifest }
        | CounterfactualRunFinalizationV1::Interrupted { manifest }
        | CounterfactualRunFinalizationV1::Failed { manifest, .. }
        | CounterfactualRunFinalizationV1::CompletedAccepted { manifest, .. } => {
            let manifest = retime_manifest(
                manifest,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Cancelled,
                },
                elapsed,
                finished_at,
                evidence_preparation_elapsed,
                false,
            )?;
            Ok(CounterfactualRunFinalizationV1::Cancelled { manifest })
        }
    }
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
    manifest: RunManifest,
    alternative: Option<AcceptedResult>,
    intended: IntendedTerminal,
    infeasibility: Option<InertInfeasibilityEvidence>,
    original_observation_late: bool,
}

impl TerminalMaterial {
    fn into_parts(
        self,
    ) -> (
        RunManifest,
        Option<AcceptedResult>,
        IntendedTerminal,
        Option<InertInfeasibilityEvidence>,
    ) {
        (
            self.manifest,
            self.alternative,
            self.intended,
            self.infeasibility,
        )
    }

    fn retime(
        self,
        elapsed: DurationMillis,
        finished_at: Rfc3339Timestamp,
        expired: bool,
        evidence_preparation_elapsed: DurationMillis,
    ) -> Result<Self, StoreError> {
        let preserves_original_infeasibility = self.infeasibility.is_some()
            && matches!(&self.intended, IntendedTerminal::NoResult)
            && matches!(
                &self.manifest.outcome,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible
                }
            );
        let force_limit = (expired || self.original_observation_late)
            && matches!(
                &self.intended,
                IntendedTerminal::Accepted | IntendedTerminal::NoResult
            )
            && !preserves_original_infeasibility;
        let outcome = if force_limit {
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::NoSolutionWithinLimit,
            }
        } else {
            self.manifest.outcome.clone()
        };
        let mut phase_timings = self.manifest.phase_timings;
        phase_timings.evidence_persistence_milliseconds = Some(evidence_preparation_elapsed);
        let manifest = RunManifest::new(
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
        .and_then(|retimed| retimed.with_native_observations(self.manifest.native_observations))
        .map_err(|error| StoreError::InvalidPersistedRun(error.to_string()))?;
        Ok(Self {
            manifest,
            alternative: if force_limit { None } else { self.alternative },
            intended: if force_limit {
                IntendedTerminal::NoResult
            } else {
                self.intended
            },
            infeasibility: if force_limit {
                None
            } else {
                self.infeasibility
            },
            original_observation_late: self.original_observation_late,
        })
    }
}

struct TerminalBuildContext<'a> {
    record: &'a RouterExecutionRecord,
    decision: Option<&'a AcceptanceDecision>,
    review_timings: Option<eutheto_verify::AcceptancePhaseTimings>,
    identity: &'a BackendRuntimeIdentity,
    options: &'a eutheto_types::SolveOptions,
    observations: &'a RouterObservations,
    compile_elapsed: DurationMillis,
    elapsed_total: DurationMillis,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    input: &'a eutheto_domain_ir::RunInput,
    condition: &'a CounterfactualConditionV1,
    infeasibility: Option<InertInfeasibilityEvidence>,
    diagnostic_alarm: Option<CorrectnessAlarm>,
    optional_explanation_elapsed: Option<DurationMillis>,
    original_observation_late: bool,
}

fn build_terminal(context: TerminalBuildContext<'_>) -> Result<TerminalMaterial, StoreError> {
    let TerminalBuildContext {
        record,
        decision,
        review_timings,
        identity,
        options,
        observations,
        compile_elapsed,
        elapsed_total,
        started_at,
        finished_at,
        input,
        condition,
        infeasibility,
        diagnostic_alarm,
        optional_explanation_elapsed,
        original_observation_late,
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
    let mut phase_timings =
        terminal_phase_timings(review_timings, compile_elapsed, backend_elapsed);
    phase_timings.optional_explanation_milliseconds = optional_explanation_elapsed;
    let (mut outcome, mut alternative, mut intended, verified_timing) = select_terminal(
        record,
        decision,
        identity,
        options,
        condition,
        attempt.is_some_and(|attempt| attempt.outcome.is_some()),
        first_verified,
    );
    if let Some(alarm) = diagnostic_alarm {
        outcome = RunTerminalOutcomeV1::VerificationAlarm {
            diagnostic_code: alarm.diagnostic_code,
        };
        alternative = None;
        intended = IntendedTerminal::Quarantined;
    }
    let manifest = RunManifest::new(
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
        infeasibility,
        original_observation_late,
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
    input: &eutheto_domain_ir::RunInput,
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

fn alarm_manifest(
    input: &eutheto_domain_ir::RunInput,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    elapsed: DurationMillis,
    diagnostic_code: &str,
) -> Result<RunManifest, StoreError> {
    RunManifest::new(
        input.run_id,
        input.checksum.clone(),
        RunTerminalOutcomeV1::VerificationAlarm {
            diagnostic_code: diagnostic_code.to_owned(),
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

fn cancelled_manifest(
    input: &eutheto_domain_ir::RunInput,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    elapsed: DurationMillis,
) -> Result<RunManifest, StoreError> {
    RunManifest::new(
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
    input: &eutheto_domain_ir::RunInput,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
) -> Result<RunManifest, StoreError> {
    RunManifest::new(
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
    input: &eutheto_domain_ir::RunInput,
    started_at: Rfc3339Timestamp,
    finished_at: Rfc3339Timestamp,
    elapsed: DurationMillis,
    kind: CounterfactualFailureKind,
) -> Result<RunManifest, StoreError> {
    let status = match kind {
        CounterfactualFailureKind::BackendUnavailable => SolveStatus::BackendUnavailable,
        CounterfactualFailureKind::InvalidModel => SolveStatus::InvalidModel,
        _ => SolveStatus::BackendFailed,
    };
    RunManifest::new(
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

pub(crate) fn validate_counterfactual_source(
    request: &SolutionStartCounterfactualRequestV1,
    base: &StoredAcceptedResultV2,
) -> Result<CounterfactualConditionV1, AppError> {
    let condition = validate_start_request(request)?;
    validate_base_request(request, base)?;
    validate_replacement_source(&condition, base)?;
    Ok(condition)
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

fn validate_replacement_source(
    condition: &CounterfactualConditionV1,
    base: &StoredAcceptedResultV2,
) -> Result<(), AppError> {
    let CounterfactualConditionPayloadV1::ReplaceAssignment {
        target_assignment_id,
        replaced_assignment_id,
    } = &condition.condition
    else {
        return Ok(());
    };
    let assignments = &base.portable.accepted_result.solution.assignments;
    let selected = |id| {
        assignments
            .iter()
            .find(|assignment| &assignment.id == id)
            .and_then(|assignment| match assignment.value {
                AssignmentValue::Boolean(value) => Some(value),
                _ => None,
            })
    };
    if selected(target_assignment_id) != Some(false)
        || selected(replaced_assignment_id) != Some(true)
    {
        return Err(invalid_request(
            "solution.counterfactual_replacement_invalid",
            "/condition",
            "Select an unassigned candidate to replace an assigned person in the saved result.",
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

fn retain_publication_wall(
    state: &mut LiveJobState,
    owner_created_at: Option<Rfc3339Timestamp>,
    observed: Rfc3339Timestamp,
) {
    if owner_created_at.is_some_and(|created| observed >= created)
        && state
            .trusted_wall
            .is_none_or(|previous| observed >= previous)
    {
        state.trusted_wall = Some(observed);
    }
}

fn has_alarm(live: &LiveJob) -> bool {
    lock_live_state(&live.state).alarm.is_some()
}

fn owned_alarm_error(live: &LiveJob) -> Option<AppError> {
    let state = lock_live_state(&live.state);
    let alarm = state.alarm.as_ref()?;
    Some(known_alarm_error(
        alarm,
        if state.settlement == AlarmSettlement::SettledCompatible {
            VerificationAlarmTerminalization::Confirmed
        } else {
            VerificationAlarmTerminalization::Unconfirmed
        },
        state.secondary_cause,
    ))
}

fn known_alarm_error(
    alarm: &OwnedAlarm,
    terminalization: VerificationAlarmTerminalization,
    secondary_cause: Option<KnownVerificationSecondaryCauseCode>,
) -> AppError {
    if let Some(context) = &alarm.public_context {
        return AppError::Verification(VerificationFailure::from_known_alarm(
            context
                .clone()
                .with_terminalization(terminalization)
                .with_secondary_cause(secondary_cause),
            None,
        ));
    }
    AppError::Verification(VerificationFailure {
        code: "verification.alarm_custody_unavailable".to_owned(),
        message: VerificationFailure::KNOWN_ALARM_MESSAGE.to_owned(),
        retryable: false,
        diagnostic_id: None,
        known_alarm: None,
    })
}

fn rekey_live_owner_map(
    jobs: &mut BTreeMap<CounterfactualJobId, LiveJob>,
    job_id: CounterfactualJobId,
    live: &LiveJob,
    request: &CounterfactualJobRequestV1,
) -> Option<LiveJob> {
    let current_present = jobs
        .values()
        .any(|current| Arc::ptr_eq(&current.state, &live.state));
    if !current_present
        || !lock_live_state(&live.state)
            .owner
            .as_ref()
            .is_some_and(|owner| same_logical_request(owner, request))
    {
        return None;
    }
    if let Some(existing) = jobs.get(&job_id).cloned() {
        if Arc::ptr_eq(&existing.state, &live.state) {
            lock_live_state(&live.state).owner = Some(request.clone());
            jobs.retain(|key, current| *key == job_id || !Arc::ptr_eq(&current.state, &live.state));
            return Some(existing);
        }
        if lock_live_state(&existing.state).owner.as_ref() != Some(request) {
            return None;
        }
        let current = lock_live_state(&live.state);
        if current.claim != LiveClaimState::Available
            || current.alarm.is_some()
            || current.settlement != AlarmSettlement::Active
        {
            return None;
        }
        drop(current);
        jobs.retain(|_, current| !Arc::ptr_eq(&current.state, &live.state));
        return Some(existing);
    }
    lock_live_state(&live.state).owner = Some(request.clone());
    jobs.retain(|_, current| !Arc::ptr_eq(&current.state, &live.state));
    jobs.insert(job_id, live.clone());
    Some(live.clone())
}

fn same_logical_request(
    left: &CounterfactualJobRequestV1,
    right: &CounterfactualJobRequestV1,
) -> bool {
    left.schema_version == right.schema_version
        && left.request_id == right.request_id
        && left.semantics == right.semantics
        && left.condition == right.condition
        && left.request_hash == right.request_hash
}

fn cancel_owner_matches(
    owner: &CounterfactualJobRequestV1,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    job_id: CounterfactualJobId,
) -> bool {
    owner.job_id == job_id
        && owner.semantics.scenario_id == scenario_id
        && owner.semantics.scenario_revision == expected_revision.value()
}

fn counts_toward_native_owner_limit(live: &LiveJob) -> bool {
    lock_live_state(&live.state).settlement != AlarmSettlement::SettledCompatible
}

fn trusted_alarm_timing_sample(
    elapsed_sample: Option<Duration>,
    trusted_elapsed: Option<Duration>,
    finished_at: Rfc3339Timestamp,
    started_at: Rfc3339Timestamp,
    trusted_wall: Option<Rfc3339Timestamp>,
    cancellation: Option<&CounterfactualCancellationObservationV1>,
) -> Option<DurationMillis> {
    if finished_at < started_at
        || trusted_wall.is_some_and(|previous| finished_at < previous)
        || cancellation.is_some_and(|value| finished_at < value.cancel_requested_at)
    {
        return None;
    }
    let elapsed = elapsed_sample?;
    if elapsed.as_millis() > u128::from(DurationMillis::MAX.value())
        || trusted_elapsed.is_some_and(|previous| elapsed < previous)
    {
        return None;
    }
    observed_milliseconds(elapsed).ok()
}

fn claim_live_state(state: &mut LiveJobState) -> Option<ClaimDisposition> {
    if state.alarm.is_some() || state.settlement != AlarmSettlement::Active {
        return None;
    }
    match state.claim {
        LiveClaimState::Available => {
            state.claim = LiveClaimState::Starting;
            Some(ClaimDisposition::New)
        }
        LiveClaimState::Starting => Some(ClaimDisposition::Recover),
        LiveClaimState::Spawned => Some(ClaimDisposition::AlreadySpawned),
    }
}

fn release_live_claim(live: &LiveJob) {
    let mut state = lock_live_state(&live.state);
    if state.settlement == AlarmSettlement::Active && state.alarm.is_none() {
        state.claim = LiveClaimState::Available;
        state.startup_budget = None;
    }
}

fn retain_startup_budget(live: &LiveJob, budget: &ParentSolveBudget) {
    lock_live_state(&live.state).startup_budget = Some(budget.clone());
}

fn startup_budget_for_recovery(live: &LiveJob) -> Option<ParentSolveBudget> {
    lock_live_state(&live.state).startup_budget.clone()
}

fn mark_live_claim_spawned(live: &LiveJob) {
    let mut state = lock_live_state(&live.state);
    if state.claim == LiveClaimState::Starting {
        state.claim = LiveClaimState::Spawned;
        state.startup_budget = None;
    }
}

fn transition_alarm_state(
    state: &mut LiveJobState,
    settlement: AlarmSettlement,
    cause: Option<KnownVerificationSecondaryCauseCode>,
) {
    state.settlement = settlement;
    state.derived = None;
    if let Some(cause) = cause {
        state.secondary_cause.get_or_insert(cause);
    }
}
fn is_definite_no_start(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::ScenarioNotFound(_)
            | StoreError::Conflict { .. }
            | StoreError::IdentityCollision(_)
            | StoreError::InvalidPersistedRun(_)
            | StoreError::InvalidPersistedResult(_)
            | StoreError::AcceptedResultNotFound(_)
            | StoreError::AcceptedResultChanged(_)
            | StoreError::InvalidPersistedDiagnostic(_)
            | StoreError::CounterfactualRequestIdConflict { .. }
            | StoreError::CounterfactualJobCollision(_)
            | StoreError::InvalidPersistedCounterfactual(_)
            | StoreError::CounterfactualCapacityExceeded { .. }
    )
}

fn schema_error(version: u32) -> AppError {
    invalid_request(
        "solution.counterfactual_schema_unsupported",
        "/schemaVersion",
        &format!("Unsupported counterfactual schema version {version}."),
    )
}

fn counterfactual_cancel_binding_error() -> AppError {
    invalid_request(
        "solution.counterfactual_cancel_binding",
        "/request",
        "The cancellation request does not match the immutable counterfactual job binding.",
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
        StoreError::AcceptedResultChanged(_) => {
            crate::application::solution_verification_mismatch()
        }
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
        AssignmentValue, ConflictUnavailableReason, CounterfactualConditionPayloadV1,
        DomainAssignmentId, InfeasibilityEvidenceV1, OriginalInfeasibilityObservationV1,
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

    #[test]
    fn alarm_timing_requires_monotonic_trusted_bounds_and_accepts_later_recovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let started = test_timestamp("2026-01-01T00:00:00.100Z")?;
        let trusted_wall = test_timestamp("2026-01-01T00:00:00.200Z")?;
        let cancellation = CounterfactualCancellationObservationV1 {
            cancel_request_id: "018f47f2-e880-7000-8000-000000000013".parse()?,
            cancel_requested_at: test_timestamp("2026-01-01T00:00:00.250Z")?,
        };
        let finished = test_timestamp("2026-01-01T00:00:00.300Z")?;

        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(30)),
                Some(Duration::from_millis(25)),
                finished,
                started,
                Some(trusted_wall),
                Some(&cancellation),
            ),
            Some(DurationMillis::new(30)?)
        );
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(24)),
                Some(Duration::from_millis(25)),
                finished,
                started,
                Some(trusted_wall),
                Some(&cancellation),
            ),
            None
        );
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(30)),
                Some(Duration::from_millis(25)),
                test_timestamp("2026-01-01T00:00:00.249Z")?,
                started,
                Some(trusted_wall),
                Some(&cancellation),
            ),
            None
        );
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(30)),
                None,
                test_timestamp("2026-01-01T00:00:00.199Z")?,
                started,
                Some(trusted_wall),
                None,
            ),
            None
        );
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(30)),
                None,
                finished,
                test_timestamp("2026-01-01T00:00:00.301Z")?,
                None,
                None,
            ),
            None
        );
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_secs(u64::MAX)),
                None,
                finished,
                started,
                None,
                None,
            ),
            None
        );
        assert_eq!(
            trusted_alarm_timing_sample(None, None, finished, started, None, None,),
            None
        );
        // An invalid candidate sample is not itself a retained lower bound.
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_secs(1)),
                None,
                test_timestamp("2026-01-01T00:00:00.500Z")?,
                started,
                None,
                None,
            ),
            Some(DurationMillis::new(1_000)?)
        );
        Ok(())
    }

    #[test]
    fn pre_run_publication_wall_bound_survives_derived_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let created = test_timestamp("2026-01-01T00:00:00Z")?;
        let started = test_timestamp("2026-01-01T00:00:00.050Z")?;
        let compiling = test_timestamp("2026-01-01T00:00:00.200Z")?;
        let mut state = LiveJobState::default();

        retain_publication_wall(&mut state, Some(created), compiling);
        retain_publication_wall(&mut state, Some(created), started);
        assert_eq!(state.trusted_wall, Some(compiling));
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(80)),
                None,
                test_timestamp("2026-01-01T00:00:00.100Z")?,
                started,
                state.trusted_wall,
                None,
            ),
            None
        );
        assert_eq!(
            trusted_alarm_timing_sample(
                Some(Duration::from_millis(250)),
                None,
                test_timestamp("2026-01-01T00:00:00.300Z")?,
                started,
                state.trusted_wall,
                None,
            ),
            Some(DurationMillis::new(250)?)
        );
        Ok(())
    }

    #[test]
    fn uncertain_reused_request_rekeys_its_provisional_owner_to_durable_job()
    -> Result<(), Box<dyn std::error::Error>> {
        let request_id = "018f47f2-e880-7000-8000-000000000001".parse()?;
        let scenario_id = "018f47f2-e880-7000-8000-000000000002".parse()?;
        let condition = CounterfactualConditionV1::new(
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.assignment")?,
                value: AssignmentValue::Boolean(true),
            },
        )?;
        let checksum = "ab".repeat(32);
        let make_request =
            |job_id: CounterfactualJobId,
             created_at: Rfc3339Timestamp|
             -> Result<CounterfactualJobRequestV1, Box<dyn std::error::Error>> {
                let semantics = CounterfactualRequestSemanticsV1 {
                    schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
                    scenario_id,
                    scenario_revision: 1,
                    snapshot_id: "018f47f2-e880-7000-8000-000000000004"
                        .parse::<eutheto_types::ScenarioSnapshotId>()?,
                    snapshot_document_hash: checksum.clone(),
                    base: AcceptedResultRefV1 {
                        solution_id: "018f47f2-e880-7000-8000-000000000003".parse()?,
                        result_checksum: checksum.clone(),
                    },
                    base_run_id: "018f47f2-e880-7000-8000-000000000005".parse()?,
                    base_run_input_checksum: checksum.clone(),
                    base_model_hash: checksum.clone(),
                    objective_policy_hash: checksum.clone(),
                    condition_checksum: condition.checksum.clone(),
                    total_budget_milliseconds: DurationMillis::new(1_000)?,
                };
                Ok(CounterfactualJobRequestV1::new(
                    job_id,
                    request_id,
                    semantics,
                    condition.clone(),
                    created_at,
                )?)
            };
        let provisional_id = "018f47f2-e880-7000-8000-000000000010".parse()?;
        let durable_id = "018f47f2-e880-7000-8000-000000000011".parse()?;
        let provisional = make_request(provisional_id, test_timestamp("2026-01-01T00:00:00Z")?)?;
        let durable = make_request(durable_id, test_timestamp("2026-01-01T00:00:00.010Z")?)?;
        assert!(cancel_owner_matches(
            &durable,
            scenario_id,
            Revision::new(1),
            durable_id
        ));
        assert!(!cancel_owner_matches(
            &durable,
            scenario_id,
            Revision::new(2),
            durable_id
        ));
        let live = LiveJob {
            cancellation: None,
            startup: Arc::new(Mutex::new(())),
            state: Arc::new(StdMutex::new(LiveJobState {
                owner: Some(provisional),
                ..LiveJobState::default()
            })),
        };
        let mut jobs = BTreeMap::from([(provisional_id, live.clone())]);

        let rekeyed = rekey_live_owner_map(&mut jobs, durable_id, &live, &durable)
            .ok_or("exact durable request did not rekey")?;
        assert!(Arc::ptr_eq(&rekeyed.state, &live.state));
        assert_eq!(jobs.len(), 1);
        assert!(!jobs.contains_key(&provisional_id));
        assert!(jobs.contains_key(&durable_id));
        assert_eq!(lock_live_state(&live.state).owner.as_ref(), Some(&durable));
        Ok(())
    }

    #[test]
    fn alarm_owner_capacity_tracks_unresolved_and_compatible_settlement() {
        let live = LiveJob {
            cancellation: None,
            startup: Arc::new(Mutex::new(())),
            state: Arc::new(StdMutex::new(LiveJobState::default())),
        };
        let spawned = LiveJob {
            cancellation: None,
            startup: Arc::new(Mutex::new(())),
            state: Arc::new(StdMutex::new(LiveJobState::default())),
        };
        {
            let mut state = lock_live_state(&spawned.state);
            assert_eq!(claim_live_state(&mut state), Some(ClaimDisposition::New));
        }
        mark_live_claim_spawned(&spawned);
        {
            let mut state = lock_live_state(&spawned.state);
            assert_eq!(
                claim_live_state(&mut state),
                Some(ClaimDisposition::AlreadySpawned)
            );
        }
        assert!(counts_toward_native_owner_limit(&live));
        {
            let mut state = lock_live_state(&live.state);
            assert_eq!(claim_live_state(&mut state), Some(ClaimDisposition::New));
        }
        release_live_claim(&live);
        {
            let mut state = lock_live_state(&live.state);
            assert_eq!(claim_live_state(&mut state), Some(ClaimDisposition::New));
            assert_eq!(
                claim_live_state(&mut state),
                Some(ClaimDisposition::Recover)
            );
            transition_alarm_state(
                &mut state,
                AlarmSettlement::SettledUnresolved,
                Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed),
            );
            assert_eq!(claim_live_state(&mut state), None);
        }
        assert!(counts_toward_native_owner_limit(&live));
        {
            let mut state = lock_live_state(&live.state);
            transition_alarm_state(&mut state, AlarmSettlement::SettledCompatible, None);
            assert_eq!(
                state.secondary_cause,
                Some(KnownVerificationSecondaryCauseCode::StorageOperationFailed)
            );
        }
        assert!(!counts_toward_native_owner_limit(&live));
    }

    // Exercise one retained alarm through execution retirement and all publication handles.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn diagnostic_alarm_handle_survives_execution_and_owner_retirement()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let directory_path = directory.path().canonicalize()?;
        let (store, _) = SqliteScenarioStore::open(directory_path.join("custody.sqlite")).await?;
        let packs = Arc::new(
            DomainPackRegistry::builder()
                .register(eutheto_workforce::WorkforcePack)
                .build()?,
        );
        let solvers = Arc::new(SolverRegistry::production()?);
        let (events, _) = broadcast::channel(1);
        let runtime = CounterfactualRuntime::new(
            Arc::new(store),
            packs,
            solvers,
            CounterfactualRuntimeServices {
                clock: Arc::new(eutheto_types::SystemClock),
                ids: Arc::new(eutheto_types::SystemIdGenerator),
                cancellation: CancellationToken::new(),
                events,
            },
        );
        let scenario_id = "018f47f2-e880-7000-8000-000000000002".parse()?;
        let job_id = "018f47f2-e880-7000-8000-000000000010".parse()?;
        let run_id = "018f47f2-e880-7000-8000-000000000011".parse()?;
        let checksum = "ab".repeat(32);
        let condition = CounterfactualConditionV1::new(
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: DomainAssignmentId::new("tests.assignment")?,
                value: AssignmentValue::Boolean(true),
            },
        )?;
        let owner = CounterfactualJobRequestV1::new(
            job_id,
            "018f47f2-e880-7000-8000-000000000001".parse()?,
            CounterfactualRequestSemanticsV1 {
                schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
                scenario_id,
                scenario_revision: 1,
                snapshot_id: "018f47f2-e880-7000-8000-000000000004".parse()?,
                snapshot_document_hash: checksum.clone(),
                base: AcceptedResultRefV1 {
                    solution_id: "018f47f2-e880-7000-8000-000000000003".parse()?,
                    result_checksum: checksum.clone(),
                },
                base_run_id: "018f47f2-e880-7000-8000-000000000005".parse()?,
                base_run_input_checksum: checksum.clone(),
                base_model_hash: checksum.clone(),
                objective_policy_hash: checksum.clone(),
                condition_checksum: condition.checksum.clone(),
                total_budget_milliseconds: DurationMillis::new(1_000)?,
            },
            condition,
            test_timestamp("2026-01-01T00:00:00Z")?,
        )?;
        let code = "verification.required_rule_rejected";
        let live = LiveJob {
            cancellation: None,
            startup: Arc::new(Mutex::new(())),
            state: Arc::new(StdMutex::new(LiveJobState {
                owner: Some(owner),
                alarm: Some(OwnedAlarm {
                    run_id,
                    input_checksum: checksum.clone(),
                    diagnostic_code: code.to_owned(),
                    public_context: Some(KnownVerificationAlarmV1::new(
                        run_id,
                        &checksum,
                        code,
                        VerificationAlarmTerminalization::Unconfirmed,
                        None,
                    )?),
                }),
                ..LiveJobState::default()
            })),
        };
        runtime.live.lock().await.insert(job_id, live.clone());
        CounterfactualRuntime::set_alarm_settlement(
            &live,
            AlarmSettlement::SettledUnresolved,
            None,
        );
        let handle = runtime
            .diagnostic_run_handle(scenario_id, Revision::new(1), run_id)
            .await
            .ok_or("settled native alarm lost its diagnostic handle")?;
        let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
        let before = handle
            .recipient_retained_bytes(&control)
            .map_err(|error| format!("{error:?}"))?;
        let extra = {
            let mut state = lock_live_state(&live.state);
            let hash = &mut state
                .owner
                .as_mut()
                .ok_or("missing native owner")?
                .semantics
                .snapshot_document_hash;
            let previous = hash.capacity();
            hash.reserve(1024 * 1024);
            hash.capacity() - previous
        };
        assert_eq!(
            handle
                .recipient_retained_bytes(&control)
                .map_err(|error| format!("{error:?}"))?,
            before + 2 * extra,
        );
        assert!(
            runtime
                .diagnostic_run_handle(scenario_id, Revision::new(2), run_id)
                .await
                .is_none()
        );
        assert!(
            runtime
                .diagnostic_run_handle(
                    "018f47f2-e880-7000-8000-000000000099".parse()?,
                    Revision::new(1),
                    run_id,
                )
                .await
                .is_none()
        );
        runtime.retain_settled_owner(job_id, &live).await;
        runtime.live.lock().await.clear();
        for (settlement, expected) in [
            (
                AlarmSettlement::SettledUnresolved,
                VerificationAlarmTerminalization::Unconfirmed,
            ),
            (
                AlarmSettlement::SettledCompatible,
                VerificationAlarmTerminalization::Confirmed,
            ),
        ] {
            CounterfactualRuntime::set_alarm_settlement(&live, settlement, None);
            let Err(AppError::Verification(failure)) = handle.admit() else {
                return Err("retained native alarm became publishable".into());
            };
            let known = failure
                .known_alarm
                .as_ref()
                .ok_or("native alarm metadata is missing")?;
            assert_eq!(known.run_id(), run_id);
            assert_eq!(known.input_checksum(), checksum);
            assert_eq!(known.terminalization(), expected);
        }
        Ok(())
    }

    #[test]
    fn startup_recovery_keeps_the_original_parent_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(ReturningClock(AtomicU64::new(100)));
        let budget = ParentSolveBudget::new(
            DurationMillis::new(10)?,
            clock.clone(),
            CancellationToken::default(),
        )?;
        let live = LiveJob {
            cancellation: None,
            startup: Arc::new(Mutex::new(())),
            state: Arc::new(StdMutex::new(LiveJobState::default())),
        };
        {
            let mut state = lock_live_state(&live.state);
            assert_eq!(claim_live_state(&mut state), Some(ClaimDisposition::New));
        }
        retain_startup_budget(&live, &budget);
        clock.0.store(111, Ordering::Relaxed);
        {
            let mut state = lock_live_state(&live.state);
            assert_eq!(
                claim_live_state(&mut state),
                Some(ClaimDisposition::Recover)
            );
        }
        let recovered = startup_budget_for_recovery(&live).ok_or("missing original budget")?;
        assert!(recovered.is_expired());
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
            alarm: CorrectnessAlarm {
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

    fn test_timestamp(value: &str) -> Result<Rfc3339Timestamp, Box<dyn std::error::Error>> {
        Ok(value.parse()?)
    }

    fn test_manifest(
        outcome: RunTerminalOutcomeV1,
        elapsed: u64,
        finished_at: &str,
    ) -> Result<RunManifest, Box<dyn std::error::Error>> {
        Ok(RunManifest::new(
            "018f47f2-e880-7000-8000-000000000010".parse()?,
            "a".repeat(64),
            outcome,
            test_timestamp("2026-01-01T00:00:00Z")?,
            test_timestamp(finished_at)?,
            Some(DurationMillis::new(elapsed)?),
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )?)
    }

    #[test]
    fn cancellation_before_tie_and_after_original_observation_are_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let observed = test_timestamp("2026-01-01T00:00:01Z")?;
        let before = CounterfactualCancellationObservationV1 {
            cancel_request_id: "018f47f2-e880-7000-8000-000000000011".parse()?,
            cancel_requested_at: test_timestamp("2026-01-01T00:00:00.999Z")?,
        };
        let tie = CounterfactualCancellationObservationV1 {
            cancel_request_id: "018f47f2-e880-7000-8000-000000000012".parse()?,
            cancel_requested_at: observed,
        };
        let after = CounterfactualCancellationObservationV1 {
            cancel_request_id: "018f47f2-e880-7000-8000-000000000013".parse()?,
            cancel_requested_at: test_timestamp("2026-01-01T00:00:01.001Z")?,
        };
        assert!(cancellation_precedes_or_ties(&before, observed));
        assert!(cancellation_precedes_or_ties(&tie, observed));
        assert!(!cancellation_precedes_or_ties(&after, observed));
        Ok(())
    }

    #[test]
    fn terminal_retime_keeps_timely_original_proof_and_sticky_alarm_but_not_late_original()
    -> Result<(), Box<dyn std::error::Error>> {
        let inert = InertInfeasibilityEvidence {
            original: OriginalInfeasibilityObservationV1 {
                run_input_checksum: "a".repeat(64),
                model_hash: "b".repeat(64),
                worker_manifest_sha256: None,
                observed_at: test_timestamp("2026-01-01T00:00:00.500Z")?,
                elapsed_milliseconds: DurationMillis::new(5)?,
            },
            condition: None,
            diagnostic: None,
            infeasibility: InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::NotRequested,
            },
        };
        let retained = TerminalMaterial {
            manifest: test_manifest(
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible,
                },
                10,
                "2026-01-01T00:00:00.010Z",
            )?,
            alternative: None,
            intended: IntendedTerminal::NoResult,
            infeasibility: Some(inert),
            original_observation_late: false,
        }
        .retime(
            DurationMillis::new(50)?,
            test_timestamp("2026-01-01T00:00:02Z")?,
            true,
            DurationMillis::ZERO,
        )?;
        assert!(matches!(
            retained.manifest.outcome,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible
            }
        ));
        assert!(retained.infeasibility.is_some());

        let late = TerminalMaterial {
            manifest: test_manifest(
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible,
                },
                10,
                "2026-01-01T00:00:00.010Z",
            )?,
            alternative: None,
            intended: IntendedTerminal::NoResult,
            infeasibility: None,
            original_observation_late: true,
        }
        .retime(
            DurationMillis::new(50)?,
            test_timestamp("2026-01-01T00:00:02Z")?,
            false,
            DurationMillis::ZERO,
        )?;
        assert!(matches!(
            late.manifest.outcome,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::NoSolutionWithinLimit
            }
        ));
        assert!(late.infeasibility.is_none());

        let alarm = TerminalMaterial {
            manifest: test_manifest(
                RunTerminalOutcomeV1::VerificationAlarm {
                    diagnostic_code: "verification.native_alarm".to_owned(),
                },
                10,
                "2026-01-01T00:00:00.010Z",
            )?,
            alternative: None,
            intended: IntendedTerminal::Quarantined,
            infeasibility: None,
            original_observation_late: false,
        }
        .retime(
            DurationMillis::new(50)?,
            test_timestamp("2026-01-01T00:00:02Z")?,
            true,
            DurationMillis::ZERO,
        )?;
        assert!(matches!(
            alarm.manifest.outcome,
            RunTerminalOutcomeV1::VerificationAlarm { .. }
        ));
        Ok(())
    }

    #[test]
    fn causal_retime_refreshes_manifest_and_rejects_backdating()
    -> Result<(), Box<dyn std::error::Error>> {
        let manifest = test_manifest(
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible,
            },
            10,
            "2026-01-01T00:00:00.010Z",
        )?;
        let refreshed = retime_manifest(
            manifest.clone(),
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible,
            },
            DurationMillis::new(20)?,
            test_timestamp("2026-01-01T00:00:00.020Z")?,
            DurationMillis::new(3)?,
            false,
        )?;
        assert_eq!(
            refreshed.elapsed_milliseconds,
            Some(DurationMillis::new(20)?)
        );
        assert_ne!(refreshed.checksum, manifest.checksum);
        assert!(matches!(
            retime_manifest(
                manifest,
                RunTerminalOutcomeV1::NoResult {
                    status: SolveStatus::Infeasible,
                },
                DurationMillis::new(9)?,
                test_timestamp("2026-01-01T00:00:00.009Z")?,
                DurationMillis::ZERO,
                false,
            ),
            Err(StoreError::InvalidPersistedRun(_))
        ));
        Ok(())
    }
}
