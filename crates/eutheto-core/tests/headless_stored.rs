#![forbid(unsafe_code)]

#[path = "support/private_storage.rs"]
mod private_storage;

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, EuthetoApp,
    SolveEstimateModelRequestV1, StoredBaselineSourceV1, StoredObjectiveSelectionV1,
    StoredSolveFailure, StoredSolveRequest,
};
use eutheto_domain_api::{CompileContext, DomainPackRegistry};
use eutheto_domain_ir::{
    AUTHORED_OBJECTIVE_PROFILE_ID, ObjectiveProfileId, RunManifest, RunObjectiveOperation,
    RunPhaseTimingsV1, RunTerminalOutcomeV1,
};
use eutheto_planning_ir::{
    PlanningIrLimitsV1, PlanningProblemSummary, canonical_ir_hash, canonical_objective_policy_hash,
};
use eutheto_solver_api::*;
use eutheto_store::{
    InitializationOutcome, NewProject, NewSolveRunV1, SolveRunListRequestV1, SqliteScenarioStore,
    StartedSolveRunV1,
};
use eutheto_types::{
    ActorRef, AppError, BackendId, BackendSelection, CancellationToken, Clock, CommandEnvelope,
    CommandSource, DomainCommandEnvelope, DurationMillis, ExplanationMode, FixedClock,
    FixedIdGenerator, FixedMonotonicClock, MonotonicClock, OperationControl, ParentSolveBudget,
    PreservationPolicy, ReproducibilityMode, ResourceLimits, Revision, Rfc3339Timestamp,
    ScenarioCommand, ScenarioDocument, SolveMode, SolveOptions, SolveStatus, SystemClock,
    WorkerThreadPolicy,
};
use eutheto_workforce::{WorkforcePack, commands};
use serde_json::json;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::Debug,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use workforce_fixture::id;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const BACKEND: &str = "tests.headless-failure";
const NOW: &str = "2026-09-07T12:00:00Z";

