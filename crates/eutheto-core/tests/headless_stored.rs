#![forbid(unsafe_code)]

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_core::{
    AppCommand, AppDependencies, AppPaths, EuthetoApp, StoredSolveFailure, StoredSolveRequest,
};
use eutheto_domain_api::{CompileContext, DomainPackRegistry};
use eutheto_domain_ir::{RunManifestV1, RunPhaseTimingsV1, RunTerminalOutcomeV1};
use eutheto_planning_ir::{
    PlanningIrLimitsV1, PlanningProblemSummary, canonical_ir_hash, canonical_objective_policy_hash,
};
use eutheto_solver_api::*;
use eutheto_store::{InitializationOutcome, NewProject, NewSolveRunV1, SqliteScenarioStore};
use eutheto_types::{
    ActorRef, AppError, BackendId, BackendSelection, CancellationToken, Clock, CommandEnvelope,
    CommandSource, DomainCommandEnvelope, DurationMillis, ExplanationMode, FixedClock,
    FixedIdGenerator, FixedMonotonicClock, MonotonicClock, OperationControl, PreservationPolicy,
    ReproducibilityMode, ResourceLimits, Revision, Rfc3339Timestamp, ScenarioCommand,
    ScenarioDocument, SolveMode, SolveOptions, SolveStatus, WorkerThreadPolicy,
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
        let directory = tempfile::Builder::new()
            .prefix("eutheto-headless-stored-")
            .tempdir_in(dirs::home_dir().ok_or("platform home directory is unavailable")?)?;
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
        })
    }

    fn seed_run(&self, request: &StoredSolveRequest) -> TestResult<NewSolveRunV1> {
        let expected_revision = request
            .expected_revision
            .ok_or("seed requires an explicit revision")?;
        let context = CompileContext {
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
    let started = fixture
        .store
        .start_solve_run(fixture.seed_run(&request)?)
        .await?;
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

    let manifest = RunManifestV1::new(
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
        .finalize_terminal_run(manifest.clone())
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

// This backend can only fail. It exercises dispatch/finalization, never accepted-result authority.
struct FailingBackend {
    descriptor: SolverDescriptor,
    runtime: BackendRuntimeIdentity,
    matrix: CapabilityMatrix,
    cancellation: Option<CancellationToken>,
    invocations: AtomicU32,
}

impl SolverBackend for FailingBackend {
    fn descriptor(&self) -> &SolverDescriptor {
        &self.descriptor
    }
    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        &self.runtime
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
        _request: &'a SolveRequest,
        _output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(async move {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
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
        invocations: AtomicU32::new(0),
    });
    let registered: Vec<Arc<dyn SolverBackend>> = vec![backend.clone()];
    Ok((SolverRegistry::new(matrix, registered)?, backend))
}

#[tokio::test]
async fn backend_failure_and_post_start_cancellation_commit_truthful_terminal_state_without_results()
-> TestResult {
    for (cancel_after_start, expected_status) in [
        (false, SolveStatus::BackendFailed),
        (true, SolveStatus::Cancelled),
    ] {
        let fixture = Fixture::new().await?;
        let dependencies = fixture.dependencies()?;
        let cancellation = dependencies.cancellation.clone();
        let (registry, backend) = failing_registry(cancel_after_start.then_some(cancellation))?;
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
    let (registry, backend) = failing_registry(None)?;
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
    let (registry, backend) = failing_registry(None)?;
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
