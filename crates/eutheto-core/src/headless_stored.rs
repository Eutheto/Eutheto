use super::super::super::{
    EuthetoApp, export_error, operation_interrupted, solution_contract_error, store_error,
};
use super::{
    HeadlessCompilationReport, HeadlessService, HeadlessSolveOutcome, PreparedSolve,
    SolvePreparationFailure, interruption_status, milliseconds, no_result, observed_elapsed,
    solve_error, timing_error,
};
use eutheto_domain_ir::{
    PortableAcceptedResultV2, RunInputV1, RunManifestV1, RunPhaseTimingsV1, RunTerminalOutcomeV1,
};
use eutheto_export::prepare_json_atomic_controlled;
use eutheto_solver_api::ProgressSink;
use eutheto_solver_router::{RouterExecutionRecord, RoutingDecision};
use eutheto_store::{
    CandidateDiagnosticsV1, ExistingSolveRunV1, NewSolveRunV1, SqliteScenarioStore, StoreError,
};
use eutheto_types::{
    AppError, BackendSelection, CancellationToken, Clock, OperationControl, ParentSolveBudget,
    RequestId, Revision, Rfc3339Timestamp, ScenarioId, ScenarioSnapshotV1, SolutionId,
    SolveOptions, SolveRunId, SolveStatus,
};
use std::path::Path;
use std::sync::Arc;

/// Caller-owned retry semantics. These are Rust service inputs, not a new wire format.
#[derive(Clone, Debug)]
pub struct StoredSolveRequest {
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub options: SolveOptions,
}

#[derive(Debug)]
pub enum StoredSolveFailure {
    /// No run was started or acquired by this invocation.
    BeforeStart(AppError),
    Preparation(Box<SolvePreparationFailure>),
    /// This invocation started the run and attempted one terminal cleanup after execution failed.
    AfterStart {
        run_id: SolveRunId,
        error: AppError,
        cleanup: Result<Box<RunManifestV1>, AppError>,
    },
    /// A primary finalization failed or its commit outcome is unknown. Never blindly retry it.
    FinalizationUnconfirmed {
        run_id: SolveRunId,
        result_id: Option<SolutionId>,
        error: AppError,
    },
}

/// Observations exist only when this call actually performed preparation and execution.
#[derive(Debug)]
pub struct StoredSolveReport {
    pub compilation: HeadlessCompilationReport,
    pub routing: RoutingDecision,
    pub execution: RouterExecutionRecord,
}

/// Retained authority and optional observations. Secondary output cannot undo this state.
///
/// The stored result identity cannot be relabeled before secondary publication:
/// ```compile_fail
/// let _cannot_relabel: fn(&mut eutheto_core::StoredSolveOutcome) =
///     |outcome| { let _ = &mut outcome.state; };
/// ```
#[derive(Debug)]
pub struct StoredSolveOutcome {
    state: ExistingSolveRunV1,
    pub reused: bool,
    pub report: Option<StoredSolveReport>,
    pub warning: Option<AppError>,
    // Only a freshly loaded store-owned portable record may be published after the primary commit.
    portable: Option<PortableAcceptedResultV2>,
}

/// An accepted database result remains committed even when this secondary publication fails.
#[derive(Debug)]
pub struct StoredResultPublication {
    pub result_id: SolutionId,
    pub publication: Result<(), AppError>,
}

impl StoredSolveOutcome {
    /// Retained database authority is read-only, including the committed result identity.
    #[must_use]
    pub fn state(&self) -> &ExistingSolveRunV1 {
        &self.state
    }

    #[must_use]
    pub fn accepted_result_id(&self) -> Option<SolutionId> {
        match self
            .state
            .manifest
            .as_ref()
            .map(|manifest| &manifest.outcome)
        {
            Some(RunTerminalOutcomeV1::Accepted { solution_id, .. }) => Some(*solution_id),
            _ => None,
        }
    }

    #[must_use]
    pub fn portable_result(&self) -> Option<&PortableAcceptedResultV2> {
        self.portable.as_ref()
    }