fn boxed(error: impl Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

struct Fixture {
    directory: TempDir,
    store: Arc<SqliteScenarioStore>,
    initialization: InitializationOutcome,
    packs: Arc<DomainPackRegistry>,
    document: ScenarioDocument,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        Self::new_with_external_plan(false).await
    }

    async fn new_with_external_plan(include_external: bool) -> TestResult<Self> {
        let directory = private_storage::tempdir("eutheto-headless-stored-")?;
        let (store, initialization) =
            SqliteScenarioStore::open(directory.path().join("eutheto.sqlite")).await?;
        let mut document = workforce_fixture::fixture()?;
        document.domain.entities.remove(&id(6).parse()?);
        document.domain.locked_assignments.clear();
        document.domain.rules.insert(
            id(30).parse()?,
            json!({
                "id": id(30), "kind": "coverage", "active": true, "strength": "required",
                "scope": {"people": {"kind": "all"}}
            }),
        );
        if include_external {
            let mut person = document.domain.entities[&id(1).parse()?].clone();
            person["id"] = json!(id(12));
            person["name"] = json!("Sky");
            person["externalId"] = json!("staff-02");
            document.domain.entities.insert(id(12).parse()?, person);
            document.domain.entities.insert(
                id(50).parse()?,
                json!({
                    "kind": "externalStartingPlan", "schemaVersion": 1,
                    "id": id(50), "name": "Unverified starting plan",
                    "assignments": [{"personId": id(1), "shiftId": id(8)}],
                    "originalImport": {
                        "sourceBlake3": "a".repeat(64), "pairContentHash": "b".repeat(64)
                    }
                }),
            );
        }
        store
            .create_project(NewProject {
                document: document.clone(),
            })
            .await?;
        Ok(Self {
            directory,
            store: Arc::new(store),
            initialization,
            packs: Arc::new(
                DomainPackRegistry::builder()
                    .register(WorkforcePack)
                    .build()?,
            ),
            document,
        })
    }

    fn dependencies(&self) -> TestResult<AppDependencies> {
        Ok(AppDependencies {
            paths: AppPaths {
                database: self.directory.path().join("eutheto.sqlite"),
                safety_backups: self.directory.path().join("backups"),
            },
            clock: Arc::new(FixedClock::new(NOW.parse()?)),
            monotonic_clock: Arc::new(FixedMonotonicClock::default()),
            ids: Arc::new(FixedIdGenerator::new([id(400).parse()?])),
            cancellation: CancellationToken::default(),
        })
    }

    fn app(&self, registry: SolverRegistry, dependencies: AppDependencies) -> EuthetoApp {
        EuthetoApp::from_initialized_store_with_registries(
            Arc::clone(&self.store),
            self.initialization.clone(),
            dependencies,
            Arc::clone(&self.packs),
            Arc::new(registry),
        )
    }

    fn request(&self) -> TestResult<StoredSolveRequest> {
        Ok(StoredSolveRequest {
            request_id: id(300).parse()?,
            scenario_id: self.document.scenario_id,
            expected_revision: Some(Revision::INITIAL),
            options: options()?,
            objective_selection: None,
        })
    }

    fn seed_run(&self, request: &StoredSolveRequest) -> TestResult<NewSolveRunV1> {
        let expected_revision = request
            .expected_revision
            .ok_or("seed requires an explicit revision")?;
        let context = CompileContext {
            objective_context: None,
            scenario_revision: expected_revision.value(),
            semantic_metadata: BTreeMap::new(),
            control: OperationControl::Cancellation(CancellationToken::default()),
            planning_limits: PlanningIrLimitsV1::DEFAULT,
        };
        let problem = self
            .packs
            .require(&self.document.domain_pack.id)?
            .compile(&self.document, &context)?;
        let mut bound_options = request.options.clone();
        bound_options.backend = BackendSelection::Specific(BackendId::new(BACKEND)?);
        Ok(NewSolveRunV1 {
            objective_context: None,
            run_id: id(400).parse()?,
            request_id: request.request_id,
            scenario_id: request.scenario_id,
            expected_revision,
            planning_ir_schema_version: problem.schema_version,
            compiler_version: problem.metadata.compiler_version.clone(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            backend_id: BackendId::new(BACKEND)?,
            backend_version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            worker_version: "1.0.0".to_owned(),
            solver_version: "1.0.0".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            model_hash: canonical_ir_hash(&problem, PlanningIrLimitsV1::DEFAULT)?,
            objective_policy_hash: canonical_objective_policy_hash(
                &problem,
                PlanningIrLimitsV1::DEFAULT,
            )?,
            solve_options: bound_options,
            temporary_condition_hash: None,
            started_at: NOW.parse()?,
        })
    }
    async fn start_seeded(&self, mut seeded: NewSolveRunV1) -> TestResult<StartedSolveRunV1> {
        let project = self.store.get_project(seeded.scenario_id).await?;
        let (capture, _) = self
            .store
            .capture_solve_snapshot(
                seeded.scenario_id,
                seeded.expected_revision,
                project.document,
                SystemClock.now(),
                None,
                OperationControl::Cancellation(CancellationToken::new()),
                None,
            )
            .await?;
        // Recovery uses real wall time; admission must follow the captured snapshot.
        seeded.started_at = SystemClock.now();
        self.store
            .start_solve_run(
                seeded,
                capture.stamp,
                Some(capture.snapshot_id),
                OperationControl::Cancellation(CancellationToken::new()),
            )
            .await
            .map_err(Into::into)
    }

    async fn advance_revision(&self) -> TestResult {
        let app = self.app(SolverRegistry::production()?, self.dependencies()?);
        let mut entity = self.document.domain.entities[&id(1).parse()?].clone();
        entity["name"] = json!("River at the newer revision");
        app.execute(AppCommand::ApplyScenario {
            request_id: id(301).parse()?,
            envelope: CommandEnvelope {
                command_id: id(200).parse()?,
                scenario_id: self.document.scenario_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Stored solve test".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity": entity}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
        assert_eq!(
            self.store
                .get_project(self.document.scenario_id)
                .await?
                .summary
                .revision,
            Revision::new(1)
        );
        Ok(())
    }

    async fn change_external_plan(
        &self,
        app: &EuthetoApp,
        revision: Revision,
        remove: bool,
    ) -> TestResult<Revision> {
        let entity_id: eutheto_types::EntityId = id(50).parse()?;
        let (command_type, payload) = if remove {
            (commands::REMOVE_ENTITY, json!({"entityId": entity_id}))
        } else {
            let mut entity = self
                .store
                .get_project(self.document.scenario_id)
                .await?
                .document
                .domain
                .entities[&entity_id]
                .clone();
            entity["assignments"] = json!([{"personId": id(12), "shiftId": id(8)}]);
            (commands::UPDATE_ENTITY, json!({"entity": entity}))
        };
        let AppCommandResult::ScenarioCommand(result) = app
            .execute(AppCommand::ApplyScenario {
                request_id: id(310 + u32::try_from(revision.value())?).parse()?,
                envelope: CommandEnvelope {
                    command_id: id(210 + u32::try_from(revision.value())?).parse()?,
                    scenario_id: self.document.scenario_id,
                    expected_revision: revision,
                    actor: ActorRef {
                        actor_id: None,
                        display_name: "External source capture regression".to_owned(),
                    },
                    source: CommandSource::System,
                    command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                        command_type: command_type.to_owned(),
                        payload,
                    }),
                },
                truncate_redo: false,
            })
            .await
            .map_err(boxed)?
        else {
            return Err("external edit omitted receipt".into());
        };
        Ok(result.new_revision)
    }
}

#[tokio::test]
async fn stored_workforce_model_estimate_compiles_without_backend_or_durable_run() -> TestResult {
    let fixture = Fixture::new().await?;
    let app = fixture.app(SolverRegistry::production()?, fixture.dependencies()?);
    let request = SolveEstimateModelRequestV1 {
        scenario_id: fixture.document.scenario_id,
        expected_revision: Revision::INITIAL,
    };
    let budget = || -> TestResult<ParentSolveBudget> {
        Ok(ParentSolveBudget::new(
            DurationMillis::new(30_000)?,
            Arc::new(FixedMonotonicClock::default()),
            CancellationToken::new(),
        )?)
    };
    let result = app
        .estimate_stored_model(request, budget()?)
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(result.scenario_id, fixture.document.scenario_id);
    assert_eq!(result.scenario_revision, Revision::INITIAL);
    assert_eq!(result.current_revision, Revision::INITIAL);
    assert!(!result.stale);
    assert!(result.compiled);
    assert!(!result.solved);
    assert_eq!(result.candidate_count, Some(1));
    assert!(
        fixture
            .store
            .list_solve_runs(SolveRunListRequestV1 {
                scenario_id: fixture.document.scenario_id,
                scenario_revision: None,
                limit: 10,
            })
            .await?
            .is_empty()
    );

    fixture.advance_revision().await?;
    assert!(matches!(
        app.estimate_stored_model(request, budget()?).await,
        Err(AppError::Conflict {
            expected_revision: Revision::INITIAL,
            actual_revision,
        }) if actual_revision == Revision::new(1)
    ));
    assert!(
        fixture
            .store
            .list_solve_runs(SolveRunListRequestV1 {
                scenario_id: fixture.document.scenario_id,
                scenario_revision: None,
                limit: 10,
            })
            .await?
            .is_empty()
    );
    Ok(())
}

fn options() -> TestResult<SolveOptions> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Custom,
        time_limit_milliseconds: DurationMillis::new(10_000)?,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 7,
        solution_limit: Some(1),
        stop_after_first_feasible: true,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::None,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 100,
            max_rules: 100,
            max_variables: 10_000,
            max_constraints: 10_000,
        },
    })
}

