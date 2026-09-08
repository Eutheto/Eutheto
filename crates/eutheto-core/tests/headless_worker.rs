#![forbid(unsafe_code)]

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

#[cfg(debug_assertions)]
use eutheto_core::StoredSolveFailure;
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, EuthetoApp, HeadlessService,
    HeadlessSolveOutcome, StoredSolveRequest,
};
use eutheto_domain_api::DomainPackRegistry;
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, PortableAcceptedResultV2, RunManifestV1, RunTerminalOutcomeV1,
    VerificationContextV1, VerificationReport,
};
use eutheto_planning_ir::PlanningProblemSummary;
use eutheto_solver_api::{
    BackendCandidate, BackendError, BackendOutputSink, BackendRuntimeIdentity, BackendSolveFuture,
    BackendTerminationReason, CandidateSubmission, CompatibilityReport, OutputError, ProgressSink,
    SolveProgressEvent, SolveRequest, SolverBackend, SolverDescriptor, SolverRegistry,
};
use eutheto_solver_ortools::{ORTOOLS_BACKEND_ID, VerifiedWorkerArtifact, registry_with_ortools};
use eutheto_store::{NewProject, SqliteScenarioStore};
use eutheto_types::{
    ActorRef, AppError, BackendSelection, CancellationToken, Clock, CommandEnvelope, CommandId,
    CommandSource, DomainCommandEnvelope, DurationMillis, ExplanationMode, OperationControl,
    PreservationPolicy, ReproducibilityMode, RequestId, ResourceLimits, Revision, Rfc3339Timestamp,
    ScenarioCommand, ScenarioSnapshotV1, SolveMode, SolveOptions, SolveStatus, SystemClock,
    SystemIdGenerator, SystemMonotonicClock, ValidationSeverity, WorkerThreadPolicy,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{collections::BTreeSet, error::Error, fmt::Debug, path::PathBuf, sync::Arc};
use workforce_fixture::id;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: impl Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

// The UTC/manual-shift transformation follows workforce_assignment_rules::fixture_value.
// Removing its template and template lock avoids the original fixture's rejected DST fold.
// One always-active qualified person can cover the sole 08:00-10:00 UTC clinic shift;
// its 120 elapsed minutes also match the person's existing workload target. No availability
// exclusions, competing shifts, preferences, or locks remain. Exact coverage 2 is impossible
// with this same one-person universe, but is a supported model rather than malformed input.
fn snapshot(coverage: u32) -> TestResult<ScenarioSnapshotV1> {
    let mut value = serde_json::to_value(workforce_fixture::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["lockedAssignments"] = json!({});
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("missing fixture entities")?
        .remove(&id(6));
    let shift = &mut value["domain"]["entities"][id(8)];
    shift["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z","local":"2026-11-01T08:00:00","offsetSeconds":0
    });
    shift["endsAt"] = json!({
        "instant":"2026-11-01T10:00:00Z","local":"2026-11-01T10:00:00","offsetSeconds":0
    });
    shift["coverage"]["count"] = json!(coverage);
    for (index, kind) in [
        (30, "eligibility"),
        (31, "availability"),
        (32, "coverage"),
        (33, "noOverlap"),
    ] {
        let mut rule = json!({
            "id":id(index),"kind":kind,"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}}
        });
        if kind == "noOverlap" {
            rule["compatibleCategoryPairs"] = json!([]);
        }
        value["domain"]["rules"][id(index)] = rule;
    }
    Ok(ScenarioSnapshotV1::current(
        Revision::INITIAL,
        serde_json::from_value(value)?,
        BTreeSet::default(),
    ))
}

async fn real_registry() -> TestResult<(SolverRegistry, BackendRuntimeIdentity)> {
    let root = PathBuf::from(std::env::var_os("EUTHETO_TEST_ORTOOLS_ARTIFACT")
        .ok_or("EUTHETO_TEST_ORTOOLS_ARTIFACT must name an installed real OR-Tools artifact; no helper fallback")?);
    let bytes = std::fs::read(root.join("solver-manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    if manifest["approval"].is_null()
        || manifest["backend_source"]["sha256"]
            .as_str()
            .is_none_or(|value| value.len() != 64)
    {
        return Err("Workforce evidence requires an approved real worker artifact".into());
    }
    let artifact = VerifiedWorkerArtifact::verify(root, Sha256::digest(&bytes).into()).await?;
    let registry = registry_with_ortools(artifact)?;
    let runtime = registry
        .get(&ORTOOLS_BACKEND_ID.parse()?)
        .ok_or("missing registered OR-Tools runtime")?
        .runtime_identity()
        .clone();
    Ok((registry, runtime))
}

async fn real_service() -> TestResult<(HeadlessService, BackendRuntimeIdentity)> {
    let (registry, runtime) = real_registry().await?;
    let service = HeadlessService::with_solver_registry(
        Arc::new(SystemClock),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
        registry,
    )
    .map_err(boxed)?;
    Ok((service, runtime))
}

fn options() -> TestResult<SolveOptions> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(60_000)?,
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

struct Progress;

impl ProgressSink for Progress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}

fn assert_bound_runtime(
    outcome: &HeadlessSolveOutcome,
    runtime: &BackendRuntimeIdentity,
) -> TestResult {
    // These checks accompany a conclusive real-worker result, not a fabricated routing record.
    // Auto must become a single immutable dispatch, with corroborated worker runtime evidence.
    assert_eq!(outcome.execution.invocation_count, 1);
    let [attempt] = outcome.execution.attempts.as_slice() else {
        return Err("ordinary Auto solve must retain exactly one backend attempt".into());
    };
    assert!(!attempt.fallback_taken);
    let backend = attempt
        .outcome
        .as_ref()
        .ok_or("missing real backend outcome")?;
    let evidence = &backend
        .evidence
        .execution
        .as_ref()
        .ok_or("missing corroborated real-worker execution evidence")?
        .reproducibility;
    assert_eq!(&backend.backend_id, runtime.backend_id());
    assert_eq!(&outcome.run_input().backend_id, runtime.backend_id());
    assert_eq!(
        outcome.run_input().solve_options.backend,
        BackendSelection::Specific(runtime.backend_id().clone())
    );
    assert_eq!(
        evidence.applied_options.backend,
        BackendSelection::Specific(runtime.backend_id().clone())
    );
    assert_eq!(outcome.run_input().worker_version, runtime.worker_version());
    assert_eq!(outcome.run_input().solver_version, runtime.solver_version());
    assert_eq!(evidence.worker_version, runtime.worker_version());
    assert_eq!(evidence.engine_version, runtime.solver_version());
    assert_eq!(evidence.applied_parameters.worker_threads, 1);
    assert!(evidence.applied_parameters_sha256.is_some());
    assert_eq!(backend.model_hash, outcome.run_input().model_hash);
    Ok(())
}

fn reseal_forged_score(
    portable: &PortableAcceptedResultV2,
) -> TestResult<PortableAcceptedResultV2> {
    let original = &portable.accepted_result;
    let report = &original.verification;
    let context = VerificationContextV1::new(
        report.scenario_id,
        report.evaluated_revision,
        report.document_hash.clone(),
        report.planning_model_hash.clone(),
        original.solution.canonical_hash()?,
        report.verification_scope_checksum.clone(),
    )?;
    let mut score = report.score.clone();
    let level = score
        .levels
        .first_mut()
        .ok_or("missing Workforce rank score")?;
    level.value = level.value.checked_add(1).ok_or("rank score overflow")?;
    let report = VerificationReport::new(
        &context,
        report.required_rule_results.clone(),
        score,
        report.warnings.clone(),
        report.metrics.clone(),
    )?;
    let accepted = AcceptedResult::new(original.solution.clone(), report)?;
    let manifest = &portable.run_manifest;
    let RunTerminalOutcomeV1::Accepted { status, .. } = &manifest.outcome else {
        return Err("portable result has no accepted outcome".into());
    };
    let manifest = RunManifestV1::new(
        manifest.run_id,
        manifest.run_input_checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: *status,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        manifest.started_at,
        manifest.finished_at,
        manifest.elapsed_milliseconds,
        manifest.first_incumbent_milliseconds,
        manifest.first_verified_feasible_milliseconds,
        manifest.phase_timings,
        manifest.verification_warnings.clone(),
    )?;
    Ok(PortableAcceptedResultV2::new(
        portable.run_input.clone(),
        manifest,
        accepted,
        portable.evidence.clone(),
    )?)
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn auto_worker_result_is_portable_but_requires_fresh_source_verification() -> TestResult {
    let (service, runtime) = real_service().await?;
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let snapshot = snapshot(1)?;
    let prepared = operation.prepare(snapshot.clone()).map_err(boxed)?;
    assert!(
        !prepared
            .compilation()
            .validation
            .issues
            .iter()
            .any(|issue| issue.severity == ValidationSeverity::Error)
    );
    let outcome = prepared.execute(&mut Progress).await.map_err(boxed)?;
    assert_eq!(outcome.execution.terminal_status, SolveStatus::Optimal);
    assert_bound_runtime(&outcome, &runtime)?;
    let accepted = outcome
        .accepted_result()
        .ok_or("real feasible candidate was not accepted")?;
    let [assignment] = accepted.solution.assignments.as_slice() else {
        return Err("expected the sole person/shift assignment, without rank auxiliaries".into());
    };
    assert_eq!(
        assignment.entity.id.as_str(),
        format!("{}.{}", id(1), id(8))
    );
    assert_eq!(assignment.value, AssignmentValue::Boolean(true));
    assert!(accepted.verification.accepted);
    assert_eq!(accepted.verification.score.feasibility, 0);
    let first = outcome
        .manifest()
        .first_incumbent_milliseconds
        .ok_or("missing first incumbent timing")?;
    let verified = outcome
        .manifest()
        .first_verified_feasible_milliseconds
        .ok_or("missing first verified timing")?;
    let elapsed = outcome
        .manifest()
        .elapsed_milliseconds
        .ok_or("missing parent elapsed timing")?;
    assert!(first <= verified && verified <= elapsed);
    assert!(elapsed <= outcome.run_input().solve_options.time_limit_milliseconds);

    let control = OperationControl::Cancellation(CancellationToken::new());
    let portable = outcome
        .into_portable()
        .map_err(boxed)?
        .ok_or("missing portable acceptance")?;
    let portable = PortableAcceptedResultV2::from_json(&serde_json::to_vec(&portable)?)?;
    let fresh = service
        .verify_result(&snapshot, &portable, &control)
        .map_err(boxed)?;
    assert!(fresh.accepted);
    assert_eq!(fresh.score, portable.accepted_result.verification.score);

    // Recompute every enclosing checksum: rejection must come from independent source scoring,
    // not a broken checksum or an untrusted report simply claiming that it was accepted.
    let forged = reseal_forged_score(&portable)?;
    forged.validate()?;
    assert!(service.verify_result(&snapshot, &forged, &control).is_err());
    let mut changed_source = snapshot.clone();
    changed_source.document = self::snapshot(2)?.document;
    assert!(
        service
            .verify_result(&changed_source, &portable, &control)
            .is_err()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn contradictory_required_coverage_reaches_worker_and_is_proven_infeasible() -> TestResult {
    let (service, runtime) = real_service().await?;
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let snapshot = snapshot(2)?;
    let readiness = service
        .validate_full(&snapshot, &operation.control())
        .map_err(boxed)?;
    assert!(readiness.issues.iter().any(|issue| {
        issue.severity == ValidationSeverity::Error
            && issue.code == "official.workforce.candidate_shortage"
    }));
    // A readiness contradiction is not a compiler/admission error and not itself an
    // infeasibility proof. Successful preparation followed by real dispatch is required.
    let prepared = operation.prepare(snapshot).map_err(boxed)?;
    assert_eq!(prepared.compilation().validation, readiness);
    let outcome = prepared.execute(&mut Progress).await.map_err(boxed)?;
    assert_bound_runtime(&outcome, &runtime)?;
    assert_eq!(outcome.execution.terminal_status, SolveStatus::Infeasible);
    assert_eq!(
        outcome.execution.attempts[0]
            .outcome
            .as_ref()
            .ok_or("missing backend proof")?
            .termination,
        BackendTerminationReason::InfeasibilityClaimed,
    );
    assert!(matches!(
        outcome.manifest().outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible
        }
    ));
    assert!(outcome.accepted_result().is_none());
    assert!(outcome.execution.selected_candidate.is_none());
    assert!(outcome.manifest().first_incumbent_milliseconds.is_none());
    assert!(
        outcome
            .manifest()
            .first_verified_feasible_milliseconds
            .is_none()
    );
    assert!(outcome.into_portable().map_err(boxed)?.is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn stored_worker_acceptance_survives_reopen_edits_retries_and_secondary_output_failure()
-> TestResult {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-headless-worker-")
        .tempdir_in(dirs::home_dir().ok_or("platform home directory is unavailable")?)?;
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("eutheto.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let solvers = Arc::new(real_registry().await?.0);
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    store
        .create_project(NewProject {
            document: original.document.clone(),
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies.clone(),
        Arc::clone(&packs),
        Arc::clone(&solvers),
    );
    let request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: original.revision,
        options: options()?,
    };
    let outcome = Box::pin(app.solve_stored(request.clone(), &mut Progress))
        .await
        .map_err(boxed)?;
    assert!(!outcome.reused);
    let execution = &outcome
        .report
        .as_ref()
        .ok_or("missing actual solve report")?
        .execution;
    assert_eq!(execution.invocation_count, 1);
    assert_eq!(execution.terminal_status, SolveStatus::Optimal);
    let result_id = outcome
        .accepted_result_id()
        .ok_or("real stored solve was not accepted")?;
    let portable = outcome
        .portable_result()
        .ok_or("committed portable output did not reload")?
        .clone();
    let state = outcome.state().clone();
    let loaded = store.load_accepted_result(result_id).await?;
    assert_eq!(loaded.document, original.document);
    assert_eq!(loaded.portable, portable);
    // Acceptance commits a result, not an edit or implicit selection of the user's scenario.
    let unchanged = store.get_project(scenario_id).await?;
    assert_eq!(unchanged.document, original.document);
    assert_eq!(unchanged.summary.revision, original.revision);
    let results = store.list_accepted_results(scenario_id).await?;
    let [result] = results.as_slice() else {
        return Err("expected exactly one committed accepted result".into());
    };
    assert_eq!(result.result.solution_id, result_id);
    assert!(!result.stale && !result.selected);

    // Close the original actor and reload through a new SQLite connection, not a cached DTO.
    drop(outcome);
    drop(app);
    drop(store);
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let reopened = store.load_accepted_result(result_id).await?;
    assert_eq!(reopened, loaded);
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies.clone(),
        packs,
        solvers,
    );
    let verification_control = OperationControl::Cancellation(CancellationToken::new());
    assert!(
        app.headless_service()
            .verify_result(&original, &reopened.portable, &verification_control)
            .map_err(boxed)?
            .accepted
    );

    // The accepted one-person assignment cannot satisfy the next revision's coverage of two.
    let changed = serde_json::to_value(snapshot(2)?.document)?;
    let edit = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: original.revision,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Headless worker regression".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity": changed["domain"]["entities"][id(8)]}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    let AppCommandResult::ScenarioCommand(edit) = edit else {
        return Err("expected a committed scenario edit".into());
    };
    assert!(edit.new_revision > original.revision);
    let current = store.get_project(scenario_id).await?;
    let current_snapshot = ScenarioSnapshotV1::current(
        current.summary.revision,
        current.document.clone(),
        BTreeSet::default(),
    );
    assert!(
        app.headless_service()
            .verify_result(&current_snapshot, &reopened.portable, &verification_control)
            .is_err()
    );
    let results = store.list_accepted_results(scenario_id).await?;
    let [result] = results.as_slice() else {
        return Err("editing the scenario must not create or replace accepted results".into());
    };
    assert!(result.stale && !result.selected);

    // Even a now-cancelled application and a changed current revision must not rerun a
    // committed request. The original request retains its exact bound run and result.
    dependencies.cancellation.cancel();
    let replay = Box::pin(app.solve_stored(request, &mut Progress))
        .await
        .map_err(boxed)?;
    assert!(replay.reused);
    assert!(replay.report.is_none());
    assert_eq!(replay.state(), &state);
    assert_eq!(replay.accepted_result_id(), Some(result_id));
    assert_eq!(replay.portable_result(), Some(&reopened.portable));
    let after_replay = store.get_project(scenario_id).await?;
    assert_eq!(after_replay.summary.revision, current.summary.revision);
    assert_eq!(after_replay.document, current.document);

    let occupied = directory.path().join("occupied-result.json");
    let existing_bytes = b"do not replace this existing destination";
    std::fs::write(&occupied, existing_bytes)?;
    let publication = replay
        .publish_result(&occupied, &CancellationToken::new())
        .ok_or("secondary failure lost the committed result identity")?;
    assert_eq!(publication.result_id, result_id);
    assert!(publication.publication.is_err());
    assert_eq!(std::fs::read(&occupied)?, existing_bytes);
    assert_eq!(store.load_accepted_result(result_id).await?, reopened);

    let cancelled_path = directory.path().join("cancelled-result.json");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let publication = replay
        .publish_result(&cancelled_path, &cancelled)
        .ok_or("cancelled secondary output lost the committed result identity")?;
    assert_eq!(publication.result_id, result_id);
    assert!(publication.publication.is_err());
    assert!(!cancelled_path.exists());
    assert_eq!(store.load_accepted_result(result_id).await?, reopened);

    let published_path = directory.path().join("accepted-result.json");
    let publication = replay
        .publish_result(&published_path, &CancellationToken::new())
        .ok_or("missing secondary publication")?;
    assert_eq!(publication.result_id, result_id);
    publication.publication.map_err(boxed)?;
    let published = PortableAcceptedResultV2::from_json(&std::fs::read(&published_path)?)?;
    assert_eq!(
        published,
        store.load_accepted_result(result_id).await?.portable
    );
    assert!(
        app.headless_service()
            .verify_result(&original, &published, &verification_control)
            .map_err(boxed)?
            .accepted
    );
    assert_eq!(
        store
            .load_solve_run_by_request(state.input.request_id)
            .await?,
        Some(state),
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn accepted_file_publication_reports_original_cancellation_without_staging() -> TestResult {
    let (service, runtime) = real_service().await?;
    let cancellation = CancellationToken::new();
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            cancellation.clone(),
        )
        .map_err(boxed)?;
    let prepared = operation.prepare(snapshot(1)?).map_err(boxed)?;
    let outcome = prepared.execute(&mut Progress).await.map_err(boxed)?;
    assert_bound_runtime(&outcome, &runtime)?;
    assert!(
        outcome
            .accepted_result()
            .ok_or("real candidate was not accepted")?
            .verification
            .accepted
    );
    let directory = tempfile::Builder::new()
        .prefix("eutheto-headless-cancelled-output-")
        .tempdir_in(dirs::home_dir().ok_or("platform home directory is unavailable")?)?;
    let destination = directory.path().join("accepted-result.json");

    cancellation.cancel();
    assert!(matches!(
        outcome.prepare_publication(&destination),
        Err(AppError::Protocol(error)) if error.code == "operation.cancelled"
    ));
    assert!(!destination.exists());
    assert!(std::fs::read_dir(directory.path())?.next().is_none());
    Ok(())
}

struct WallDeadlineAtCompletion {
    started_at: Rfc3339Timestamp,
    completed_at: Rfc3339Timestamp,
    started: std::sync::atomic::AtomicBool,
}

impl Clock for WallDeadlineAtCompletion {
    fn now(&self) -> Rfc3339Timestamp {
        if self.started.swap(true, Ordering::SeqCst) {
            self.completed_at
        } else {
            self.started_at
        }
    }
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn real_candidate_revoked_by_wall_deadline_retains_incumbent_not_verified_milestone()
-> TestResult {
    let (registry, runtime) = real_registry().await?;
    let options = options()?;
    let started_at = SystemClock.now();
    let completed_at = Rfc3339Timestamp::from_timestamp(started_at.as_timestamp().checked_add(
        std::time::Duration::from_millis(options.time_limit_milliseconds.value() + 1),
    )?);
    // Only wall time jumps. Real monotonic time leaves the real worker and independent
    // reviewer ample budget; the immutable wall deadline revokes acceptance at finalization.
    let service = HeadlessService::with_solver_registry(
        Arc::new(WallDeadlineAtCompletion {
            started_at,
            completed_at,
            started: std::sync::atomic::AtomicBool::new(false),
        }),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
        registry,
    )
    .map_err(boxed)?;
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let prepared = operation.prepare(snapshot(1)?).map_err(boxed)?;
    let outcome = prepared.execute(&mut Progress).await.map_err(boxed)?;
    assert_bound_runtime(&outcome, &runtime)?;
    // The backend solved the fixture, but revoked candidates cannot cross the service boundary.
    assert_eq!(outcome.execution.terminal_status, SolveStatus::Optimal);
    assert!(outcome.execution.selected_candidate.is_none());
    assert!(matches!(
        outcome.manifest().outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::NoSolutionWithinLimit
        }
    ));
    assert!(outcome.manifest().first_incumbent_milliseconds.is_some());
    assert!(
        outcome
            .manifest()
            .first_verified_feasible_milliseconds
            .is_none()
    );
    assert!(outcome.accepted_result().is_none());
    assert!(outcome.correctness_alarm().is_none());
    assert!(outcome.into_portable().map_err(boxed)?.is_none());
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn accepted_insert_rollback_finalizes_failed_run_and_retry_does_not_redispatch() -> TestResult
{
    let directory = tempfile::Builder::new()
        .prefix("eutheto-headless-accepted-rollback-")
        .tempdir_in(dirs::home_dir().ok_or("platform home directory is unavailable")?)?;
    let cancellation = CancellationToken::new();
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("eutheto.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: cancellation.clone(),
    };
    let solvers = Arc::new(real_registry().await?.0);
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    store
        .create_project(NewProject {
            document: original.document.clone(),
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        packs,
        solvers,
    );
    let request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: original.revision,
        options: options()?,
    };
    // This failpoint is reached only after a real accepted result enters the transaction.
    // Rollback is known, so it must not be reported as an uncertain finalization or leave
    // the request running. No synthetic accepted record bypasses the real solve/reviewer.
    store.set_failpoint(eutheto_store::Failpoint::AfterAcceptedSolutionInsert)?;
    let failure = Box::pin(app.solve_stored(request.clone(), &mut Progress)).await;
    let Err(StoredSolveFailure::AfterStart {
        run_id,
        cleanup: Ok(manifest),
        ..
    }) = failure
    else {
        return Err(format!(
            "expected known rollback with successful terminal cleanup: {failure:?}"
        )
        .into());
    };
    assert!(matches!(
        manifest.outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendFailed
        }
    ));
    assert!(store.list_accepted_results(scenario_id).await?.is_empty());
    let retained = store
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("rollback cleanup lost the request")?;
    assert_eq!(retained.input.run_id, run_id);
    assert_eq!(retained.manifest.as_ref(), Some(manifest.as_ref()));

    // Cancellation would prevent a fresh solve. A retry must instead expose the retained
    // terminal failure without redispatch, acquiring ownership, or creating accepted rows.
    cancellation.cancel();
    let replay = Box::pin(app.solve_stored(request, &mut Progress))
        .await
        .map_err(boxed)?;
    assert!(replay.reused);
    assert!(replay.report.is_none());
    assert_eq!(replay.state(), &retained);
    assert!(replay.accepted_result_id().is_none());
    assert!(replay.portable_result().is_none());
    assert!(store.list_accepted_results(scenario_id).await?.is_empty());
    let current = store.get_project(scenario_id).await?;
    assert_eq!(current.document, original.document);
    assert_eq!(current.summary.revision, original.revision);
    Ok(())
}

#[derive(Clone, Copy)]
enum WorkerFault {
    FailAfterRealOutcome,
    ClearAssignmentBooleans,
}

// The proxy never fabricates metadata, a candidate, or a successful terminal outcome.
// All worker authority comes from the approved backend; only the named fault is injected.
struct FaultedRealWorker {
    inner: Arc<dyn SolverBackend>,
    fault: WorkerFault,
    invocations: Arc<AtomicUsize>,
}

impl SolverBackend for FaultedRealWorker {
    fn descriptor(&self) -> &SolverDescriptor {
        self.inner.descriptor()
    }

    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        self.inner.runtime_identity()
    }

    fn compatibility(
        &self,
        problem: &PlanningProblemSummary,
        options: &SolveOptions,
    ) -> CompatibilityReport {
        self.inner.compatibility(problem, options)
    }

    fn solve<'a>(
        &'a self,
        request: &'a SolveRequest,
        output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(async move {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            match self.fault {
                WorkerFault::FailAfterRealOutcome => {
                    self.inner.solve(request, output).await?;
                    Err(BackendError::new(
                        "tests.after_real_candidate",
                        "Injected failure after the approved worker completed its real solve.",
                    )?)
                }
                WorkerFault::ClearAssignmentBooleans => {
                    self.inner
                        .solve(request, &mut ClearedAssignmentOutput { inner: output })
                        .await
                }
            }
        })
    }
}

struct ClearedAssignmentOutput<'a> {
    inner: &'a mut dyn BackendOutputSink,
}

impl BackendOutputSink for ClearedAssignmentOutput<'_> {
    fn emit_progress(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        self.inner.emit_progress(event)
    }

    fn submit_candidate(
        &mut self,
        mut candidate: CandidateSubmission,
    ) -> Result<BackendCandidate, OutputError> {
        // Keep the real typed variable set, integer bounds, objective evidence and sequence.
        // Boolean false is a valid domain value, but selecting nobody violates source coverage.
        for value in candidate.values.booleans.values_mut() {
            *value = false;
        }
        self.inner.submit_candidate(candidate)
    }
}

async fn faulted_real_registry(
    fault: WorkerFault,
) -> TestResult<(SolverRegistry, Arc<AtomicUsize>)> {
    let (registry, runtime) = real_registry().await?;
    let inner = Arc::clone(
        registry
            .get(runtime.backend_id())
            .ok_or("missing real worker")?,
    );
    let invocations = Arc::new(AtomicUsize::new(0));
    let backend: Arc<dyn SolverBackend> = Arc::new(FaultedRealWorker {
        inner,
        fault,
        invocations: Arc::clone(&invocations),
    });
    Ok((
        SolverRegistry::new(registry.matrix().clone(), [backend])?,
        invocations,
    ))
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn real_candidate_without_terminal_runtime_evidence_cannot_escape_as_accepted() -> TestResult
{
    let (registry, invocations) = faulted_real_registry(WorkerFault::FailAfterRealOutcome).await?;
    let service = HeadlessService::with_solver_registry(
        Arc::new(SystemClock),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
        registry,
    )
    .map_err(boxed)?;
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let prepared = operation.prepare(snapshot(1)?).map_err(boxed)?;
    let outcome = prepared.execute(&mut Progress).await.map_err(boxed)?;
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    let [attempt] = outcome.execution.attempts.as_slice() else {
        return Err("expected exactly one delegated real-worker attempt".into());
    };
    assert!(attempt.candidate_count > 0);
    assert_eq!(
        attempt.backend_failure_code.as_deref(),
        Some("tests.after_real_candidate")
    );
    assert!(attempt.outcome.is_none());
    assert!(matches!(
        outcome.manifest().outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::BackendFailed
        }
    ));
    assert!(outcome.manifest().first_incumbent_milliseconds.is_some());
    assert!(
        outcome
            .manifest()
            .first_verified_feasible_milliseconds
            .is_none()
    );
    assert!(outcome.execution.selected_candidate.is_none());
    assert!(outcome.accepted_result().is_none());
    assert!(outcome.correctness_alarm().is_none());
    assert!(outcome.into_portable().map_err(boxed)?.is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn real_worker_coverage_mutation_is_independently_quarantined_and_retained() -> TestResult {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-headless-worker-quarantine-")
        .tempdir_in(dirs::home_dir().ok_or("platform home directory is unavailable")?)?;
    let cancellation = CancellationToken::new();
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("eutheto.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: cancellation.clone(),
    };
    let (registry, invocations) =
        faulted_real_registry(WorkerFault::ClearAssignmentBooleans).await?;
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    store
        .create_project(NewProject {
            document: original.document.clone(),
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        packs,
        Arc::new(registry),
    );
    let request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: original.revision,
        options: options()?,
    };
    let outcome = Box::pin(app.solve_stored(request.clone(), &mut Progress))
        .await
        .map_err(boxed)?;
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    let manifest = outcome
        .state()
        .manifest
        .as_ref()
        .ok_or("quarantine was not finalized")?;
    // This exact category proves the mutation reached independent original-domain coverage
    // verification, rather than failing a generic candidate/output contract or a clock check.
    assert!(matches!(
        &manifest.outcome,
        RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }
            if diagnostic_code == "verification.required_rule_rejected"
    ));
    assert!(manifest.first_incumbent_milliseconds.is_some());
    assert!(manifest.first_verified_feasible_milliseconds.is_none());
    assert!(outcome.accepted_result_id().is_none());
    assert!(outcome.portable_result().is_none());
    assert!(store.list_accepted_results(scenario_id).await?.is_empty());
    let retained = store
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("quarantine lost the durable request")?;
    assert_eq!(&retained, outcome.state());
    let before_retry = invocations.load(Ordering::SeqCst);
    cancellation.cancel();
    let replay = Box::pin(app.solve_stored(request, &mut Progress))
        .await
        .map_err(boxed)?;
    assert!(replay.reused);
    assert_eq!(replay.state(), &retained);
    assert_eq!(invocations.load(Ordering::SeqCst), before_retry);
    assert!(replay.report.is_none());
    assert!(replay.accepted_result_id().is_none());
    assert!(store.list_accepted_results(scenario_id).await?.is_empty());
    Ok(())
}