    /// Publishes only the reloaded durable result, never an in-memory precommit candidate.
    /// Cancellation or output failure is a warning-bearing secondary outcome with the result ID.
    #[must_use]
    pub fn publish_result(
        &self,
        destination: &Path,
        cancellation: &CancellationToken,
    ) -> Option<StoredResultPublication> {
        let result_id = self.accepted_result_id()?;
        let publication = (|| {
            let portable = self.portable.as_ref().ok_or_else(result_reload_warning)?;
            let control = OperationControl::Cancellation(cancellation.clone());
            let prepared = prepare_json_atomic_controlled(destination, portable, &control)
                .map_err(|error| export_error(&error))?;
            prepared
                .publish_controlled(&control)
                .map_err(|error| export_error(&error))
        })();
        Some(StoredResultPublication {
            result_id,
            publication,
        })
    }
}

impl EuthetoApp {
    /// Reuses this application's exact registries and injected clocks without opening another DB.
    #[must_use]
    pub fn headless_service(&self) -> HeadlessService {
        HeadlessService {
            packs: Arc::clone(&self.pack_registry),
            solvers: Arc::clone(&self.solver_registry),
            clock: Arc::clone(&self.clock),
            monotonic_clock: Arc::clone(&self.monotonic_clock),
            ids: Arc::clone(&self.ids),
        }
    }