struct Progress;
impl ProgressSink for Progress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}

struct ForbiddenClock {
    wall: Rfc3339Timestamp,
    reads: AtomicU32,
}
impl Clock for ForbiddenClock {
    fn now(&self) -> Rfc3339Timestamp {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.wall
    }
}
impl MonotonicClock for ForbiddenClock {
    fn now(&self) -> Duration {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Duration::ZERO
    }
}

#[tokio::test]
async fn running_and_terminal_retries_preserve_original_authority_before_current_runtime_checks()
-> TestResult {
    let fixture = Fixture::new().await?;
    let request = fixture.request()?;
    let seeded = fixture.seed_run(&request)?;
    let started = fixture.start_seeded(seeded).await?;
    fixture.advance_revision().await?;
    let mut dependencies = fixture.dependencies()?;
    let clock = Arc::new(ForbiddenClock {
        wall: NOW.parse()?,
        reads: AtomicU32::new(0),
    });
    dependencies.clock = clock.clone();
    dependencies.monotonic_clock = clock.clone();
    dependencies.ids = Arc::new(FixedIdGenerator::new([]));
    dependencies.cancellation.cancel();
    // No backend is available and no identity or clock may be acquired by this application.
    let app = fixture.app(SolverRegistry::production()?, dependencies);
    let running = Box::pin(app.solve_stored(request.clone(), &mut Progress))
        .await
        .map_err(boxed)?;
    assert!(running.reused);
    assert_eq!(running.state().input, started.input);
    assert!(running.state().manifest.is_none());
    assert!(running.report.is_none());
    assert!(running.portable_result().is_none());
    assert!(running.accepted_result_id().is_none());
    let frozen = fixture.store.load_solve_input(started.input.run_id).await?;
    assert_eq!(frozen.document, fixture.document);
    let mut capture_request = request.clone();
    capture_request.expected_revision = None;
    let captured_retry = Box::pin(app.solve_stored(capture_request.clone(), &mut Progress))
        .await
        .map_err(boxed)?;
    assert_eq!(captured_retry.state(), running.state());

    let mut changed_options = request.clone();
    changed_options.options.random_seed += 1;
    assert!(matches!(
        Box::pin(app.solve_stored(changed_options.clone(), &mut Progress)).await,
        Err(StoredSolveFailure::BeforeStart(AppError::Validation(_)))
    ));
    assert_eq!(
        &fixture
            .store
            .load_solve_run_by_request(request.request_id)
            .await?
            .ok_or("missing running request")?,
        running.state()
    );

    let manifest = RunManifest::new(
        started.input.run_id,
        started.input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendFailed,
        },
        started.started_at,
        started.started_at,
        Some(DurationMillis::ZERO),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    fixture
        .store
        .finalize_terminal_run(manifest.clone(), None)
        .await?;
    let terminal = Box::pin(app.solve_stored(request.clone(), &mut Progress))
        .await
        .map_err(boxed)?;
    assert!(terminal.reused);
    assert_eq!(terminal.state().input, started.input);
    assert_eq!(terminal.state().manifest.as_ref(), Some(&manifest));
    assert!(terminal.report.is_none());
    assert!(terminal.portable_result().is_none());
    assert!(terminal.accepted_result_id().is_none());
    let captured_terminal = Box::pin(app.solve_stored(capture_request, &mut Progress))
        .await
        .map_err(boxed)?;
    assert_eq!(captured_terminal.state(), terminal.state());
    assert!(matches!(
        Box::pin(app.solve_stored(changed_options, &mut Progress)).await,
        Err(StoredSolveFailure::BeforeStart(AppError::Validation(_)))
    ));
    assert_eq!(
        &fixture
            .store
            .load_solve_run_by_request(request.request_id)
            .await?
            .ok_or("missing terminal request")?,
        terminal.state()
    );
    assert!(
        fixture
            .store
            .list_accepted_results(request.scenario_id)
            .await?
            .is_empty()
    );
    assert_eq!(clock.reads.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn native_retries_inherit_only_the_recorded_implicit_explanation_policy() -> TestResult {
    for terminal in [false, true] {
        let fixture = Fixture::new().await?;
        let legacy = fixture.request()?;
        let seeded = fixture.seed_run(&legacy)?;
        let started = fixture.start_seeded(seeded).await?;
        if terminal {
            fixture
                .store
                .finalize_terminal_run(
                    RunManifest::new(
                        started.input.run_id,
                        started.input.checksum.clone(),
                        RunTerminalOutcomeV1::NoResult {
                            status: SolveStatus::BackendFailed,
                        },
                        started.started_at,
                        started.started_at,
                        Some(DurationMillis::ZERO),
                        None,
                        None,
                        RunPhaseTimingsV1::default(),
                        Vec::new(),
                    )?,
                    None,
                )
                .await?;
        }
        fixture.advance_revision().await?;
        let app = fixture.app(SolverRegistry::production()?, fixture.dependencies()?);
        let mut current = legacy.clone();
        current.options.explanation_mode = ExplanationMode::Standard;
        // Fully specified Core callers still cannot change a recorded option on replay.
        assert!(matches!(
            app.solve_stored(current.clone(), &mut Progress).await,
            Err(StoredSolveFailure::BeforeStart(AppError::Validation(_)))
        ));
        let mut native = current.clone();
        let retained = app
            .solve_lookup_native_request(&mut native)
            .await
            .map_err(boxed)?
            .ok_or("native replay lost its retained run")?;
        assert_eq!(retained.input, started.input);
        assert_eq!(retained.manifest.is_some(), terminal);
        assert_eq!(native.options.explanation_mode, ExplanationMode::None);
        assert_eq!(native.options, legacy.options);

        let mut new_request = current.clone();
        new_request.request_id = id(600).parse()?;
        assert!(
            app.solve_lookup_native_request(&mut new_request)
                .await
                .map_err(boxed)?
                .is_none()
        );
        assert_eq!(new_request.options, current.options);

        let mut wrong_mode = current.clone();
        wrong_mode.options.mode = SolveMode::Deep;
        let mut wrong_limit = current.clone();
        wrong_limit.options.time_limit_milliseconds = DurationMillis::new(10_001)?;
        let mut wrong_source = current.clone();
        wrong_source.scenario_id = id(601).parse()?;
        let mut wrong_revision = current.clone();
        wrong_revision.expected_revision = Some(Revision::new(1));
        for mut changed in [wrong_mode, wrong_limit, wrong_source, wrong_revision] {
            let submitted_options = changed.options.clone();
            assert!(matches!(
                app.solve_lookup_native_request(&mut changed).await,
                Err(AppError::Validation(_))
            ));
            assert_eq!(changed.options, submitted_options);
        }
        assert_eq!(
            fixture
                .store
                .load_solve_run_by_request(legacy.request_id)
                .await?
                .ok_or("replay changed durable request ownership")?,
            retained
        );
    }
    Ok(())
}

// This backend can only fail. It exercises dispatch/finalization, never accepted-result authority.
struct FailingBackend {
    descriptor: SolverDescriptor,
    runtime: BackendRuntimeIdentity,
    matrix: CapabilityMatrix,
    cancellation: Option<CancellationToken>,
    invocations: AtomicU32,
    expire_backend_clock: Option<Arc<FixedMonotonicClock>>,
}

impl SolverBackend for FailingBackend {
    fn completion_reserve(&self) -> DurationMillis {
        DurationMillis::ZERO
    }

    fn descriptor(&self) -> &SolverDescriptor {
        &self.descriptor
    }
    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        &self.runtime
    }

    fn verified_worker_manifest_sha256(&self) -> Option<&[u8; 32]> {
        None
    }
    fn compatibility(
        &self,
        summary: &PlanningProblemSummary,
        options: &SolveOptions,
    ) -> CompatibilityReport {
        compatibility_for(&self.matrix, &self.descriptor.id, summary, options).unwrap_or_else(
            |_| CompatibilityReport {
                level: CompatibilityLevel::Unsupported,
                unsupported_features: Vec::new(),
                warnings: Vec::new(),
                estimated_translation_cost: None,
            },
        )
    }
    fn solve<'a>(
        &'a self,
        request: &'a SolveRequest,
        _output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(async move {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
            }
            if let Some(clock) = &self.expire_backend_clock {
                assert!(
                    clock
                        .advance(request.dispatch_budget().remaining_backend_duration())
                        .is_ok()
                );
            }
            Err(BackendError::new(
                "tests.headless.failure",
                "Deliberate bounded backend failure",
            )?)
        })
    }
}