    /// Owns one finite ordinary solve. The application's injected cancellation token governs it.
    /// A matching request returns existing state before clocks, IDs, current revision or routing.
    /// Dropping this future after ownership is acquired requires the existing interrupted recovery.
    ///
    /// # Errors
    /// Distinguishes preparation, owned-run cleanup and unconfirmed database finalization.
    // The finite owner keeps idempotent lookup and every post-start terminalization path together.
    #[allow(clippy::too_many_lines)]
    pub async fn solve_stored(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        request.options.validate().map_err(|_| {
            StoredSolveFailure::BeforeStart(solve_error(
                "solve.options_invalid",
                "Solve options are invalid.",
            ))
        })?;
        if let Some(existing) = self
            .store
            .load_solve_run_by_request(request.request_id)
            .await
            .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
        {
            validate_retry(&request, &existing).map_err(StoredSolveFailure::BeforeStart)?;
            return Ok(load_committed_output(&self.store, existing, true, None).await);
        }
        let operation = self
            .headless_service()
            .begin_solve(
                request.request_id,
                request.options.clone(),
                self.cancellation.child(),
            )
            .map_err(StoredSolveFailure::BeforeStart)?;
        let project = self
            .store
            .get_project(request.scenario_id)
            .await
            .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?;
        if project.summary.revision != request.expected_revision {
            return Err(StoredSolveFailure::BeforeStart(store_error(
                StoreError::Conflict {
                    expected: request.expected_revision,
                    actual: project.summary.revision,
                },
            )));
        }
        let mut snapshot = ScenarioSnapshotV1::current(
            project.summary.revision,
            project.document,
            project.portable.required_capabilities,
        );
        snapshot.semantic_extensions = project.portable.semantic_extensions;
        snapshot.extensions = project.portable.extensions;
        let prepared = operation
            .prepare_captured(snapshot, None)
            .map_err(StoredSolveFailure::Preparation)?;
        prepared
            .operation
            .control()
            .check()
            .map_err(|reason| StoredSolveFailure::BeforeStart(operation_interrupted(reason)))?;
        let started = match self.store.start_solve_run(prepared.store_request()).await {
            Ok(started) => started,
            Err(error @ StoreError::SolveRequestIdConflict { .. }) => {
                // A concurrent owner may have bound Auto under a different current runtime.
                // Reconcile the caller's original semantics by reading; do not reroute or retry start.
                if let Some(existing) = self
                    .store
                    .load_solve_run_by_request(request.request_id)
                    .await
                    .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
                {
                    validate_retry(&request, &existing).map_err(StoredSolveFailure::BeforeStart)?;
                    return Ok(load_committed_output(&self.store, existing, true, None).await);
                }
                return Err(StoredSolveFailure::BeforeStart(store_error(error)));
            }
            Err(error) => return Err(StoredSolveFailure::BeforeStart(store_error(error))),
        };
        if started.reused {
            let existing = self
                .store
                .load_solve_run_by_request(request.request_id)
                .await
                .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
                .ok_or_else(|| StoredSolveFailure::BeforeStart(solution_contract_error()))?;
            validate_retry(&request, &existing).map_err(StoredSolveFailure::BeforeStart)?;
            return Ok(load_committed_output(&self.store, existing, true, None).await);
        }
        // Only this branch owns finalization. All post-start execution errors enter cleanup.
        let owner = SolveOwner {
            input: started.input.clone(),
            budget: prepared.operation.budget.clone(),
            started_at: prepared.operation.started_at,
            clock: Arc::clone(&self.clock),
        };
        let execution = async {
            if started.started_at != prepared.operation.started_at {
                return Err(solution_contract_error());
            }
            let loaded = self
                .store
                .load_solve_input(started.input.run_id)
                .await
                .map_err(store_error)?;
            if loaded.input != started.input || loaded.document != prepared.snapshot.document {
                return Err(solution_contract_error());
            }
            let mut outcome = prepared.execute_bound(loaded.input, progress).await?;
            outcome.refresh_admission()?;
            Ok(outcome)
        }
        .await;
        let outcome = match execution {
            Ok(outcome) => outcome,
            Err(error) => {
                let cleanup = owner.cleanup(&self.store).await.map(Box::new);
                return Err(StoredSolveFailure::AfterStart {
                    run_id: owner.input.run_id,
                    error,
                    cleanup,
                });
            }
        };
        let HeadlessSolveOutcome {
            run_input,
            manifest,
            compilation,
            routing,
            execution,
            accepted,
            evidence,
            ..
        } = outcome;
        let result_id = accepted.as_ref().map(|result| result.solution.solution_id);
        let report = Some(StoredSolveReport {
            compilation,
            routing,
            execution,
        });
        let finalization = match accepted {
            Some(accepted) => {
                self.store
                    .finalize_accepted_run(accepted, manifest.clone(), evidence)
                    .await
            }
            None if matches!(
                manifest.outcome,
                RunTerminalOutcomeV1::VerificationAlarm { .. }
            ) =>
            {
                self.store
                    .finalize_quarantined_run(manifest.clone(), CandidateDiagnosticsV1::default())
                    .await
            }
            None => self.store.finalize_terminal_run(manifest.clone()).await,
        };
        if let Err(error) = finalization {
            if matches!(error, StoreError::SolveRunTerminalConflict(_))
                && let Ok(Some(existing)) = self
                    .store
                    .load_solve_run_by_request(run_input.request_id)
                    .await
                && existing.input == run_input
                && existing.manifest.is_some()
            {
                return Ok(load_committed_output(&self.store, existing, false, report).await);
            }
            if known_precommit_rejection(&error) {
                let cleanup = owner.cleanup(&self.store).await.map(Box::new);
                return Err(StoredSolveFailure::AfterStart {
                    run_id: run_input.run_id,
                    error: store_error(error),
                    cleanup,
                });
            }
            return Err(StoredSolveFailure::FinalizationUnconfirmed {
                run_id: run_input.run_id,
                result_id,
                error: store_error(error),
            });
        }
        let state = ExistingSolveRunV1 {
            input: run_input,
            started_at: manifest.started_at,
            manifest: Some(manifest),
        };
        Ok(load_committed_output(&self.store, state, false, report).await)
    }
}

fn known_precommit_rejection(error: &StoreError) -> bool {
    if matches!(
        error,
        StoreError::InvalidPersistedRun(_)
            | StoreError::InvalidPersistedResult(_)
            | StoreError::InvalidPersistedDiagnostic(_)
            | StoreError::SnapshotMismatch(_)
            | StoreError::InvalidScenarioIdentity(_)
            | StoreError::IdentityCollision(_)
            | StoreError::NumericRange
            | StoreError::Json(_)
    ) {
        return true;
    }
    #[cfg(debug_assertions)]
    if matches!(error, StoreError::InjectedFailure) {
        return true;
    }
    false
}

impl PreparedSolve {
    fn store_request(&self) -> NewSolveRunV1 {
        NewSolveRunV1 {
            run_id: self.operation.run_id,
            request_id: self.operation.request_id,
            scenario_id: self.snapshot.document.scenario_id,
            expected_revision: self.snapshot.revision,
            planning_ir_schema_version: self.problem.schema_version,
            compiler_version: self.problem.metadata.compiler_version.clone(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            backend_id: self.identity.backend_id().clone(),
            backend_version: self.identity.backend_version().to_owned(),
            adapter_version: self.identity.adapter_version().to_owned(),
            worker_version: self.identity.worker_version().to_owned(),
            solver_version: self.identity.solver_version().to_owned(),
            protocol_major: self.identity.protocol_major(),
            protocol_minor: self.identity.protocol_minor(),
            model_hash: self.report.summary.canonical_ir_hash.clone(),
            objective_policy_hash: self.objective_policy_hash.clone(),
            solve_options: self.operation.options.clone(),
            temporary_condition_hash: None,
            started_at: self.operation.started_at,
        }
    }
}

struct SolveOwner {
    input: RunInputV1,
    budget: ParentSolveBudget,
    started_at: Rfc3339Timestamp,
    clock: Arc<dyn Clock>,
}

impl SolveOwner {
    async fn cleanup(&self, store: &SqliteScenarioStore) -> Result<RunManifestV1, AppError> {
        let interruption = OperationControl::Solve(self.budget.phase_view())
            .check()
            .err()
            .map(interruption_status);
        let finished_at = self.clock.now();
        let elapsed = observed_elapsed(&self.budget).and_then(milliseconds).ok();
        let outcome = if elapsed.is_some() {
            no_result(interruption.unwrap_or(SolveStatus::BackendFailed))
        } else {
            // An incomplete run with unusable timing cannot invent a measured terminal duration.
            RunTerminalOutcomeV1::Interrupted
        };
        let manifest = RunManifestV1::new(
            self.input.run_id,
            self.input.checksum.clone(),
            outcome,
            self.started_at,
            finished_at,
            elapsed,
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )
        .map_err(|_| timing_error())?;
        store
            .finalize_terminal_run(manifest.clone())
            .await
            .map_err(store_error)?;
        Ok(manifest)
    }
}

fn validate_retry(
    request: &StoredSolveRequest,
    existing: &ExistingSolveRunV1,
) -> Result<(), AppError> {
    let mut options = request.options.clone();
    if options.backend == BackendSelection::Auto {
        options.backend = BackendSelection::Specific(existing.input.backend_id.clone());
    }
    if existing.input.request_id != request.request_id
        || existing.input.scenario_id != request.scenario_id
        || existing.input.scenario_revision != request.expected_revision.value()
        || existing.input.temporary_condition_hash.is_some()
        || existing.input.solve_options != options
    {
        return Err(store_error(StoreError::SolveRequestIdConflict {
            request_id: request.request_id,
        }));
    }
    Ok(())
}

async fn load_committed_output(
    store: &SqliteScenarioStore,
    state: ExistingSolveRunV1,
    reused: bool,
    report: Option<StoredSolveReport>,
) -> StoredSolveOutcome {
    let mut outcome = StoredSolveOutcome {
        state,
        reused,
        report,
        warning: None,
        portable: None,
    };
    if outcome.accepted_result_id().is_none()
        && let Some(report) = &mut outcome.report
    {
        report.execution.selected_candidate = None;
    }
    if let Some(result_id) = outcome.accepted_result_id() {
        match store.load_accepted_result(result_id).await {
            Ok(stored)
                if stored.portable.run_input == outcome.state.input
                    && Some(&stored.portable.run_manifest) == outcome.state.manifest.as_ref() =>
            {
                outcome.portable = Some(stored.portable);
            }
            Ok(_) | Err(_) => outcome.warning = Some(result_reload_warning()),
        }
    }
    outcome
}

fn result_reload_warning() -> AppError {
    solve_error(
        "solve.committed_result_unavailable",
        "The result is committed, but its retained portable output could not be loaded. Retry output by result identity without rerunning the solve.",
    )
}