fn failing_registry(
    cancellation: Option<CancellationToken>,
    expire_backend_clock: Option<Arc<FixedMonotonicClock>>,
) -> TestResult<(SolverRegistry, Arc<FailingBackend>)> {
    let generated = CapabilityMatrix::generated()?;
    let features: Vec<_> = generated.features().cloned().collect();
    let backend_id = BackendId::new(BACKEND)?;
    let cells = features
        .iter()
        .map(|feature| {
            (
                feature.id.clone(),
                SupportCell::Supported {
                    fixture_id: format!("tests.headless.failure.{}", feature.id.as_str()),
                },
            )
        })
        .collect();
    let matrix = CapabilityMatrix::new(
        generated.schema_version(),
        generated.planning_ir_schema_version(),
        features,
        vec![BackendSupportColumn {
            backend_id: backend_id.clone(),
            backend_version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            cells,
        }],
        Vec::new(),
    )?;
    let descriptor = SolverDescriptor {
        id: backend_id.clone(),
        display_name: "Failure-only lifecycle fixture".to_owned(),
        version: "1.0.0".to_owned(),
        adapter_version: "1.0.0".to_owned(),
        distribution: SolverDistribution::BuiltIn,
        license: LicenseMetadata {
            spdx_expression: "Apache-2.0".to_owned(),
            license_name: "Apache License 2.0".to_owned(),
            source_url: None,
        },
        stability: BackendStability::Experimental,
        capabilities: matrix.backend_capabilities(&backend_id)?,
    };
    let runtime = BackendRuntimeIdentity::new(
        backend_id,
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        1,
        0,
    )?;
    let backend = Arc::new(FailingBackend {
        descriptor,
        runtime,
        matrix: matrix.clone(),
        cancellation,
        expire_backend_clock,
        invocations: AtomicU32::new(0),
    });
    let registered: Vec<Arc<dyn SolverBackend>> = vec![backend.clone()];
    Ok((SolverRegistry::new(matrix, registered)?, backend))
}

#[tokio::test]
async fn backend_failure_cancellation_and_time_limit_commit_truthful_terminal_state_without_results()
-> TestResult {
    for (cancel_after_start, expire_backend, expected_status) in [
        (false, false, SolveStatus::BackendFailed),
        (true, false, SolveStatus::Cancelled),
        (false, true, SolveStatus::NoSolutionWithinLimit),
    ] {
        let fixture = Fixture::new().await?;
        let mut dependencies = fixture.dependencies()?;
        let clock = Arc::new(FixedMonotonicClock::default());
        dependencies.monotonic_clock = clock.clone();
        let cancellation = dependencies.cancellation.clone();
        let (registry, backend) = failing_registry(
            cancel_after_start.then_some(cancellation),
            expire_backend.then_some(clock),
        )?;
        let app = fixture.app(registry, dependencies);
        let request = fixture.request()?;
        let outcome = Box::pin(app.solve_stored(request.clone(), &mut Progress))
            .await
            .map_err(boxed)?;
        assert!(!outcome.reused);
        assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
        assert_eq!(
            outcome
                .state()
                .manifest
                .as_ref()
                .ok_or("run remained running")?
                .outcome,
            RunTerminalOutcomeV1::NoResult {
                status: expected_status
            }
        );
        if expire_backend {
            let manifest = outcome.state().manifest.as_ref().ok_or("manifest")?;
            let native = manifest
                .native_observations
                .as_ref()
                .ok_or("native observations")?;
            assert_eq!(native.terminal_status, SolveStatus::NoSolutionWithinLimit);
            assert_eq!(
                native.terminal_reason,
                eutheto_domain_ir::RunNativeRouterReason::BackendTerminated
            );
            let attempt = native.attempt.as_ref().ok_or("attempt")?;
            assert_eq!(attempt.invocation_index, Some(1));
            assert_eq!(
                attempt.termination,
                eutheto_domain_ir::RunNativeAttemptTermination::BackendOutcome(
                    eutheto_domain_ir::RunNativeTerminationReason::TimeLimit,
                )
            );
            assert!(attempt.outcome.is_none());
            assert_eq!(
                attempt.backend_failure_code.as_deref(),
                Some("tests.headless.failure")
            );
            assert_eq!(
                manifest.elapsed_milliseconds,
                Some(attempt.backend_limit_milliseconds)
            );
            assert_eq!(
                manifest.phase_timings.backend_milliseconds,
                Some(attempt.backend_limit_milliseconds)
            );
            assert!(attempt.backend_limit_milliseconds < request.options.time_limit_milliseconds);
            let decoded = RunManifest::from_json(&serde_json::to_vec(manifest)?)?;
            assert_eq!(&decoded, manifest);
        }
        assert!(outcome.accepted_result_id().is_none());
        assert!(outcome.portable_result().is_none());
        assert_eq!(
            &fixture
                .store
                .load_solve_run_by_request(request.request_id)
                .await?
                .ok_or("missing run")?,
            outcome.state()
        );
        assert!(
            fixture
                .store
                .list_accepted_results(request.scenario_id)
                .await?
                .is_empty()
        );

        // Even after cancellation, a terminal retry cannot invoke the backend again.
        let retry = Box::pin(app.solve_stored(request, &mut Progress))
            .await
            .map_err(boxed)?;
        assert!(retry.reused);
        assert_eq!(retry.state(), outcome.state());
        assert!(retry.report.is_none());
        assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

#[tokio::test]
async fn compilation_resource_limit_starts_no_run_and_preserves_stored_revision() -> TestResult {
    let fixture = Fixture::new().await?;
    let (registry, backend) = failing_registry(None, None)?;
    let app = fixture.app(registry, fixture.dependencies()?);
    let mut request = fixture.request()?;
    request.options.resource_limits.max_variables = 1;
    let Err(StoredSolveFailure::Preparation(failure)) =
        Box::pin(app.solve_stored(request.clone(), &mut Progress)).await
    else {
        return Err("resource-limited compilation did not fail before admission".into());
    };
    assert_eq!(failure.status, SolveStatus::NoSolutionWithinLimit);
    assert!(matches!(
        failure.error,
        AppError::Protocol(ref error) if error.code == "operation.resource_limit"
    ));
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .store
            .load_solve_run_by_request(request.request_id)
            .await?
            .is_none()
    );
    assert!(
        fixture
            .store
            .list_accepted_results(request.scenario_id)
            .await?
            .is_empty()
    );
    assert_eq!(
        fixture
            .store
            .get_project(request.scenario_id)
            .await?
            .summary
            .revision,
        Revision::INITIAL
    );
    Ok(())
}

#[tokio::test]
async fn current_revision_capture_is_explicit_and_observed_revision_conflicts_never_rebase()
-> TestResult {
    let fixture = Fixture::new().await?;
    fixture.advance_revision().await?;
    let mut dependencies = fixture.dependencies()?;
    dependencies.ids = Arc::new(FixedIdGenerator::new([id(400).parse()?, id(401).parse()?]));
    let (registry, backend) = failing_registry(None, None)?;
    let app = fixture.app(registry, dependencies);
    let mut request = fixture.request()?;
    assert!(matches!(
        Box::pin(app.solve_stored(request.clone(), &mut Progress)).await,
        Err(StoredSolveFailure::BeforeStart(AppError::Conflict {
            expected_revision,
            actual_revision,
        })) if expected_revision == Revision::INITIAL && actual_revision == Revision::new(1)
    ));
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 0);
    request.expected_revision = None;
    let outcome = Box::pin(app.solve_stored(request, &mut Progress))
        .await
        .map_err(boxed)?;
    assert_eq!(outcome.state().input.scenario_revision, 1);
    let captured = fixture
        .store
        .load_solve_input(outcome.state().input.run_id)
        .await?;
    assert_eq!(
        captured.document.domain.entities[&id(1).parse()?]["name"],
        "River at the newer revision"
    );
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn unsupported_stored_objective_selection_is_rejected_before_run_admission() -> TestResult {
    let fixture = Fixture::new().await?;
    let (registry, backend) = failing_registry(None, None)?;
    let app = fixture.app(registry, fixture.dependencies()?);
    let mut request = fixture.request()?;
    request.objective_selection = Some(StoredObjectiveSelectionV1 {
        schema_version: 2,
        profile_id: ObjectiveProfileId::new(AUTHORED_OBJECTIVE_PROFILE_ID)?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Optimize,
        baseline_source: StoredBaselineSourceV1::Accepted,
        alternatives_capture_id: None,
    });
    let Err(failure) = Box::pin(app.solve_stored(request.clone(), &mut Progress)).await else {
        return Err("unsupported selector started a solve".into());
    };
    assert!(matches!(
        &failure,
        StoredSolveFailure::BeforeStart(AppError::Protocol(error))
            if error.code == "solve.objective_selection_invalid"
    ));
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .store
            .load_solve_run_by_request(request.request_id)
            .await?
            .is_none()
    );
    Ok(())
}

fn external_repair_selection() -> TestResult<StoredObjectiveSelectionV1> {
    Ok(StoredObjectiveSelectionV1 {
        schema_version: 1,
        profile_id: "official.workforce.objective.minimal-changes".parse()?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Repair,
        baseline_source: StoredBaselineSourceV1::External {
            entity_id: id(50).parse()?,
        },
        alternatives_capture_id: None,
    })
}

#[tokio::test]
async fn external_stored_retry_preserves_captured_source_after_edit_and_deletion() -> TestResult {
    let fixture = Fixture::new_with_external_plan(true).await?;
    let (registry, backend) = failing_registry(None, None)?;
    let app = fixture.app(registry, fixture.dependencies()?);
    let mut request = fixture.request()?;
    request.objective_selection = Some(external_repair_selection()?);
    let first = app
        .solve_stored(request.clone(), &mut Progress)
        .await
        .map_err(boxed)?;
    let run_id = first.state().input.run_id;
    let captured = fixture.store.load_solve_input(run_id).await?;
    assert_eq!(captured.document, fixture.document);
    let context = captured
        .input
        .objective_context
        .as_ref()
        .ok_or("missing external context")?;
    assert_eq!(context.schema_version, 3);
    assert!(context.baseline.is_none());
    let binding = context
        .external_baseline
        .as_deref()
        .ok_or("missing external source")?;
    let pairs = [eutheto_workforce::model::AssignmentPair {
        person_id: id(1).parse()?,
        shift_id: id(8).parse()?,
    }];
    assert_eq!(
        binding.selected_pair_content_hash,
        eutheto_workforce::assignment_rules::workforce_pair_content_hash(&pairs, None)?,
    );
    assert_ne!(binding.selected_pair_content_hash, "b".repeat(64));
    let input_bytes = serde_json::to_vec(&captured.input)?;
    let snapshot_bytes = serde_json::to_vec(&captured.document)?;
    let mut revision = Revision::INITIAL;
    for remove in [false, true] {
        revision = fixture.change_external_plan(&app, revision, remove).await?;
        assert_eq!(
            fixture
                .store
                .get_project(request.scenario_id)
                .await?
                .document
                .domain
                .entities
                .contains_key(&id(50).parse()?),
            !remove,
        );
        for expected_revision in [Some(Revision::INITIAL), None] {
            let mut retry_request = request.clone();
            retry_request.expected_revision = expected_revision;
            let retry = app
                .solve_stored(retry_request, &mut Progress)
                .await
                .map_err(boxed)?;
            assert!(retry.reused);
            assert!(retry.report.is_none());
            assert_eq!(retry.state(), first.state());
        }
        for changed_source in [
            StoredBaselineSourceV1::Accepted,
            StoredBaselineSourceV1::External {
                entity_id: id(51).parse()?,
            },
        ] {
            let mut changed = request.clone();
            changed
                .objective_selection
                .as_mut()
                .ok_or("missing selector")?
                .baseline_source = changed_source;
            assert!(matches!(
                app.solve_stored(changed, &mut Progress).await,
                Err(StoredSolveFailure::BeforeStart(AppError::Validation(report)))
                    if report.issues.iter().any(|issue| issue.code == "solve.request_id_conflict")
            ));
        }
        let retained = fixture.store.load_solve_input(run_id).await?;
        assert_eq!(serde_json::to_vec(&retained.input)?, input_bytes);
        assert_eq!(serde_json::to_vec(&retained.document)?, snapshot_bytes);
    }
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    assert!(
        fixture
            .store
            .list_accepted_results(request.scenario_id)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn external_alternatives_share_one_capture_and_refuse_new_members_after_deletion()
-> TestResult {
    let fixture = Fixture::new_with_external_plan(true).await?;
    let mut dependencies = fixture.dependencies()?;
    dependencies.ids = Arc::new(FixedIdGenerator::new(
        (400..405)
            .map(|index| id(index).parse())
            .collect::<Result<Vec<_>, _>>()?,
    ));
    let (registry, backend) = failing_registry(None, None)?;
    let app = fixture.app(registry, dependencies);
    let options = options()?;
    let parent = ParentSolveBudget::new(
        options.time_limit_milliseconds,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    let started_at = NOW.parse()?;
    let capture = app
        .capture_objective_alternatives_source(
            fixture.document.scenario_id,
            Some(Revision::INITIAL),
            StoredBaselineSourceV1::External {
                entity_id: id(50).parse()?,
            },
            OperationControl::Solve(parent.phase_view()),
            started_at,
        )
        .await
        .map_err(boxed)?;
    let mut shared_source = None;
    let mut retained_members = Vec::new();
    for (index, profile) in [
        "official.workforce.objective.balanced",
        "official.workforce.objective.preference-focused",
        "official.workforce.objective.fairness-first",
        "official.workforce.objective.minimal-changes",
    ]
    .into_iter()
    .enumerate()
    {
        let request = StoredSolveRequest {
            request_id: id(320 + u32::try_from(index)?).parse()?,
            scenario_id: fixture.document.scenario_id,
            expected_revision: Some(capture.revision()),
            options: options.clone(),
            objective_selection: Some(
                capture.selection(profile.parse()?, RunObjectiveOperation::Optimize),
            ),
        };
        let outcome = app
            .solve_stored_with_budget_observer_in_capture(
                request,
                &mut Progress,
                parent.clone(),
                started_at,
                Arc::new(|_| {}),
                &capture,
                u8::try_from(4 - index)?,
            )
            .await
            .map_err(boxed)?;
        let loaded = fixture
            .store
            .load_solve_input(outcome.state().input.run_id)
            .await?;
        let context = loaded
            .input
            .objective_context
            .as_ref()
            .ok_or("missing member context")?;
        assert_eq!(context.schema_version, 3);
        assert_eq!(context.profile_id.as_str(), profile);
        assert!(context.baseline.is_none());
        let baseline = context
            .external_baseline
            .as_deref()
            .ok_or("missing member source")?;
        assert_eq!(loaded.input.snapshot_id, capture.snapshot_id());
        assert_eq!(loaded.input.scenario_revision, Revision::INITIAL.value());
        assert_eq!(loaded.document, fixture.document);
        assert_eq!(
            outcome
                .state()
                .manifest
                .as_ref()
                .ok_or("missing member terminal")?
                .started_at,
            started_at,
        );
        let source = (
            loaded.input.snapshot_document_hash.clone(),
            loaded.input.snapshot_created_at,
            baseline.clone(),
        );
        if let Some(expected) = &shared_source {
            assert_eq!(&source, expected);
        } else {
            shared_source = Some(source);
        }
        retained_members.push((
            loaded.input.run_id,
            serde_json::to_vec(&loaded.input)?,
            serde_json::to_vec(&loaded.document)?,
        ));
    }
    let revision = fixture
        .change_external_plan(&app, Revision::INITIAL, true)
        .await?;
    let refused = StoredSolveRequest {
        request_id: id(324).parse()?,
        scenario_id: fixture.document.scenario_id,
        expected_revision: Some(capture.revision()),
        options,
        objective_selection: Some(capture.selection(
            "official.workforce.objective.balanced".parse()?,
            RunObjectiveOperation::Optimize,
        )),
    };
    assert!(matches!(
        app.solve_stored_with_budget_observer_in_capture(
            refused.clone(),
            &mut Progress,
            parent,
            started_at,
            Arc::new(|_| {}),
            &capture,
            1,
        ).await,
        Err(StoredSolveFailure::BeforeStart(AppError::Conflict {
            expected_revision: Revision::INITIAL,
            actual_revision,
        })) if actual_revision == revision
    ));
    assert!(
        fixture
            .store
            .load_solve_run_by_request(refused.request_id)
            .await?
            .is_none()
    );
    for (run_id, input_bytes, snapshot_bytes) in retained_members {
        let retained = fixture.store.load_solve_input(run_id).await?;
        assert_eq!(serde_json::to_vec(&retained.input)?, input_bytes);
        assert_eq!(serde_json::to_vec(&retained.document)?, snapshot_bytes);
    }
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 4);
    assert!(
        !fixture
            .store
            .get_project(fixture.document.scenario_id)
            .await?
            .document
            .domain
            .entities
            .contains_key(&id(50).parse()?)
    );
    Ok(())
}

#[tokio::test]
async fn copying_external_plan_remaps_current_pairs_without_rewriting_provenance_or_capture()
-> TestResult {
    let fixture = Fixture::new_with_external_plan(true).await?;
    let (registry, _) = failing_registry(None, None)?;
    let app = fixture.app(registry, fixture.dependencies()?);
    let mut request = fixture.request()?;
    request.objective_selection = Some(external_repair_selection()?);
    let outcome = app
        .solve_stored(request.clone(), &mut Progress)
        .await
        .map_err(boxed)?;
    let run_id = outcome.state().input.run_id;
    let original = fixture.store.load_solve_input(run_id).await?;
    let revision = fixture
        .change_external_plan(&app, Revision::INITIAL, false)
        .await?;
    let source = fixture.store.get_project(request.scenario_id).await?;
    let mut dependencies = fixture.dependencies()?;
    dependencies.ids = Arc::new(FixedIdGenerator::new(
        (500..550)
            .map(|index| id(index).parse())
            .collect::<Result<Vec<_>, _>>()?,
    ));
    let duplicator = fixture.app(SolverRegistry::production()?, dependencies);
    let AppCommandResult::Project(copy) = duplicator
        .execute(AppCommand::DuplicateProject {
            request_id: id(330).parse()?,
            source_id: request.scenario_id,
            expected_revision: revision,
            title: "Copied unverified starting plan".to_owned(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("copy omitted project".into());
    };
    let copied = fixture.store.get_project(copy.scenario_id).await?;
    let (plan_id, plan) = copied
        .document
        .domain
        .entities
        .iter()
        .find(|(_, entity)| entity["kind"] == "externalStartingPlan")
        .ok_or("copy omitted external plan")?;
    assert_ne!(*plan_id, id(50).parse()?);
    assert_eq!(plan["id"], serde_json::to_value(plan_id)?);
    assert_eq!(
        plan["originalImport"],
        source.document.domain.entities[&id(50).parse()?]["originalImport"]
    );
    assert_eq!(
        plan["name"],
        source.document.domain.entities[&id(50).parse()?]["name"]
    );
    assert_eq!(
        plan["assignments"].as_array().ok_or("copied pairs")?.len(),
        1
    );
    let pair = &plan["assignments"][0];
    let person_id: eutheto_types::EntityId =
        pair["personId"].as_str().ok_or("copied person")?.parse()?;
    let shift_id: eutheto_types::EntityId =
        pair["shiftId"].as_str().ok_or("copied shift")?.parse()?;
    assert_ne!(person_id, id(12).parse()?);
    assert_ne!(shift_id, id(8).parse()?);
    assert_eq!(copied.document.domain.entities[&person_id]["name"], "Sky");
    assert_eq!(
        copied.document.domain.entities[&shift_id]["kind"],
        "shiftInstance"
    );
    let unchanged = fixture.store.get_project(request.scenario_id).await?;
    assert_eq!(unchanged.document, source.document);
    let retained = fixture.store.load_solve_input(run_id).await?;
    assert_eq!(
        serde_json::to_vec(&retained.input)?,
        serde_json::to_vec(&original.input)?
    );
    assert_eq!(
        serde_json::to_vec(&retained.document)?,
        serde_json::to_vec(&original.document)?
    );
    assert_eq!(
        fixture
            .store
            .load_solve_run_by_request(request.request_id)
            .await?
            .ok_or("original run")?,
        *outcome.state(),
    );
    assert!(
        fixture
            .store
            .list_solve_runs(SolveRunListRequestV1 {
                scenario_id: copy.scenario_id,
                scenario_revision: None,
                limit: 10,
            })
            .await?
            .is_empty()
    );
    Ok(())
}
