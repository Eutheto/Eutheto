#![forbid(unsafe_code)]

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

#[path = "headless_worker/repair.rs"]
mod repair;

#[path = "headless_worker/alternatives.rs"]
mod alternatives;

#[path = "headless_worker/recipient_report.rs"]
mod recipient_report;

use eutheto_core::StoredSolveFailure;
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    COUNTERFACTUAL_API_SCHEMA_VERSION, DIAGNOSTIC_COPY_API_SCHEMA_VERSION,
    DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION, DiagnosticCopyRequestV1, DiagnosticExportRequestV1,
    DiagnosticReport, EuthetoApp, HeadlessService, HeadlessSolveOutcome,
    SOLUTION_API_SCHEMA_VERSION, SOLUTION_COMPARE_SCHEMA_VERSION, SolutionCompareRequestV3,
    SolutionExplainRequestV1, SolutionStartCounterfactualRequestV1, SolutionVerifyRequestV1,
    SolutionViewRequestV1, StoredObjectiveSelectionV1, StoredSolveOutcome, StoredSolveRequest,
};
use eutheto_domain_api::{DomainPack, DomainPackRegistry};
use eutheto_domain_ir::{
    AUTHORED_OBJECTIVE_PROFILE_ID, AcceptedResult, AcceptedResultRefV1, AssignmentValue,
    ComparisonOrdering, CounterfactualConditionPayloadV1, CounterfactualJobState,
    CounterfactualResult, DiagnosticNativeProfileV1, ExplanationEvidencePayloadV2, ExplanationKind,
    ExplanationRequestSubjectV1, ExplanationRequestV1, InfeasibilityEvidenceV1,
    PortableAcceptedResult, RunBaselineOriginal, RunInfeasibilityEvidenceV1, RunManifest,
    RunNativeAttemptTermination, RunNativeRouterReason, RunObjectiveOperation,
    RunTerminalOutcomeV1, ScoreComparisonV2, SolutionComparisonV2, VerificationContextV1,
    VerificationReport, VerifiedCheckpoint,
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
    CommandSource, DomainCommandEnvelope, DurationMillis, EntityId, ExplanationMode,
    OperationControl, OperationId, PersonId, PreservationPolicy, ReproducibilityMode, RequestId,
    ResourceLimits, Revision, Rfc3339Timestamp, ScenarioCommand, ScenarioDocument, ScenarioId,
    ScenarioSnapshotV1, SolutionId, SolveMode, SolveOptions, SolveRunId, SolveStatus, SystemClock,
    SystemIdGenerator, SystemMonotonicClock, ValidationSeverity, WorkerThreadPolicy,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::BTreeSet,
    error::Error,
    fmt::Debug,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Notify;
use workforce_fixture::id;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: impl Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

fn diagnostic_failure(message: &str, evidence: &InfeasibilityEvidenceV1) -> Box<dyn Error> {
    match evidence {
        InfeasibilityEvidenceV1::Unavailable { reason } => {
            std::io::Error::other(format!("{message}; safe unavailable reason: {reason:?}")).into()
        }
        InfeasibilityEvidenceV1::Conflict { .. } => {
            std::io::Error::other(message.to_owned()).into()
        }
    }
}

fn attach_unadmitted_manual_selection(
    document: &mut ScenarioDocument,
) -> TestResult<eutheto_workforce::model::ManualSelection> {
    let checksum = "a".repeat(64);
    let base_id = EntityId::new(&SystemIdGenerator)?;
    let manual_id = EntityId::new(&SystemIdGenerator)?;
    let solution_id = SolutionId::new(&SystemIdGenerator)?;
    let accepted_result = AcceptedResultRefV1 {
        solution_id,
        result_checksum: checksum.clone(),
    };
    let assignments = vec![eutheto_workforce::model::AssignmentPair {
        person_id: id(1).parse()?,
        shift_id: id(8).parse()?,
    }];
    let baseline = eutheto_workforce::model::BaseSchedule {
        id: base_id,
        source_solution_id: solution_id,
        source_revision: Revision::INITIAL,
        assignments: assignments.clone(),
    };
    let original = RunBaselineOriginal {
        entity: eutheto_workforce::assignment_rules::workforce_baseline_reference(base_id, false)?,
        pair_content_hash: eutheto_workforce::assignment_rules::workforce_pair_content_hash(
            &assignments,
            None,
        )?,
        accepted_result,
        source_scenario_id: document.scenario_id,
        source_revision: Revision::INITIAL.value(),
        source_document_hash: checksum.clone(),
        source_run_id: SolveRunId::new(&SystemIdGenerator)?,
        source_run_input_checksum: checksum.clone(),
        source_run_manifest_checksum: checksum.clone(),
    };
    let mut manual = eutheto_workforce::model::ManualSelection {
        schema_version: 1,
        id: manual_id,
        original,
        admission_source_scenario_id: document.scenario_id,
        admission_source_revision: Revision::INITIAL,
        admission_source_document_hash: checksum,
        admission_command_id: CommandId::new(&SystemIdGenerator)?,
        assignments,
        checksum: String::new(),
    };
    manual.checksum =
        eutheto_workforce::assignment_rules::workforce_manual_selection_checksum(&manual, None)?;
    document.domain.entities.insert(
        base_id,
        serde_json::to_value(eutheto_workforce::model::WorkforceEntity::BaseSchedule(
            baseline,
        ))?,
    );
    document.domain.entities.insert(
        manual_id,
        serde_json::to_value(eutheto_workforce::model::WorkforceEntity::ManualSelection(
            manual.clone(),
        ))?,
    );
    Ok(manual)
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
fn foundational_snapshot() -> TestResult<ScenarioSnapshotV1> {
    let mut value = serde_json::to_value(workforce_fixture::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("missing fixture entities")?
        .remove(&id(6));
    value["domain"]["rules"] = json!({});
    value["domain"]["lockedAssignments"][id(14)]["shiftId"] = json!(id(8));
    value["domain"]["entities"][id(1)]["activeRange"] = json!({
        "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
    });
    let shift = &mut value["domain"]["entities"][id(8)];
    shift["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z","local":"2026-11-01T08:00:00","offsetSeconds":0
    });
    shift["endsAt"] = json!({
        "instant":"2026-11-01T10:00:00Z","local":"2026-11-01T10:00:00","offsetSeconds":0
    });
    Ok(ScenarioSnapshotV1::current(
        Revision::INITIAL,
        serde_json::from_value(value)?,
        BTreeSet::default(),
    ))
}

fn feasible_no_assumption_snapshot() -> TestResult<ScenarioSnapshotV1> {
    let mut snapshot = snapshot(1)?;
    snapshot.document.domain.rules.clear();
    Ok(snapshot)
}

fn many_fixed_snapshot(remove_fixed: Option<usize>) -> TestResult<ScenarioSnapshotV1> {
    let mut value = serde_json::to_value(workforce_fixture::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["lockedAssignments"] = json!({});
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("missing fixture entities")?
        .remove(&id(6));
    let mut base_shift = value["domain"]["entities"][id(8)].clone();
    base_shift["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z","local":"2026-11-01T08:00:00","offsetSeconds":0
    });
    base_shift["endsAt"] = json!({
        "instant":"2026-11-01T10:00:00Z","local":"2026-11-01T10:00:00","offsetSeconds":0
    });
    value["domain"]["entities"][id(8)] = base_shift.clone();
    let mut shift_ids = Vec::with_capacity(17);
    shift_ids.push(id(8));
    for index in 1..17_u32 {
        let shift_id = id(100 + index);
        let mut shift = base_shift.clone();
        shift["id"] = json!(shift_id);
        value["domain"]["entities"]
            .as_object_mut()
            .ok_or("missing fixture entities")?
            .insert(shift_id.clone(), shift);
        shift_ids.push(shift_id);
    }
    value["domain"]["rules"] = json!({});
    for (index, shift_id) in shift_ids.iter().enumerate() {
        if remove_fixed == Some(index) {
            continue;
        }
        let rule_id = id(300 + u32::try_from(index)?);
        value["domain"]["rules"][rule_id.clone()] = json!({
            "id":rule_id, "kind":"fixedAssignment", "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "personId":id(1), "shiftId":shift_id,
            "assigned":true
        });
    }
    let maximum_id = id(400);
    value["domain"]["rules"][maximum_id.clone()] = json!({
        "id":maximum_id, "kind":"maximumAssignmentCount", "active":true, "strength":"required",
        "scope":{"people":{"kind":"all"}}, "calendarId":id(2), "maximum":16
    });
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
fn options_with_explanation(mode: ExplanationMode) -> TestResult<SolveOptions> {
    let mut options = options()?;
    options.explanation_mode = mode;
    Ok(options)
}

async fn stored_real_run(
    document: ScenarioSnapshotV1,
    explanation_mode: ExplanationMode,
) -> TestResult<(
    tempfile::TempDir,
    Arc<SqliteScenarioStore>,
    StoredSolveOutcome,
    BackendRuntimeIdentity,
    EuthetoApp,
)> {
    stored_real_run_with_options(document, options_with_explanation(explanation_mode)?, None).await
}

async fn stored_real_run_with_options(
    document: ScenarioSnapshotV1,
    options: SolveOptions,
    objective_selection: Option<StoredObjectiveSelectionV1>,
) -> TestResult<(
    tempfile::TempDir,
    Arc<SqliteScenarioStore>,
    StoredSolveOutcome,
    BackendRuntimeIdentity,
    EuthetoApp,
)> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-headless-diagnostic-")
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
    let (registry, runtime) = real_registry().await?;
    let solvers = Arc::new(registry);
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    store
        .create_project(NewProject {
            document: document.document.clone(),
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        Arc::clone(&packs),
        solvers,
    );
    let request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: document.document.scenario_id,
        expected_revision: Some(document.revision),
        options,
        objective_selection,
    };
    let outcome = Box::pin(app.solve_stored(request, &mut Progress))
        .await
        .map_err(boxed)?;
    Ok((directory, store, outcome, runtime, app))
}

async fn reopen_real_app(
    directory: &tempfile::TempDir,
) -> TestResult<(Arc<SqliteScenarioStore>, EuthetoApp)> {
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
    let (registry, _runtime) = real_registry().await?;
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        packs,
        Arc::new(registry),
    );
    Ok((store, app))
}

fn comparison_snapshot() -> TestResult<ScenarioSnapshotV1> {
    let mut snapshot = snapshot(1)?;
    let p0_key: EntityId = id(0).parse()?;
    let p1_key: EntityId = id(1).parse()?;
    let p1: eutheto_workforce::model::WorkforceEntity = serde_json::from_value(
        snapshot
            .document
            .domain
            .entities
            .get(&p1_key)
            .ok_or("missing canonical P1")?
            .clone(),
    )?;
    let eutheto_workforce::model::WorkforceEntity::Person(mut p1) = p1 else {
        return Err("canonical P1 changed kind".into());
    };
    "P1".clone_into(&mut p1.name);
    let mut p0 = p1.clone();
    p0.id = id(0).parse()?;
    "P0".clone_into(&mut p0.name);
    p0.external_id = Some("staff-00".to_owned());
    snapshot.document.domain.entities.insert(
        p1_key,
        serde_json::to_value(eutheto_workforce::model::WorkforceEntity::Person(
            p1.clone(),
        ))?,
    );
    snapshot.document.domain.entities.insert(
        p0_key,
        serde_json::to_value(eutheto_workforce::model::WorkforceEntity::Person(
            p0.clone(),
        ))?,
    );
    snapshot.document.domain.preferences.clear();
    let people = snapshot
        .document
        .domain
        .entities
        .values()
        .filter(|entity| entity["kind"] == json!("person"))
        .count();
    let shifts = snapshot
        .document
        .domain
        .entities
        .values()
        .filter(|entity| entity["kind"] == json!("shiftInstance"))
        .count();
    if people != 2
        || shifts != 1
        || !snapshot.document.domain.locked_assignments.is_empty()
        || p0.id >= p1.id
        || p0.eligible_assignment_type_ids != p1.eligible_assignment_type_ids
    {
        return Err("comparison fixture is not the canonical unlocked P0<P1 pair".into());
    }
    for (rule_id, expected_kind) in [
        (30, "eligibility"),
        (31, "availability"),
        (32, "coverage"),
        (33, "noOverlap"),
    ] {
        let rule = snapshot
            .document
            .domain
            .rules
            .get(&id(rule_id).parse()?)
            .ok_or("missing required comparison rule")?;
        if rule["kind"] != json!(expected_kind)
            || rule["active"] != json!(true)
            || rule["strength"] != json!("required")
        {
            return Err("comparison fixture rule is not active Required".into());
        }
    }
    Ok(snapshot)
}

fn authored_optimize_selection() -> TestResult<StoredObjectiveSelectionV1> {
    Ok(StoredObjectiveSelectionV1 {
        schema_version: 1,
        profile_id: eutheto_domain_ir::ObjectiveProfileId::new(AUTHORED_OBJECTIVE_PROFILE_ID)?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Optimize,
        baseline_source: eutheto_core::StoredBaselineSourceV1::Accepted,
        alternatives_capture_id: None,
    })
}

async fn solve_real_again(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    options: SolveOptions,
    objective_selection: StoredObjectiveSelectionV1,
) -> TestResult<StoredSolveOutcome> {
    let request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(revision),
        options,
        objective_selection: Some(objective_selection),
    };
    app.solve_stored(request, &mut Progress)
        .await
        .map_err(boxed)
}

fn selected_person(result: &AcceptedResult) -> TestResult<PersonId> {
    let mut selected = None;
    for assignment in &result.solution.assignments {
        let (pair, chosen) =
            eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment)
                .map_err(boxed)?;
        if chosen && selected.replace(pair.person_id).is_some() {
            return Err("worker selected more than one person for the sole shift".into());
        }
    }
    selected.ok_or_else(|| "worker did not cover the sole shift".into())
}

fn collect_selected_person(
    people: &mut BTreeSet<PersonId>,
    assignment: &eutheto_domain_ir::DomainAssignment,
) -> TestResult {
    let (pair, chosen) =
        eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment)
            .map_err(boxed)?;
    if chosen {
        people.insert(pair.person_id);
    }
    Ok(())
}

fn assert_comparison_person_delta(
    comparison: &SolutionComparisonV2,
    base_person: &PersonId,
    candidate_person: &PersonId,
) -> TestResult {
    let mut before_people = BTreeSet::new();
    let mut after_people = BTreeSet::new();
    for assignment in &comparison.assignments {
        match assignment {
            eutheto_domain_ir::AssignmentComparisonV1::Added { after } => {
                collect_selected_person(&mut after_people, after)?;
            }
            eutheto_domain_ir::AssignmentComparisonV1::Removed { before } => {
                collect_selected_person(&mut before_people, before)?;
            }
            eutheto_domain_ir::AssignmentComparisonV1::Changed { before, after } => {
                collect_selected_person(&mut before_people, before)?;
                collect_selected_person(&mut after_people, after)?;
            }
        }
    }
    assert_eq!(
        before_people,
        BTreeSet::from([*base_person]),
        "comparison semantic delta must retain P0 as the selected base person"
    );
    assert_eq!(
        after_people,
        BTreeSet::from([*candidate_person]),
        "comparison semantic delta must retain P1 as the selected candidate person"
    );
    Ok(())
}
fn score_rank(score: &eutheto_domain_ir::ScoreVector) -> TestResult<i64> {
    score
        .levels
        .iter()
        .find(|level| level.level_id.as_str() == "official.workforce.objective.assignment.rank")
        .map(|level| level.value)
        .ok_or_else(|| "missing stable assignment rank score level".into())
}

fn assert_comparable_rank(
    comparison: &SolutionComparisonV2,
    base_rank: i64,
    candidate_rank: i64,
    delta: i64,
    ordering: ComparisonOrdering,
) -> TestResult {
    comparison.validate()?;
    let ScoreComparisonV2::Comparable {
        base_score,
        candidate_score,
        score_levels,
        ordering: actual_ordering,
    } = &comparison.score
    else {
        return Err(format!("expected comparable score, got {:?}", comparison.score).into());
    };
    assert_eq!(*actual_ordering, ordering);
    assert_eq!(score_rank(base_score)?, base_rank);
    assert_eq!(score_rank(candidate_score)?, candidate_rank);
    let rank = score_levels
        .iter()
        .find(|level| level.level_id.as_str() == "official.workforce.objective.assignment.rank")
        .ok_or("missing rank delta")?;
    assert_eq!(
        (rank.before, rank.after, rank.delta),
        (base_rank, candidate_rank, delta)
    );
    Ok(())
}

async fn query_comparison(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    base_solution_id: SolutionId,
    candidate_solution_id: SolutionId,
) -> TestResult<eutheto_core::SolutionComparisonDtoV3> {
    let result = app
        .query(AppQuery::SolutionCompareV3(SolutionCompareRequestV3 {
            schema_version: SOLUTION_COMPARE_SCHEMA_VERSION,
            scenario_id,
            base_solution_id,
            candidate_solution_id,
        }))
        .await
        .map_err(boxed)?;
    match result {
        AppQueryResult::SolutionComparisonV3(comparison) => Ok(*comparison),
        other => Err(format!("unexpected current comparison result: {other:?}").into()),
    }
}

async fn query_explanation(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    source_revision: Revision,
    subject: ExplanationRequestSubjectV1,
) -> TestResult<eutheto_core::SolutionExplanationDtoV2> {
    let result = app
        .query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                request: ExplanationRequestV1::new(subject)?,
            },
            source_revision,
            control: OperationControl::Cancellation(CancellationToken::new()),
        })
        .await
        .map_err(boxed)?;
    match result {
        AppQueryResult::SolutionExplanation(explanation) => Ok(*explanation),
        other => Err(format!("unexpected current explanation result: {other:?}").into()),
    }
}

#[derive(Clone, Copy, Debug)]
enum UntrustedMetadataControl {
    UnknownCompilerVersion,
    MissingObjectiveContext,
    WrongObjectivePolicyDigest,
}

fn reseal_untrusted_source_metadata(
    source: &eutheto_store::StoredAcceptedResultV2,
    control: UntrustedMetadataControl,
) -> TestResult<PortableAcceptedResult> {
    let original_input = &source.portable.run_input;
    let original_manifest = &source.portable.run_manifest;
    let compiler_version = match control {
        UntrustedMetadataControl::UnknownCompilerVersion => "unknown".to_owned(),
        _ => original_input.compiler_version.clone(),
    };
    let objective_policy_hash = match control {
        UntrustedMetadataControl::WrongObjectivePolicyDigest => {
            let wrong =
                eutheto_domain_ir::blake3_hex(b"deliberately wrong current objective policy");
            assert_ne!(wrong, original_input.objective_policy_hash);
            wrong
        }
        _ => original_input.objective_policy_hash.clone(),
    };
    let objective_context = match control {
        UntrustedMetadataControl::MissingObjectiveContext => None,
        _ => original_input.objective_context.clone(),
    };
    let input = eutheto_domain_ir::RunInput::new(
        original_input.run_id,
        original_input.request_id,
        original_input.scenario_id,
        original_input.scenario_revision,
        original_input.snapshot_id,
        original_input.snapshot_document_hash.clone(),
        original_input.snapshot_created_at,
        original_input.pack_id.clone(),
        original_input.pack_schema_version,
        original_input.planning_ir_schema_version,
        compiler_version,
        original_input.application_version.clone(),
        original_input.backend_id.clone(),
        original_input.backend_version.clone(),
        original_input.adapter_version.clone(),
        original_input.worker_version.clone(),
        original_input.solver_version.clone(),
        original_input.protocol_major,
        original_input.protocol_minor,
        original_input.model_hash.clone(),
        objective_policy_hash,
        original_input.solve_options.clone(),
        original_input.scenario_timezone.clone(),
        original_input.temporary_condition_hash.clone(),
        objective_context,
    )
    .map_err(boxed)?;
    let observations = original_manifest
        .native_observations
        .as_ref()
        .ok_or("real accepted worker source omitted native observations")?;
    let observations = eutheto_domain_ir::RunNativeObservations::new(
        &input,
        observations.objectives.clone(),
        observations.terminal_status,
        observations.terminal_reason,
        observations.attempt.clone(),
    )
    .map_err(boxed)?;
    let manifest = RunManifest::new(
        input.run_id,
        input.checksum.clone(),
        original_manifest.outcome.clone(),
        original_manifest.started_at,
        original_manifest.finished_at,
        original_manifest.elapsed_milliseconds,
        original_manifest.first_incumbent_milliseconds,
        original_manifest.first_verified_feasible_milliseconds,
        original_manifest.phase_timings,
        original_manifest.verification_warnings.clone(),
    )
    .map_err(boxed)?
    .with_native_observations(Some(observations))
    .map_err(boxed)?;
    let portable = PortableAcceptedResult::new(
        input,
        manifest,
        source.portable.accepted_result.clone(),
        source.portable.evidence.clone(),
    )
    .map_err(boxed)?;
    portable.validate().map_err(boxed)?;
    Ok(portable)
}

fn replace_test_run_metadata(database: &Path, sources: &[&PortableAcceptedResult]) -> TestResult {
    let mut connection = rusqlite::Connection::open(database)?;
    let transaction = connection.transaction()?;
    for source in sources {
        source.validate().map_err(boxed)?;
        let input_json = serde_json::to_string(&source.run_input)?;
        let manifest_json = serde_json::to_string(&source.run_manifest)?;
        let changed = transaction.execute(
            "UPDATE solve_runs SET input_hash = ?1, run_input_json = ?2, run_manifest_json = ?3 WHERE id = ?4",
            rusqlite::params![
                source.run_input.request_hash.as_str(),
                input_json,
                manifest_json,
                source.run_input.run_id.to_string()
            ],
        )?;
        assert_eq!(changed, 1, "expected exactly one real solve-run row");
    }
    transaction.commit()?;
    Ok(())
}

fn assert_unavailable_controlled_comparison(
    comparison: &SolutionComparisonV2,
    control: UntrustedMetadataControl,
    base: (&AcceptedResultRefV1, &PortableAcceptedResult),
    candidate: (&AcceptedResultRefV1, &PortableAcceptedResult),
    candidate_revision: Revision,
    p0: &PersonId,
    p1: &PersonId,
) -> TestResult {
    let (base_reference, base_metadata) = base;
    let (candidate_reference, candidate_metadata) = candidate;
    comparison.validate().map_err(boxed)?;
    assert_eq!(comparison.base.accepted_result, *base_reference);
    assert_eq!(comparison.candidate.accepted_result, *candidate_reference);
    assert_eq!(comparison.base.scenario_revision, Revision::INITIAL.value());
    assert_eq!(
        comparison.candidate.scenario_revision,
        candidate_revision.value()
    );
    assert_eq!(comparison.runs.base.run_id, base_metadata.run_input.run_id);
    assert_eq!(
        comparison.runs.base.run_manifest_checksum,
        base_metadata.run_manifest.checksum
    );
    assert_eq!(
        comparison.runs.candidate.run_id,
        candidate_metadata.run_input.run_id
    );
    assert_eq!(
        comparison.runs.candidate.run_manifest_checksum,
        candidate_metadata.run_manifest.checksum
    );
    assert_comparison_person_delta(comparison, p0, p1)?;
    assert!(comparison.protections.is_empty());
    let ScoreComparisonV2::Unavailable { reason } = &comparison.score else {
        return Err(format!(
            "untrusted {control:?} metadata unexpectedly remained score-comparable"
        )
        .into());
    };
    assert_eq!(
        *reason,
        eutheto_domain_ir::ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch
    );
    Ok(())
}

fn assert_same_cost_fingerprint(
    base: &eutheto_store::StoredAcceptedResultV2,
    candidate: &eutheto_store::StoredAcceptedResultV2,
) -> TestResult {
    let prepared =
        eutheto_domain_ir::prepare_comparison_v2(&base.portable, &candidate.portable, None)
            .map_err(boxed)?;
    let enrichment = eutheto_workforce::WorkforcePack
        .comparison_enrichment(
            &base.document,
            &base.portable,
            &candidate.document,
            &candidate.portable,
            &prepared,
            None,
        )
        .map_err(boxed)?;
    let (base_context, candidate_context) = enrichment
        .scoring_contexts
        .ok_or("Workforce did not establish current scoring context")?;
    assert_eq!(base_context.producer_id, candidate_context.producer_id);
    assert_eq!(base_context.semantic_hash, candidate_context.semantic_hash);
    Ok(())
}

fn assert_authored_optimize_record(source: &eutheto_store::StoredAcceptedResultV2) -> TestResult {
    let input = &source.portable.run_input;
    assert_eq!(
        input.solve_options.worker_threads,
        WorkerThreadPolicy::Exact(1)
    );
    assert_eq!(input.solve_options.random_seed, 1);
    let context = input
        .objective_context
        .as_ref()
        .ok_or("ordinary source lacks its recorded objective context")?;
    assert_eq!(context.profile_id.as_str(), AUTHORED_OBJECTIVE_PROFILE_ID);
    assert_eq!(context.operation, RunObjectiveOperation::Optimize);
    assert_eq!(context.semantic_version, 1);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_comparison_preserves_rank_scope_after_feasibility_pruning() -> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let p0: PersonId = id(0).parse()?;
    let p1: PersonId = id(1).parse()?;
    let selection = authored_optimize_selection()?;
    let run_options = options_with_explanation(ExplanationMode::None)?;
    let (directory, store, base_outcome, runtime, app) = stored_real_run_with_options(
        original.clone(),
        run_options.clone(),
        Some(selection.clone()),
    )
    .await?;
    assert_stored_bound_runtime(&base_outcome, &runtime)?;
    let base_solution_id = base_outcome
        .accepted_result_id()
        .ok_or("real base worker result is missing")?;
    let base = store.load_accepted_result(base_solution_id).await?;
    assert_authored_optimize_record(&base)?;
    assert_eq!(selected_person(&base.portable.accepted_result)?, p0);
    assert_eq!(
        score_rank(&base.portable.accepted_result.verification.score)?,
        1
    );

    let p0_key: EntityId = id(0).parse()?;
    let original_person: eutheto_workforce::model::Person = match serde_json::from_value(
        original
            .document
            .domain
            .entities
            .get(&p0_key)
            .ok_or("missing original P0")?
            .clone(),
    )? {
        eutheto_workforce::model::WorkforceEntity::Person(person) => person,
        _ => return Err("original P0 changed kind".into()),
    };
    assert!(!original_person.eligible_assignment_type_ids.is_empty());
    let mut excluded_person = original_person.clone();
    // Workforce assignment authority checks `contains(assignment_type_id)`: an empty list
    // therefore removes every explicit type membership; it does not mean "all types".
    excluded_person.eligible_assignment_type_ids.clear();
    let excluded_value = serde_json::to_value(eutheto_workforce::model::WorkforceEntity::Person(
        excluded_person,
    ))?;
    let edit = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Rank-scope comparison regression".to_owned(),
                },
                source: CommandSource::Desktop,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity":excluded_value}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    let AppCommandResult::ScenarioCommand(edit) = edit else {
        return Err("eligibility change did not produce a native command receipt".into());
    };
    let edited_project = store.get_project(scenario_id).await?;
    let edited_person: eutheto_workforce::model::WorkforceEntity = serde_json::from_value(
        edited_project
            .document
            .domain
            .entities
            .get(&p0_key)
            .ok_or("edited P0 disappeared")?
            .clone(),
    )?;
    let eutheto_workforce::model::WorkforceEntity::Person(edited_person) = edited_person else {
        return Err("edited P0 changed kind".into());
    };
    assert!(edited_person.eligible_assignment_type_ids.is_empty());

    let candidate_outcome = solve_real_again(
        &app,
        scenario_id,
        edit.new_revision,
        run_options.clone(),
        selection.clone(),
    )
    .await?;
    assert_stored_bound_runtime(&candidate_outcome, &runtime)?;
    let candidate_solution_id = candidate_outcome
        .accepted_result_id()
        .ok_or("real candidate worker result is missing")?;
    let candidate = store.load_accepted_result(candidate_solution_id).await?;
    assert_authored_optimize_record(&candidate)?;
    assert_eq!(selected_person(&candidate.portable.accepted_result)?, p1);
    // P0 is no longer a feasible assignment, but remains in the original rank population:
    // P1's rank stays 2 instead of being renumbered to 1.
    assert_eq!(
        score_rank(&candidate.portable.accepted_result.verification.score)?,
        2
    );
    assert_ne!(
        base.portable.run_input.objective_policy_hash,
        candidate.portable.run_input.objective_policy_hash,
        "feasibility pruning lowers a different emitted objective policy"
    );
    assert_same_cost_fingerprint(&base, &candidate)?;
    assert_eq!(
        base.portable.run_input.objective_context,
        candidate.portable.run_input.objective_context
    );
    assert_eq!(
        base.portable.run_input.compiler_version,
        candidate.portable.run_input.compiler_version
    );
    assert_ne!(
        base.portable.run_input.run_id,
        candidate.portable.run_input.run_id
    );
    assert_eq!(base.portable.run_input.scenario_revision, 0);
    assert_eq!(
        candidate.portable.run_input.scenario_revision,
        edit.new_revision.value()
    );

    let base_reference = AcceptedResultRefV1::from_result(&base.portable.accepted_result)?;
    let candidate_reference =
        AcceptedResultRefV1::from_result(&candidate.portable.accepted_result)?;
    let query =
        query_comparison(&app, scenario_id, base_solution_id, candidate_solution_id).await?;
    assert_eq!(query.current_revision, edit.new_revision);
    assert_eq!(query.comparison.base.accepted_result, base_reference);
    assert_eq!(
        query.comparison.candidate.accepted_result,
        candidate_reference
    );
    assert_eq!(query.comparison.base.scenario_revision, 0);
    assert_eq!(
        query.comparison.candidate.scenario_revision,
        edit.new_revision.value()
    );
    assert_eq!(
        query.comparison.runs.base.run_id,
        base.portable.run_input.run_id
    );
    assert_eq!(
        query.comparison.runs.base.run_manifest_checksum,
        base.portable.run_manifest.checksum
    );
    assert_eq!(
        query.comparison.runs.candidate.run_id,
        candidate.portable.run_input.run_id
    );
    assert_eq!(
        query.comparison.runs.candidate.run_manifest_checksum,
        candidate.portable.run_manifest.checksum
    );
    assert_comparison_person_delta(&query.comparison, &p0, &p1)?;
    assert!(query.comparison.protections.is_empty());
    assert_comparable_rank(&query.comparison, 1, 2, 1, ComparisonOrdering::Worse)?;

    let difference = query_explanation(
        &app,
        scenario_id,
        edit.new_revision,
        ExplanationRequestSubjectV1::SolutionDifference {
            left: base_reference.clone(),
            right: candidate_reference.clone(),
        },
    )
    .await?;
    assert_eq!(difference.current_revision, edit.new_revision);
    assert_eq!(
        difference.scenario_revisions,
        [Revision::INITIAL, edit.new_revision]
    );
    difference.explanation.validate()?;
    assert_eq!(
        difference.explanation.evidence.evidence.kind(),
        ExplanationKind::SolutionDifference
    );
    let ExplanationEvidencePayloadV2::SolutionDifference {
        comparison: difference_comparison,
    } = &difference.explanation.evidence.evidence
    else {
        return Err("difference explanation omitted comparison evidence".into());
    };
    assert_eq!(difference_comparison.as_ref(), &query.comparison);
    assert_comparable_rank(difference_comparison, 1, 2, 1, ComparisonOrdering::Worse)?;

    let repair = query_explanation(
        &app,
        scenario_id,
        edit.new_revision,
        ExplanationRequestSubjectV1::Repair {
            current: candidate_reference.clone(),
            base: base_reference.clone(),
        },
    )
    .await?;
    assert_eq!(repair.current_revision, edit.new_revision);
    assert_eq!(
        repair.scenario_revisions,
        [edit.new_revision, Revision::INITIAL]
    );
    repair.explanation.validate()?;
    assert_eq!(
        repair.explanation.evidence.evidence.kind(),
        ExplanationKind::Repair
    );
    let ExplanationEvidencePayloadV2::Repair {
        repair: repair_evidence,
    } = &repair.explanation.evidence.evidence
    else {
        return Err("Repair explanation omitted its comparison evidence".into());
    };
    assert_eq!(
        repair_evidence.causality,
        eutheto_domain_ir::RepairCausalityV1::NotEstablished
    );
    assert_eq!(
        repair_evidence.comparison.runs.base.run_id,
        base.portable.run_input.run_id
    );
    assert_eq!(
        repair_evidence.comparison.runs.candidate.run_id,
        candidate.portable.run_input.run_id
    );
    assert_comparable_rank(
        &repair_evidence.comparison,
        1,
        2,
        1,
        ComparisonOrdering::Worse,
    )?;

    // These controls reseal only untrusted historical metadata around genuine worker outputs.
    // The accepted solution, source snapshot, score/report, worker attempt, and observed values
    // remain untouched; each check uses a reopened app with no live ordinary custody.
    drop(app);
    drop(store);
    drop(base_outcome);
    drop(candidate_outcome);
    let database = directory.path().join("eutheto.sqlite");
    for control in [
        UntrustedMetadataControl::UnknownCompilerVersion,
        UntrustedMetadataControl::MissingObjectiveContext,
        UntrustedMetadataControl::WrongObjectivePolicyDigest,
    ] {
        let base_metadata = reseal_untrusted_source_metadata(&base, control)?;
        let candidate_metadata = match control {
            UntrustedMetadataControl::MissingObjectiveContext => candidate.portable.clone(),
            _ => reseal_untrusted_source_metadata(&candidate, control)?,
        };
        match control {
            UntrustedMetadataControl::UnknownCompilerVersion => {
                assert_eq!(base_metadata.run_input.compiler_version, "unknown");
                assert_eq!(candidate_metadata.run_input.compiler_version, "unknown");
            }
            UntrustedMetadataControl::MissingObjectiveContext => {
                assert!(base_metadata.run_input.objective_context.is_none());
                assert_eq!(
                    candidate_metadata.run_input.objective_context,
                    candidate.portable.run_input.objective_context
                );
                assert!(candidate_metadata.run_input.objective_context.is_some());
            }
            UntrustedMetadataControl::WrongObjectivePolicyDigest => {
                assert_ne!(
                    base_metadata.run_input.objective_policy_hash,
                    base.portable.run_input.objective_policy_hash
                );
                assert_ne!(
                    candidate_metadata.run_input.objective_policy_hash,
                    candidate.portable.run_input.objective_policy_hash
                );
            }
        }
        replace_test_run_metadata(&database, &[&base_metadata, &candidate_metadata])?;
        let (control_store, control_app) = reopen_real_app(&directory).await?;
        let comparison = match control_app
            .query(AppQuery::SolutionCompareV3(SolutionCompareRequestV3 {
                schema_version: SOLUTION_COMPARE_SCHEMA_VERSION,
                scenario_id,
                base_solution_id,
                candidate_solution_id,
            }))
            .await
        {
            Ok(AppQueryResult::SolutionComparisonV3(comparison)) => Some(*comparison),
            Ok(other) => {
                return Err(format!("unexpected controlled comparison result: {other:?}").into());
            }
            Err(AppError::Verification(failure))
                if matches!(
                    failure.code.as_str(),
                    "solution.reverification_failed" | "solution.reverification_mismatch"
                ) =>
            {
                eprintln!(
                    "untrusted {control:?} metadata: public comparison refused at independent source verification ({})",
                    failure.code
                );
                None
            }
            Err(error) => return Err(boxed(error)),
        };
        let explanation = match control_app
            .query(AppQuery::SolutionExplain {
                request: SolutionExplainRequestV1 {
                    schema_version: SOLUTION_API_SCHEMA_VERSION,
                    scenario_id,
                    request: ExplanationRequestV1::new(
                        ExplanationRequestSubjectV1::SolutionDifference {
                            left: base_reference.clone(),
                            right: candidate_reference.clone(),
                        },
                    )?,
                },
                source_revision: edit.new_revision,
                control: OperationControl::Cancellation(CancellationToken::new()),
            })
            .await
        {
            Ok(AppQueryResult::SolutionExplanation(explanation)) => Some(*explanation),
            Ok(other) => {
                return Err(format!("unexpected controlled explanation result: {other:?}").into());
            }
            Err(AppError::Verification(failure))
                if matches!(
                    failure.code.as_str(),
                    "solution.reverification_failed" | "solution.reverification_mismatch"
                ) =>
            {
                eprintln!(
                    "untrusted {control:?} metadata: public difference explanation refused at independent source verification ({})",
                    failure.code
                );
                None
            }
            Err(error) => return Err(boxed(error)),
        };

        if let Some(comparison) = &comparison {
            assert_eq!(comparison.current_revision, edit.new_revision);
            assert_unavailable_controlled_comparison(
                &comparison.comparison,
                control,
                (&base_reference, &base_metadata),
                (&candidate_reference, &candidate_metadata),
                edit.new_revision,
                &p0,
                &p1,
            )?;
        }
        if let Some(explanation) = &explanation {
            explanation.explanation.validate().map_err(boxed)?;
            assert_eq!(explanation.current_revision, edit.new_revision);
            assert_eq!(
                explanation.scenario_revisions,
                [Revision::INITIAL, edit.new_revision]
            );
            let ExplanationEvidencePayloadV2::SolutionDifference {
                comparison: explained_comparison,
            } = &explanation.explanation.evidence.evidence
            else {
                return Err("controlled difference explanation omitted its comparison".into());
            };
            if let Some(comparison) = &comparison {
                assert_eq!(explained_comparison.as_ref(), &comparison.comparison);
            }
            assert_unavailable_controlled_comparison(
                explained_comparison,
                control,
                (&base_reference, &base_metadata),
                (&candidate_reference, &candidate_metadata),
                edit.new_revision,
                &p0,
                &p1,
            )?;
        }
        drop(control_app);
        drop(control_store);
    }
    replace_test_run_metadata(&database, &[&base.portable, &candidate.portable])?;
    let (store, app) = reopen_real_app(&directory).await?;

    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: edit.new_revision,
    })
    .await
    .map_err(boxed)?;
    let restored_project = store.get_project(scenario_id).await?;
    assert!(restored_project.summary.revision > edit.new_revision);
    assert_eq!(restored_project.document, original.document);
    let restored_revision = restored_project.summary.revision;
    let restored_person: eutheto_workforce::model::WorkforceEntity = serde_json::from_value(
        restored_project
            .document
            .domain
            .entities
            .get(&p0_key)
            .ok_or("restored P0 disappeared")?
            .clone(),
    )?;
    let eutheto_workforce::model::WorkforceEntity::Person(restored_person) = restored_person else {
        return Err("restored P0 changed kind".into());
    };
    assert_eq!(
        restored_person.eligible_assignment_type_ids,
        original_person.eligible_assignment_type_ids
    );
    let restored_outcome =
        solve_real_again(&app, scenario_id, restored_revision, run_options, selection).await?;
    assert_stored_bound_runtime(&restored_outcome, &runtime)?;
    let restored_solution_id = restored_outcome
        .accepted_result_id()
        .ok_or("real restored worker result is missing")?;
    let restored = store.load_accepted_result(restored_solution_id).await?;
    assert_authored_optimize_record(&restored)?;
    assert_eq!(selected_person(&restored.portable.accepted_result)?, p0);
    assert_eq!(
        score_rank(&restored.portable.accepted_result.verification.score)?,
        1
    );
    assert_eq!(
        base.portable.run_input.objective_policy_hash,
        restored.portable.run_input.objective_policy_hash
    );
    assert_same_cost_fingerprint(&base, &restored)?;

    let restored_query =
        query_comparison(&app, scenario_id, base_solution_id, restored_solution_id).await?;
    assert_eq!(restored_query.current_revision, restored_revision);
    assert_eq!(
        restored_query.comparison.base.accepted_result,
        base_reference
    );
    assert_eq!(
        restored_query.comparison.candidate.accepted_result,
        AcceptedResultRefV1::from_result(&restored.portable.accepted_result)?
    );
    assert_eq!(
        restored_query.comparison.candidate.scenario_revision,
        restored_revision.value()
    );
    assert_eq!(
        restored_query.comparison.runs.base.run_id,
        base.portable.run_input.run_id
    );
    assert_eq!(
        restored_query.comparison.runs.candidate.run_id,
        restored.portable.run_input.run_id
    );
    assert_eq!(
        restored_query
            .comparison
            .runs
            .candidate
            .run_manifest_checksum,
        restored.portable.run_manifest.checksum
    );
    assert_comparable_rank(
        &restored_query.comparison,
        1,
        1,
        0,
        ComparisonOrdering::Equivalent,
    )?;
    Ok(())
}

async fn review_manual(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    intent: eutheto_core::ManualEditIntentV1,
) -> Result<eutheto_core::ManualEditReviewV1, Box<dyn Error>> {
    let result = app
        .manual_edit(
            eutheto_core::ManualEditRequestV1::Review {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: revision,
                intent,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let eutheto_core::ManualEditResultV1::Review { review } = result else {
        return Err("manual review omitted review".into());
    };
    Ok(review)
}

async fn apply_reviewed_manual(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    intent: eutheto_core::ManualEditIntentV1,
    approval_digest: String,
    truncate_redo: bool,
) -> Result<eutheto_types::CommandResult, Box<dyn Error>> {
    let result = app
        .manual_edit(
            eutheto_core::ManualEditRequestV1::Apply {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: revision,
                intent,
                approval_digest,
                command_id: CommandId::new(&SystemIdGenerator)?,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Guided repair regression".to_owned(),
                },
                truncate_redo,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let eutheto_core::ManualEditResultV1::Applied { applied } = result else {
        return Err("manual apply omitted committed receipt".into());
    };
    assert!(applied.source_admission_failure.is_none());
    Ok(applied.command_result)
}

async fn edit_manual_source(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    command_type: &str,
    payload: Value,
) -> TestResult<eutheto_types::CommandResult> {
    let result = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: revision,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Guided source regression".to_owned(),
                },
                source: CommandSource::Desktop,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: command_type.to_owned(),
                    payload,
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    let AppCommandResult::ScenarioCommand(receipt) = result else {
        return Err("source edit omitted receipt".into());
    };
    Ok(receipt)
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_guided_manual_wide_eligibility_keeps_review_and_history_bounded()
-> TestResult {
    use eutheto_core::{ManualEditDispositionV1, ManualEditIntentV1, ManualProtectionV1};

    let mut original = comparison_snapshot()?;
    let assignment_type = original
        .document
        .domain
        .entities
        .get(&id(4).parse()?)
        .ok_or("missing canonical assignment type")?
        .clone();
    let mut eligible_types = vec![id(4)];
    for index in 0..63 {
        let key = id(1000 + index);
        let mut value = assignment_type.clone();
        value["id"] = json!(key);
        value["name"] = json!(format!("Matrix type {index:02}"));
        original
            .document
            .domain
            .entities
            .insert(key.parse()?, value);
        eligible_types.push(key);
    }
    for person in original.document.domain.entities.values_mut() {
        if person["kind"] == "person" {
            person["eligibleAssignmentTypeIds"] = json!(eligible_types);
        }
    }
    let person = original
        .document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("missing canonical person")?
        .clone();
    for index in 2..100 {
        let key = id(200 + index);
        let mut value = person.clone();
        value["id"] = json!(key);
        value["name"] = json!(format!("Matrix person {index:03}"));
        value["externalId"] = json!(format!("matrix-{index:03}"));
        original
            .document
            .domain
            .entities
            .insert(key.parse()?, value);
    }
    original
        .document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or("missing canonical shift")?["coverage"]["count"] = json!(2);
    assert_eq!(
        original
            .document
            .domain
            .entities
            .values()
            .filter(|value| value["kind"] == "person")
            .count(),
        100,
    );
    assert_eq!(
        original
            .document
            .domain
            .entities
            .values()
            .filter(|value| value["kind"] == "assignmentType")
            .count(),
        64,
    );
    let scenario_id = original.document.scenario_id;
    let mut solve_options = options_with_explanation(ExplanationMode::None)?;
    // Size the real worker's compilation request to this fixture; manual admission caps stay unchanged.
    solve_options.resource_limits.max_entities = 512;
    solve_options.resource_limits.max_variables = 512;
    solve_options.resource_limits.max_constraints = 512;
    let (directory, store, outcome, runtime, app) = stored_real_run_with_options(
        original,
        solve_options,
        Some(authored_optimize_selection()?),
    )
    .await
    .map_err(|error| format!("wide worker solve/store: {error:?}"))?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let accepted_id = outcome
        .accepted_result_id()
        .ok_or("missing accepted result")?;
    let accepted = store.load_accepted_result(accepted_id).await?;
    let original_bytes = serde_json::to_vec(&accepted.portable)?;
    let selected = accepted
        .portable
        .accepted_result
        .solution
        .assignments
        .iter()
        .map(eutheto_workforce::assignment_rules::decode_workforce_assignment)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(pair, selected)| selected.then_some(pair))
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 2);
    let AppCommandResult::ScenarioCommand(adopted) = app
        .execute(AppCommand::AdoptBaseSchedule(
            eutheto_core::SolutionAdoptBaseRequestV1 {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                result: AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Wide repair regression".to_owned(),
                },
                truncate_redo: false,
            },
        ))
        .await
        .map_err(|error| format!("wide base adoption: {error:?}"))?
    else {
        return Err("wide source adoption omitted receipt".into());
    };
    let pair = selected[0];
    let hard = ManualEditIntentV1::Protection {
        person_id: pair.person_id,
        shift_id: pair.shift_id,
        protection: ManualProtectionV1::Hard,
    };
    let source_document = store.get_project(scenario_id).await?.document;
    let reviewed = review_manual(&app, scenario_id, adopted.new_revision, hard.clone())
        .await
        .map_err(|error| format!("wide first Hard Review: {error:?}"))?;
    assert_eq!(reviewed.source.selected_count, 2);
    assert_eq!(reviewed.disposition, ManualEditDispositionV1::Valid);
    assert_eq!(
        reviewed.source.document_hash,
        blake3::hash(&serde_json::to_vec(&source_document)?)
            .to_hex()
            .to_string(),
    );
    let mut canonical_selection = selected.clone();
    canonical_selection.sort_unstable();
    let buffered_digest_meaning = (
        "eutheto/manual-edit/v1",
        &source_document,
        adopted.new_revision,
        &reviewed.source.original,
        &hard,
        &canonical_selection,
        vec![(
            pair,
            None::<eutheto_types::AssignmentId>,
            eutheto_workforce::model::LockState::Hard {},
        )],
    );
    assert_eq!(
        reviewed.approval_digest.as_deref(),
        Some(
            blake3::hash(&serde_json::to_vec(&buffered_digest_meaning)?)
                .to_hex()
                .to_string()
                .as_str()
        ),
    );
    let protected = apply_reviewed_manual(
        &app,
        scenario_id,
        adopted.new_revision,
        hard,
        reviewed
            .approval_digest
            .ok_or("wide hard review omitted digest")?,
        false,
    )
    .await
    .map_err(|error| format!("wide first Hard Apply: {error:?}"))?;
    let protected_document = store.get_project(scenario_id).await?.document;
    assert_eq!(protected_document.domain.locked_assignments.len(), 1);
    let mut repair_options = accepted.portable.run_input.solve_options.clone();
    repair_options.explanation_mode = ExplanationMode::Standard;
    let repaired = solve_real_again(
        &app,
        scenario_id,
        protected.new_revision,
        repair_options,
        StoredObjectiveSelectionV1 {
            schema_version: 1,
            profile_id: eutheto_domain_ir::ObjectiveProfileId::new(
                "official.workforce.objective.minimal-changes",
            )?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            baseline_source: eutheto_core::StoredBaselineSourceV1::Accepted,
            alternatives_capture_id: None,
        },
    )
    .await?;
    let repaired_id = repaired
        .accepted_result_id()
        .ok_or("missing protected Repair")?;
    let repaired = store.load_accepted_result(repaired_id).await?;
    let assignments = &repaired.portable.accepted_result.solution.assignments;
    let replaced = assignments
        .iter()
        .find(|assignment| {
            eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment)
                .is_ok_and(|(candidate, selected)| candidate == pair && selected)
        })
        .ok_or("missing protected assignment")?;
    let target = assignments
        .iter()
        .find(|assignment| {
            eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment)
                .is_ok_and(|(candidate, selected)| candidate.shift_id == pair.shift_id && !selected)
        })
        .ok_or("missing replacement candidate")?;
    let diagnostic = run_actual_conditional_worker(
        &app,
        &store,
        scenario_id,
        protected.new_revision,
        repaired_id,
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: target.id.clone(),
            replaced_assignment_id: replaced.id.clone(),
        },
    )
    .await?;
    assert_eq!(
        diagnostic.run_manifest.outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible,
        },
    );
    assert!(matches!(
        diagnostic.conclusion,
        eutheto_domain_ir::CounterfactualConclusionV1::ProvenImpossible
            | eutheto_domain_ir::CounterfactualConclusionV1::NotDistinguishedWithinBudget
    ));
    let unlocked = ManualEditIntentV1::Protection {
        person_id: pair.person_id,
        shift_id: pair.shift_id,
        protection: ManualProtectionV1::Unlocked,
    };
    // This review also authenticates the first complete, inline private receipt.
    let reviewed = review_manual(&app, scenario_id, protected.new_revision, unlocked.clone())
        .await
        .map_err(|error| format!("wide authenticated-B unlock Review: {error:?}"))?;
    assert_eq!(reviewed.disposition, ManualEditDispositionV1::Valid);
    let unprotected = apply_reviewed_manual(
        &app,
        scenario_id,
        protected.new_revision,
        unlocked,
        reviewed
            .approval_digest
            .ok_or("wide unlock review omitted digest")?,
        false,
    )
    .await
    .map_err(|error| format!("wide authenticated-B unlock Apply: {error:?}"))?;
    assert!(
        store
            .get_project(scenario_id)
            .await?
            .document
            .domain
            .locked_assignments
            .is_empty()
    );
    drop(app);
    let (reopened_store, reopened) = reopen_real_app(&directory)
        .await
        .map_err(|error| format!("wide app reopen: {error:?}"))?;
    let AppCommandResult::ScenarioCommand(undone) = reopened
        .execute(AppCommand::Undo {
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: unprotected.new_revision,
        })
        .await
        .map_err(|error| format!("wide reopened Undo: {error:?}"))?
    else {
        return Err("wide native Undo omitted receipt".into());
    };
    assert_eq!(
        reopened_store
            .get_project(scenario_id)
            .await?
            .document
            .domain
            .locked_assignments,
        protected_document.domain.locked_assignments,
    );
    reopened
        .execute(AppCommand::Redo {
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: undone.new_revision,
        })
        .await
        .map_err(|error| format!("wide reopened Redo: {error:?}"))?;
    assert!(
        reopened_store
            .get_project(scenario_id)
            .await?
            .document
            .domain
            .locked_assignments
            .is_empty()
    );
    assert_eq!(
        serde_json::to_vec(
            &reopened_store
                .load_accepted_result(accepted_id)
                .await?
                .portable
        )?,
        original_bytes,
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_manual_opaque_source_preserves_admitted_history() -> TestResult {
    use eutheto_core::{ManualEditDispositionV1, ManualEditIntentV1, ManualProtectionV1};

    for chunks in [75, 100] {
        let mut original = comparison_snapshot()?;
        original.document.extensions.insert(
            "nonsemantic.org.eutheto.test.opaque".to_owned(),
            json!(vec!["x".repeat(8192); chunks]),
        );
        let scenario_id = original.document.scenario_id;
        let (directory, store, outcome, _, app) = stored_real_run_with_options(
            original,
            options_with_explanation(ExplanationMode::None)?,
            Some(authored_optimize_selection()?),
        )
        .await
        .map_err(|error| format!("opaque source solve: {error:?}"))?;
        let accepted_id = outcome
            .accepted_result_id()
            .ok_or("missing opaque source result")?;
        let accepted = store.load_accepted_result(accepted_id).await?;
        let accepted_bytes = serde_json::to_vec(&accepted.portable)?;
        let person_id = selected_person(&accepted.portable.accepted_result)?;
        let shift_id = id(8).parse()?;
        let AppCommandResult::ScenarioCommand(adopted) = app
            .execute(AppCommand::AdoptBaseSchedule(
                eutheto_core::SolutionAdoptBaseRequestV1 {
                    schema_version: 1,
                    request_id: RequestId::new(&SystemIdGenerator)?,
                    command_id: CommandId::new(&SystemIdGenerator)?,
                    scenario_id,
                    expected_revision: Revision::INITIAL,
                    result: AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?,
                    actor: ActorRef {
                        actor_id: None,
                        display_name: "Opaque source replay regression".to_owned(),
                    },
                    truncate_redo: false,
                },
            ))
            .await
            .map_err(|error| format!("opaque source adoption: {error:?}"))?
        else {
            return Err("opaque source base adoption omitted receipt".into());
        };
        let mut revision = adopted.new_revision;
        let mut saved = vec![store.get_project(scenario_id).await?.document];
        for protection in [ManualProtectionV1::Hard, ManualProtectionV1::Unlocked] {
            let intent = ManualEditIntentV1::Protection {
                person_id,
                shift_id,
                protection,
            };
            let review = review_manual(&app, scenario_id, revision, intent.clone())
                .await
                .map_err(|error| format!("opaque source Review: {error:?}"))?;
            assert_eq!(review.disposition, ManualEditDispositionV1::Valid);
            match apply_reviewed_manual(
                &app,
                scenario_id,
                revision,
                intent,
                review
                    .approval_digest
                    .ok_or("opaque review omitted approval")?,
                false,
            )
            .await
            {
                Ok(applied) => {
                    revision = applied.new_revision;
                    saved.push(store.get_project(scenario_id).await?.document);
                }
                Err(error) => {
                    assert!(
                        error.to_string().contains("operation.resource_limit"),
                        "{error}"
                    );
                    let unchanged = store.get_project(scenario_id).await?;
                    assert_eq!(unchanged.summary.revision, revision);
                    assert_eq!(
                        &unchanged.document,
                        saved.last().ok_or("missing prior source")?
                    );
                    break;
                }
            }
        }
        if chunks == 75 {
            assert_eq!(
                saved.len(),
                3,
                "The 600 KiB source must admit both protection changes"
            );
        }
        drop(app);
        let (reopened_store, reopened) = reopen_real_app(&directory).await?;
        for expected in saved.iter().rev().skip(1) {
            let AppCommandResult::ScenarioCommand(undone) = reopened
                .execute(AppCommand::Undo {
                    request_id: RequestId::new(&SystemIdGenerator)?,
                    scenario_id,
                    expected_revision: revision,
                })
                .await
                .map_err(|error| format!("opaque source reopened Undo: {error:?}"))?
            else {
                return Err("opaque source Undo omitted receipt".into());
            };
            revision = undone.new_revision;
            assert_eq!(
                &reopened_store.get_project(scenario_id).await?.document,
                expected
            );
        }
        for expected in saved.iter().skip(1) {
            let AppCommandResult::ScenarioCommand(redone) = reopened
                .execute(AppCommand::Redo {
                    request_id: RequestId::new(&SystemIdGenerator)?,
                    scenario_id,
                    expected_revision: revision,
                })
                .await
                .map_err(|error| format!("opaque source reopened Redo: {error:?}"))?
            else {
                return Err("opaque source Redo omitted receipt".into());
            };
            revision = redone.new_revision;
            assert_eq!(
                &reopened_store.get_project(scenario_id).await?.document,
                expected
            );
        }
        assert_eq!(
            serde_json::to_vec(
                &reopened_store
                    .load_accepted_result(accepted_id)
                    .await?
                    .portable
            )?,
            accepted_bytes,
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_guided_manual_review_protects_atomic_replacement_and_reopen_undo()
-> TestResult {
    use eutheto_core::{
        ManualEditDispositionV1, ManualEditIntentV1, ManualProtectionChangeV1, ManualProtectionV1,
    };
    let mut original = comparison_snapshot()?;
    original.document.domain.entities.remove(&id(8).parse()?);
    let mut recurring = workforce_fixture::fixture()?
        .domain
        .entities
        .remove(&id(6).parse()?)
        .ok_or("missing genuine recurring fixture template")?;
    recurring["timing"] =
        json!({"kind": "elapsedDuration", "startTime": "08:00:00", "durationMinutes": 120});
    recurring["coverage"] = json!({"kind": "atLeast", "minimum": 1, "qualificationMinimums": []});
    original
        .document
        .domain
        .entities
        .insert(id(6).parse()?, recurring.clone());
    recurring["id"] = json!(id(60));
    recurring["timing"] =
        json!({"kind": "elapsedDuration", "startTime": "12:00:00", "durationMinutes": 60});
    recurring["coverage"] = json!({"kind": "atLeast", "minimum": 0, "qualificationMinimums": []});
    recurring["occurrenceIdentities"] =
        json!({(id(61)): {"id": id(61), "localStartDate": "2026-11-01"}});
    original
        .document
        .domain
        .entities
        .insert(id(60).parse()?, recurring);
    let scenario_id = original.document.scenario_id;
    let (directory, store, outcome, runtime, app) = stored_real_run_with_options(
        original,
        options_with_explanation(ExplanationMode::None)?,
        Some(authored_optimize_selection()?),
    )
    .await?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let accepted_id = outcome
        .accepted_result_id()
        .ok_or("missing original accepted result")?;
    let accepted = store.load_accepted_result(accepted_id).await?;
    let original_bytes = serde_json::to_vec(&accepted.portable)?;
    let reference = AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?;
    let AppCommandResult::ScenarioCommand(adopted) = app
        .execute(AppCommand::AdoptBaseSchedule(
            eutheto_core::SolutionAdoptBaseRequestV1 {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                result: reference.clone(),
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Guided regression".to_owned(),
                },
                truncate_redo: false,
            },
        ))
        .await
        .map_err(boxed)?
    else {
        return Err("adoption omitted receipt".into());
    };
    let incumbent = selected_person(&accepted.portable.accepted_result)?;
    let shift = accepted
        .portable
        .accepted_result
        .solution
        .assignments
        .iter()
        .find_map(|assignment| {
            eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment)
                .ok()
                .filter(|(_, selected)| *selected)
                .map(|(pair, _)| pair.shift_id)
        })
        .ok_or("missing original shift")?;
    assert_eq!(shift, id(7).parse()?);
    assert!(
        !store
            .get_project(scenario_id)
            .await?
            .document
            .domain
            .entities
            .contains_key(&shift.as_entity_id())
    );
    let target = id(1).parse()?;
    assert_ne!(incumbent, target);
    let read = app
        .manual_edit(
            eutheto_core::ManualEditRequestV1::Read {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: adopted.new_revision,
                offset: 0,
                limit: 1,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let eutheto_core::ManualEditResultV1::Read { read } = read else {
        return Err("read omitted page".into());
    };
    assert_eq!(read.source.original.accepted_result, reference);
    assert_eq!(read.rows.len(), 1);
    assert_eq!(read.source.selected_count, 1);
    assert!(read.rows[0].person_available && read.rows[0].shift_available);
    assert!(read.rows[0].shift_label.contains("2026-11-01T08:00:00"));
    let present_add = ManualEditIntentV1::Add {
        person_id: incumbent,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    assert!(
        review_manual(&app, scenario_id, adopted.new_revision, present_add)
            .await
            .is_err()
    );
    let unknown = ManualEditIntentV1::Add {
        person_id: id(99).parse()?,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    assert!(
        review_manual(&app, scenario_id, adopted.new_revision, unknown)
            .await
            .is_err()
    );
    let stale_stored_id = ManualEditIntentV1::Add {
        person_id: target,
        shift_id: id(8).parse()?,
        protection_changes: Vec::new(),
    };
    assert!(
        review_manual(&app, scenario_id, adopted.new_revision, stale_stored_id)
            .await
            .is_err()
    );
    let missing_remove = ManualEditIntentV1::Remove {
        person_id: target,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    assert!(
        review_manual(&app, scenario_id, adopted.new_revision, missing_remove)
            .await
            .is_err()
    );
    let empty_weight = ManualEditIntentV1::Protection {
        person_id: incumbent,
        shift_id: shift,
        protection: ManualProtectionV1::Soft {
            stability_weight: 0,
        },
    };
    assert!(
        review_manual(&app, scenario_id, adopted.new_revision, empty_weight)
            .await
            .is_err()
    );
    let oversized = eutheto_core::ManualEditRequestV1::Apply {
        schema_version: 1,
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: adopted.new_revision,
        intent: ManualEditIntentV1::Protection {
            person_id: incumbent,
            shift_id: shift,
            protection: ManualProtectionV1::Hard,
        },
        approval_digest: "x".repeat(eutheto_core::MAX_MANUAL_EDIT_REQUEST_BYTES),
        command_id: CommandId::new(&SystemIdGenerator)?,
        actor: ActorRef {
            actor_id: None,
            display_name: "Bounds".to_owned(),
        },
        truncate_redo: false,
    };
    assert!(
        app.manual_edit(
            oversized,
            OperationControl::Cancellation(CancellationToken::new())
        )
        .await
        .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        matches!(app.manual_edit(eutheto_core::ManualEditRequestV1::Read {
        schema_version: 1, request_id: RequestId::new(&SystemIdGenerator)?, scenario_id,
        expected_revision: adopted.new_revision, offset: 0, limit: 1,
    }, OperationControl::Cancellation(cancelled)).await,
        Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled")
    );
    let expired = eutheto_types::ParentSolveBudget::new(
        DurationMillis::ZERO,
        Arc::new(eutheto_types::FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    assert!(
        matches!(app.manual_edit(eutheto_core::ManualEditRequestV1::Read {
        schema_version: 1, request_id: RequestId::new(&SystemIdGenerator)?, scenario_id,
        expected_revision: adopted.new_revision, offset: 0, limit: 1,
    }, OperationControl::Solve(expired.phase_view())).await,
        Err(AppError::Protocol(failure)) if failure.code == "operation.deadline_exceeded")
    );
    assert!(
        serde_json::from_value::<ManualEditIntentV1>(json!({
            "kind": "protection", "personId": incumbent, "shiftId": shift,
            "protection": { "kind": "hard" }, "receipt": "not-native-authority",
        }))
        .is_err()
    );
    let remove = ManualEditIntentV1::Remove {
        person_id: incumbent,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    let invalid = review_manual(&app, scenario_id, adopted.new_revision, remove.clone()).await?;
    assert_eq!(invalid.disposition, ManualEditDispositionV1::Invalid);
    assert!(invalid.approval_digest.is_none());
    assert!(
        invalid
            .evaluations
            .iter()
            .any(|finding| !finding.evaluation.satisfied
                && !finding.owner_paths.is_empty()
                && !finding.evaluation.expected.is_empty()
                && !finding.evaluation.observed.is_empty())
    );
    let coverage_rule = id(32).parse()?;
    assert!(invalid.evaluations.iter().any(|finding| {
        !finding.evaluation.satisfied
            && finding.evaluation.rule_id == coverage_rule
            && finding
                .owner_paths
                .contains(&format!("/domain/entities/{}/coverage", id(6)))
    }));
    assert!(
        apply_reviewed_manual(
            &app,
            scenario_id,
            adopted.new_revision,
            remove,
            "0".repeat(64),
            false
        )
        .await
        .is_err()
    );
    assert_eq!(store.history(scenario_id).await?.len(), 1);
    let lock = ManualEditIntentV1::Protection {
        person_id: incumbent,
        shift_id: shift,
        protection: ManualProtectionV1::Hard,
    };
    let lock_review = review_manual(&app, scenario_id, adopted.new_revision, lock.clone()).await?;
    let repeated = review_manual(&app, scenario_id, adopted.new_revision, lock.clone()).await?;
    assert_eq!(lock_review.approval_digest, repeated.approval_digest);
    assert!(
        apply_reviewed_manual(
            &app,
            scenario_id,
            adopted.new_revision,
            lock.clone(),
            "0".repeat(64),
            false
        )
        .await
        .is_err()
    );
    assert_eq!(store.history(scenario_id).await?.len(), 1);
    let locked = apply_reviewed_manual(
        &app,
        scenario_id,
        adopted.new_revision,
        lock.clone(),
        lock_review
            .approval_digest
            .ok_or("valid lock omitted digest")?,
        false,
    )
    .await?;
    assert!(
        review_manual(&app, scenario_id, adopted.new_revision, lock.clone())
            .await
            .is_err()
    );
    assert!(
        review_manual(&app, scenario_id, locked.new_revision, lock)
            .await
            .is_err()
    );
    let before_replace = store.get_project(scenario_id).await?;
    let soft = ManualEditIntentV1::Protection {
        person_id: incumbent,
        shift_id: shift,
        protection: ManualProtectionV1::Soft {
            stability_weight: 7,
        },
    };
    let reviewed_soft = review_manual(&app, scenario_id, locked.new_revision, soft).await?;
    assert_eq!(reviewed_soft.disposition, ManualEditDispositionV1::Valid);
    assert_eq!(
        reviewed_soft.changes[0].before.lock_id,
        reviewed_soft.changes[0].after.lock_id
    );
    assert_eq!(
        reviewed_soft.changes[0].after.protection,
        ManualProtectionV1::Soft {
            stability_weight: 7
        }
    );
    let changed_weight = ManualEditIntentV1::Protection {
        person_id: incumbent,
        shift_id: shift,
        protection: ManualProtectionV1::Soft {
            stability_weight: 8,
        },
    };
    assert!(
        apply_reviewed_manual(
            &app,
            scenario_id,
            locked.new_revision,
            changed_weight,
            reviewed_soft
                .approval_digest
                .ok_or("valid Soft review omitted digest")?,
            false
        )
        .await
        .is_err()
    );
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        before_replace.document
    );
    let clear_and_remove = ManualEditIntentV1::Remove {
        person_id: incumbent,
        shift_id: shift,
        protection_changes: vec![ManualProtectionChangeV1 {
            person_id: incumbent,
            shift_id: shift,
            protection: ManualProtectionV1::Unlocked,
        }],
    };
    let invalid = review_manual(
        &app,
        scenario_id,
        locked.new_revision,
        clear_and_remove.clone(),
    )
    .await?;
    assert_eq!(invalid.disposition, ManualEditDispositionV1::Invalid);
    assert!(
        apply_reviewed_manual(
            &app,
            scenario_id,
            locked.new_revision,
            clear_and_remove,
            "0".repeat(64),
            false
        )
        .await
        .is_err()
    );
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        before_replace.document
    );
    let replacement_without_unlock = ManualEditIntentV1::Replace {
        incumbent_person_id: incumbent,
        target_person_id: target,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    let invalid = review_manual(
        &app,
        scenario_id,
        locked.new_revision,
        replacement_without_unlock,
    )
    .await?;
    assert_eq!(invalid.disposition, ManualEditDispositionV1::Invalid);
    assert!(invalid.evaluations.iter().any(|finding| {
        !finding.evaluation.satisfied
            && finding
                .owner_paths
                .iter()
                .any(|path| path.starts_with("/domain/lockedAssignments/"))
    }));
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        before_replace.document
    );
    let transitions = vec![
        ManualProtectionChangeV1 {
            person_id: incumbent,
            shift_id: shift,
            protection: ManualProtectionV1::Unlocked,
        },
        ManualProtectionChangeV1 {
            person_id: target,
            shift_id: shift,
            protection: ManualProtectionV1::Hard,
        },
    ];
    let duplicate = ManualEditIntentV1::Replace {
        incumbent_person_id: incumbent,
        target_person_id: target,
        shift_id: shift,
        protection_changes: vec![transitions[0].clone(), transitions[0].clone()],
    };
    assert!(
        review_manual(&app, scenario_id, locked.new_revision, duplicate)
            .await
            .is_err()
    );
    let replacement = ManualEditIntentV1::Replace {
        incumbent_person_id: incumbent,
        target_person_id: target,
        shift_id: shift,
        protection_changes: transitions,
    };
    let reviewed =
        review_manual(&app, scenario_id, locked.new_revision, replacement.clone()).await?;
    assert_eq!(reviewed.disposition, ManualEditDispositionV1::Valid);
    assert_eq!(reviewed.changes.len(), 2);
    let repeated =
        review_manual(&app, scenario_id, locked.new_revision, replacement.clone()).await?;
    assert_eq!(reviewed.approval_digest, repeated.approval_digest);
    let replaced = apply_reviewed_manual(
        &app,
        scenario_id,
        locked.new_revision,
        replacement,
        reviewed
            .approval_digest
            .ok_or("valid replacement omitted digest")?,
        false,
    )
    .await?;
    let after_replace = store.get_project(scenario_id).await?;
    assert_eq!(store.history(scenario_id).await?.len(), 3);
    assert_eq!(after_replace.document.domain.locked_assignments.len(), 1);
    let lock: eutheto_workforce::model::AssignmentLock = serde_json::from_value(
        after_replace
            .document
            .domain
            .locked_assignments
            .values()
            .next()
            .ok_or("target lock missing")?
            .clone(),
    )?;
    assert_eq!(lock.person_id, target);
    assert_eq!(lock.state, eutheto_workforce::model::LockState::Hard {});
    assert_eq!(
        serde_json::to_vec(&store.load_accepted_result(accepted_id).await?.portable)?,
        original_bytes
    );
    drop(app);
    drop(store);
    drop(outcome);
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
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        Arc::new(
            DomainPackRegistry::builder()
                .register(eutheto_workforce::WorkforcePack)
                .build()?,
        ),
        Arc::new(real_registry().await?.0),
    );
    let AppCommandResult::ScenarioCommand(undone) = app
        .execute(AppCommand::Undo {
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: replaced.new_revision,
        })
        .await
        .map_err(boxed)?
    else {
        return Err("restart Undo omitted receipt".into());
    };
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        before_replace.document
    );
    let clear = ManualEditIntentV1::Protection {
        person_id: incumbent,
        shift_id: shift,
        protection: ManualProtectionV1::Unlocked,
    };
    let reviewed = review_manual(&app, scenario_id, undone.new_revision, clear.clone()).await?;
    let digest = reviewed
        .approval_digest
        .ok_or("valid unlock omitted digest")?;
    assert!(
        apply_reviewed_manual(
            &app,
            scenario_id,
            undone.new_revision,
            clear.clone(),
            digest.clone(),
            false
        )
        .await
        .is_err()
    );
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        before_replace.document
    );
    let cleared =
        apply_reviewed_manual(&app, scenario_id, undone.new_revision, clear, digest, true).await?;
    assert!(
        store
            .get_project(scenario_id)
            .await?
            .document
            .domain
            .locked_assignments
            .is_empty()
    );

    // B-only recurring work can disappear without changing immutable A or its base endpoints.
    let optional_shift = id(61).parse()?;
    let add_optional = ManualEditIntentV1::Add {
        person_id: target,
        shift_id: optional_shift,
        protection_changes: Vec::new(),
    };
    let reviewed = review_manual(
        &app,
        scenario_id,
        cleared.new_revision,
        add_optional.clone(),
    )
    .await?;
    let extra = apply_reviewed_manual(
        &app,
        scenario_id,
        cleared.new_revision,
        add_optional,
        reviewed
            .approval_digest
            .ok_or("optional work omitted digest")?,
        false,
    )
    .await?;
    let removed_template = edit_manual_source(
        &app,
        scenario_id,
        extra.new_revision,
        eutheto_workforce::commands::REMOVE_ENTITY,
        json!({"entityId": id(60)}),
    )
    .await?;
    let read = app
        .manual_edit(
            eutheto_core::ManualEditRequestV1::Read {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: removed_template.new_revision,
                offset: 0,
                limit: 100,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let eutheto_core::ManualEditResultV1::Read { read } = read else {
        return Err("historical work read omitted page".into());
    };
    let missing = read
        .rows
        .iter()
        .find(|row| row.shift_id == optional_shift)
        .ok_or("historical B-only work disappeared")?;
    assert!(missing.selected && missing.person_available && !missing.shift_available);
    assert_eq!(missing.shift_label, "Work unavailable in current source");
    let protect_missing = ManualEditIntentV1::Protection {
        person_id: target,
        shift_id: optional_shift,
        protection: ManualProtectionV1::Hard,
    };
    assert!(
        review_manual(
            &app,
            scenario_id,
            removed_template.new_revision,
            protect_missing
        )
        .await
        .is_err()
    );
    let add_missing = ManualEditIntentV1::Add {
        person_id: incumbent,
        shift_id: optional_shift,
        protection_changes: Vec::new(),
    };
    assert!(
        review_manual(
            &app,
            scenario_id,
            removed_template.new_revision,
            add_missing
        )
        .await
        .is_err()
    );
    let remove_missing = ManualEditIntentV1::Remove {
        person_id: target,
        shift_id: optional_shift,
        protection_changes: Vec::new(),
    };
    let reviewed = review_manual(
        &app,
        scenario_id,
        removed_template.new_revision,
        remove_missing.clone(),
    )
    .await?;
    assert_eq!(reviewed.disposition, ManualEditDispositionV1::Valid);
    let current = apply_reviewed_manual(
        &app,
        scenario_id,
        removed_template.new_revision,
        remove_missing,
        reviewed
            .approval_digest
            .ok_or("historical removal omitted digest")?,
        false,
    )
    .await?;

    // Native approved-leave evidence must navigate to its authored window, not a related person.
    let leave = json!({
        "kind": "availability", "id": id(62), "personId": target, "availabilityKind": "approvedTimeOff",
        "timeWindow": {"kind": "instant", "startsAt": "2026-11-01T08:00:00Z", "endsAt": "2026-11-01T10:00:00Z"},
        "effectiveRange": {"startDate": "2026-11-01", "endDateExclusive": "2026-11-02"}, "source": "", "note": "",
    });
    let with_leave = edit_manual_source(
        &app,
        scenario_id,
        current.new_revision,
        eutheto_workforce::commands::ADD_ENTITY,
        json!({"entity": leave}),
    )
    .await?;
    let unprotected_replace = ManualEditIntentV1::Replace {
        incumbent_person_id: incumbent,
        target_person_id: target,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    let invalid_leave = review_manual(
        &app,
        scenario_id,
        with_leave.new_revision,
        unprotected_replace.clone(),
    )
    .await?;
    assert_eq!(invalid_leave.disposition, ManualEditDispositionV1::Invalid);
    assert!(invalid_leave.evaluations.iter().any(|finding| {
        !finding.evaluation.satisfied
            && finding
                .owner_paths
                .contains(&format!("/domain/entities/{}/timeWindow", id(62)))
    }));
    let without_leave = edit_manual_source(
        &app,
        scenario_id,
        with_leave.new_revision,
        eutheto_workforce::commands::REMOVE_ENTITY,
        json!({"entityId": id(62)}),
    )
    .await?;
    let target_value = store
        .get_project(scenario_id)
        .await?
        .document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("target missing")?
        .clone();
    let mut inactive_target = target_value.clone();
    inactive_target["activeRange"] =
        json!({"kind": "dateRange", "startDate": "2026-11-02", "endDateExclusive": "2026-11-03"});
    let outside_range = edit_manual_source(
        &app,
        scenario_id,
        without_leave.new_revision,
        eutheto_workforce::commands::UPDATE_ENTITY,
        json!({"entity": inactive_target}),
    )
    .await?;
    let invalid_person = review_manual(
        &app,
        scenario_id,
        outside_range.new_revision,
        unprotected_replace.clone(),
    )
    .await?;
    assert_eq!(invalid_person.disposition, ManualEditDispositionV1::Invalid);
    assert!(invalid_person.evaluations.iter().any(|finding| {
        !finding.evaluation.satisfied
            && finding
                .owner_paths
                .contains(&format!("/domain/entities/{}/activeRange", id(1)))
    }));
    let restored_target = edit_manual_source(
        &app,
        scenario_id,
        outside_range.new_revision,
        eutheto_workforce::commands::UPDATE_ENTITY,
        json!({"entity": target_value}),
    )
    .await?;
    let reviewed = review_manual(
        &app,
        scenario_id,
        restored_target.new_revision,
        unprotected_replace.clone(),
    )
    .await?;
    let replaced = apply_reviewed_manual(
        &app,
        scenario_id,
        restored_target.new_revision,
        unprotected_replace,
        reviewed
            .approval_digest
            .ok_or("restored replacement omitted digest")?,
        false,
    )
    .await?;
    let removed_person = edit_manual_source(
        &app,
        scenario_id,
        replaced.new_revision,
        eutheto_workforce::commands::REMOVE_ENTITY,
        json!({"entityId": target}),
    )
    .await?;
    let read = app
        .manual_edit(
            eutheto_core::ManualEditRequestV1::Read {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: removed_person.new_revision,
                offset: 0,
                limit: 100,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let eutheto_core::ManualEditResultV1::Read { read } = read else {
        return Err("historical person read omitted page".into());
    };
    assert!(
        read.rows
            .iter()
            .any(|row| row.person_id == target && !row.person_available && row.shift_available)
    );
    let replace_missing_person = ManualEditIntentV1::Replace {
        incumbent_person_id: target,
        target_person_id: incumbent,
        shift_id: shift,
        protection_changes: Vec::new(),
    };
    let reviewed = review_manual(
        &app,
        scenario_id,
        removed_person.new_revision,
        replace_missing_person.clone(),
    )
    .await?;
    assert_eq!(reviewed.disposition, ManualEditDispositionV1::Valid);
    apply_reviewed_manual(
        &app,
        scenario_id,
        removed_person.new_revision,
        replace_missing_person,
        reviewed
            .approval_digest
            .ok_or("missing incumbent replacement omitted digest")?,
        false,
    )
    .await?;
    assert_eq!(
        serde_json::to_vec(&store.load_accepted_result(accepted_id).await?.portable)?,
        original_bytes
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_repair_facts_keep_recorded_manual_frame_after_history_undo() -> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let run_options = options_with_explanation(ExplanationMode::None)?;
    let (directory, store, base_outcome, runtime, app) = stored_real_run_with_options(
        original,
        run_options.clone(),
        Some(authored_optimize_selection()?),
    )
    .await?;
    assert_stored_bound_runtime(&base_outcome, &runtime)?;
    let base = store
        .load_accepted_result(
            base_outcome
                .accepted_result_id()
                .ok_or("missing accepted base")?,
        )
        .await?;
    let base_reference = AcceptedResultRefV1::from_result(&base.portable.accepted_result)?;
    let base_pair = base
        .portable
        .accepted_result
        .solution
        .assignments
        .iter()
        .find_map(|assignment| {
            match eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment) {
                Ok((pair, true)) => Some(Ok(pair)),
                Ok((_, false)) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .ok_or("missing base pair")?
        .map_err(boxed)?;
    let manual_pair = eutheto_workforce::model::AssignmentPair {
        person_id: id(1).parse()?,
        shift_id: base_pair.shift_id,
    };
    assert_ne!(base_pair.person_id, manual_pair.person_id);
    let AppCommandResult::ScenarioCommand(adoption) = app
        .execute(AppCommand::AdoptBaseSchedule(
            eutheto_core::SolutionAdoptBaseRequestV1 {
                schema_version: eutheto_core::SOLUTION_ADOPT_BASE_SCHEMA_VERSION,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                result: base_reference.clone(),
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Repair facts regression".to_owned(),
                },
                truncate_redo: false,
            },
        ))
        .await
        .map_err(boxed)?
    else {
        return Err("base adoption omitted command receipt".into());
    };
    let intent = eutheto_core::ManualEditIntentV1::Replace {
        incumbent_person_id: base_pair.person_id,
        target_person_id: manual_pair.person_id,
        shift_id: base_pair.shift_id,
        protection_changes: Vec::new(),
    };
    let reviewed = review_manual(&app, scenario_id, adoption.new_revision, intent.clone()).await?;
    assert_eq!(
        reviewed.disposition,
        eutheto_core::ManualEditDispositionV1::Valid
    );
    assert!(
        reviewed
            .evaluations
            .iter()
            .all(|outcome| outcome.evaluation.satisfied)
    );
    assert_eq!(store.history(scenario_id).await?.len(), 1);
    let admission = apply_reviewed_manual(
        &app,
        scenario_id,
        adoption.new_revision,
        intent,
        reviewed
            .approval_digest
            .ok_or("valid review omitted digest")?,
        false,
    )
    .await?;
    let admitted = store.get_project(scenario_id).await?;
    let mut person = admitted
        .document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("missing manually selected person")?
        .clone();
    person["eligibleAssignmentTypeIds"] = json!([]);
    let AppCommandResult::ScenarioCommand(edit) = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: admission.new_revision,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Repair facts regression".to_owned(),
                },
                source: CommandSource::Desktop,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity":person}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?
    else {
        return Err("source change omitted command receipt".into());
    };
    let repair_outcome = solve_real_again(
        &app,
        scenario_id,
        edit.new_revision,
        run_options,
        StoredObjectiveSelectionV1 {
            schema_version: 1,
            profile_id: eutheto_domain_ir::ObjectiveProfileId::new(
                "official.workforce.objective.minimal-changes",
            )?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            baseline_source: eutheto_core::StoredBaselineSourceV1::Accepted,
            alternatives_capture_id: None,
        },
    )
    .await?;
    assert_stored_bound_runtime(&repair_outcome, &runtime)?;
    let repaired = store
        .load_accepted_result(
            repair_outcome
                .accepted_result_id()
                .ok_or("missing accepted Repair")?,
        )
        .await?;
    assert_eq!(
        selected_person(&repaired.portable.accepted_result)?,
        base_pair.person_id
    );
    let repaired_reference = AcceptedResultRefV1::from_result(&repaired.portable.accepted_result)?;
    let recorded = repaired
        .portable
        .run_input
        .objective_context
        .as_ref()
        .and_then(|context| context.baseline.as_deref())
        .ok_or("missing recorded Repair baseline")?;
    assert!(matches!(
        recorded.selection,
        eutheto_domain_ir::RunBaselineSelection::ManualSelection { .. }
    ));
    recipient_report::assert_recipient_repair_history(
        &app,
        scenario_id,
        repaired.portable.accepted_result.solution.solution_id,
        edit.new_revision,
        &base_pair,
        &manual_pair,
        true,
    )
    .await?;
    recipient_report::assert_file_recipient_repair(
        &app.headless_service(),
        &repaired,
        &base_pair,
        &manual_pair,
    )?;
    let mut visible = edit.new_revision;
    for _ in 0..2 {
        let AppCommandResult::ScenarioCommand(undo) = app
            .execute(AppCommand::Undo {
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: visible,
            })
            .await
            .map_err(boxed)?
        else {
            return Err("Undo omitted receipt".into());
        };
        visible = undo.new_revision;
    }
    recipient_report::assert_recipient_repair_history(
        &app,
        scenario_id,
        repaired.portable.accepted_result.solution.solution_id,
        visible,
        &base_pair,
        &manual_pair,
        false,
    )
    .await?;
    let current = store.get_project(scenario_id).await?;
    assert!(
        !current
            .document
            .domain
            .entities
            .values()
            .any(|raw| raw["kind"] == "manualSelection")
    );
    let explanation = query_explanation(
        &app,
        scenario_id,
        edit.new_revision,
        ExplanationRequestSubjectV1::Repair {
            current: repaired_reference,
            base: base_reference,
        },
    )
    .await?;
    assert_eq!(explanation.current_revision, visible);
    assert_eq!(
        explanation.scenario_revisions,
        [edit.new_revision, Revision::INITIAL]
    );
    let ExplanationEvidencePayloadV2::Repair { repair } =
        &explanation.explanation.evidence.evidence
    else {
        return Err("missing Repair evidence".into());
    };
    // Pruning a false native variable is descriptive, not a selected assignment change.
    for change in &repair.comparison.assignments {
        match change {
            eutheto_domain_ir::AssignmentComparisonV1::Added { after } => {
                assert!(
                    !eutheto_workforce::assignment_rules::decode_workforce_assignment(after)?.1
                );
            }
            eutheto_domain_ir::AssignmentComparisonV1::Removed { before } => {
                assert!(
                    !eutheto_workforce::assignment_rules::decode_workforce_assignment(before)?.1
                );
            }
            eutheto_domain_ir::AssignmentComparisonV1::Changed { before, after } => {
                assert_eq!(
                    eutheto_workforce::assignment_rules::decode_workforce_assignment(before)?,
                    eutheto_workforce::assignment_rules::decode_workforce_assignment(after)?,
                    "A→C must not be replaced by B→C",
                );
            }
        }
    }
    let fact_id = |name: &str| {
        eutheto_domain_ir::VerificationFactId::new(format!("official.workforce.fact.{name}"))
    };
    let manual_assignment = base
        .portable
        .accepted_result
        .solution
        .assignments
        .iter()
        .find(|assignment| {
            eutheto_workforce::assignment_rules::decode_workforce_assignment(assignment)
                .is_ok_and(|(pair, _)| pair == manual_pair)
        })
        .ok_or("missing saved native manual-pair identity")?;
    let observation = explanation
        .explanation
        .rendered
        .messages
        .iter()
        .find(|message| message.assignments.contains(&manual_assignment.id))
        .ok_or("missing B-only invalidated assignment fact")?;
    assert_eq!(
        observation.parameters.get(&fact_id("repair_frame")?),
        Some(&eutheto_domain_ir::VerificationValue::Text(
            "recordedBaselineToCandidate".to_owned()
        ),)
    );
    assert_eq!(
        observation.parameters.get(&fact_id("baseline_admission")?),
        Some(&eutheto_domain_ir::VerificationValue::Text(
            "notEstablished".to_owned()
        ),)
    );
    assert_eq!(
        observation
            .parameters
            .get(&fact_id("baseline_pair_content_hash")?),
        Some(&eutheto_domain_ir::VerificationValue::Text(
            recorded.selected_pair_content_hash.clone()
        ),)
    );
    assert_eq!(
        observation
            .parameters
            .get(&fact_id("repair_source_revision")?),
        Some(&eutheto_domain_ir::VerificationValue::Integer(
            i64::try_from(edit.new_revision.value())?
        ),)
    );
    explanation.explanation.validate()?;
    assert_eq!(
        store.get_project(scenario_id).await?.summary.revision,
        visible
    );
    drop(directory);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_manual_replay_reopens_and_rejects_overwritten_inverse_values() -> TestResult
{
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (directory, store, outcome, runtime, app) =
        stored_real_run(original, ExplanationMode::None).await?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let result_id = outcome
        .accepted_result_id()
        .ok_or("missing real accepted result")?;
    let accepted = store.load_accepted_result(result_id).await?;
    let reference = AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?;
    let adopted = app
        .execute(AppCommand::AdoptBaseSchedule(
            eutheto_core::SolutionAdoptBaseRequestV1 {
                schema_version: eutheto_core::SOLUTION_ADOPT_BASE_SCHEMA_VERSION,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                result: reference.clone(),
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Native replay regression".to_owned(),
                },
                truncate_redo: false,
            },
        ))
        .await
        .map_err(boxed)?;
    let AppCommandResult::ScenarioCommand(adoption) = adopted else {
        return Err("base adoption did not produce a command receipt".into());
    };
    let before_manual = store.get_project(scenario_id).await?;
    let raw_base = before_manual
        .document
        .domain
        .entities
        .values()
        .find(|raw| raw.get("kind").and_then(Value::as_str) == Some("baseSchedule"))
        .ok_or("missing adopted base")?;
    let eutheto_workforce::model::WorkforceEntity::BaseSchedule(base) =
        serde_json::from_value(raw_base.clone())?
    else {
        return Err("base projection changed kind".into());
    };
    let admission = app
        .admit_manual_selection(
            eutheto_core::ManualSelectionAdmissionRequestV1 {
                schema_version: eutheto_core::MANUAL_SELECTION_ADMISSION_SCHEMA_VERSION,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: adoption.new_revision,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Native replay regression".to_owned(),
                },
                original: reference,
                assignments: base.assignments,
                lock_changes: Vec::new(),
                truncate_redo: false,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?
        .command_result;
    let admitted = store.get_project(scenario_id).await?;
    assert!(
        admitted
            .document
            .domain
            .entities
            .values()
            .any(|raw| raw.get("kind").and_then(Value::as_str) == Some("manualSelection"))
    );
    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: admission.new_revision,
    })
    .await
    .map_err(boxed)?;
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        before_manual.document
    );
    drop(outcome);
    drop(app);
    drop(store);

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
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        Arc::new(
            DomainPackRegistry::builder()
                .register(eutheto_workforce::WorkforcePack)
                .build()?,
        ),
        Arc::new(real_registry().await?.0),
    );
    app.execute(AppCommand::Redo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::new(3),
    })
    .await
    .map_err(boxed)?;
    let restored = store.get_project(scenario_id).await?;
    assert_eq!(restored.document, admitted.document);

    let mut replacement_base = restored
        .document
        .domain
        .entities
        .get(&base.id)
        .ok_or("missing restored base")?
        .clone();
    replacement_base["sourceRevision"] = json!(4);
    let mut replacement_person = restored
        .document
        .domain
        .entities
        .values()
        .find(|raw| raw.get("kind").and_then(Value::as_str) == Some("person"))
        .ok_or("missing source person")?
        .clone();
    replacement_person["name"] = json!("Bob");
    let command_id = CommandId::new(&SystemIdGenerator)?;
    app.execute(AppCommand::ApplyScenario {
        request_id: RequestId::new(&SystemIdGenerator)?,
        envelope: CommandEnvelope {
            command_id,
            scenario_id,
            expected_revision: restored.summary.revision,
            actor: ActorRef {
                actor_id: None,
                display_name: "Native replay regression".to_owned(),
            },
            source: CommandSource::Desktop,
            command: ScenarioCommand::ApplyBatch(eutheto_types::CommandBatch {
                label: None,
                commands: [replacement_base, replacement_person]
                    .into_iter()
                    .map(|entity| {
                        ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                            command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                            payload: json!({"entity": entity}),
                        })
                    })
                    .collect(),
            }),
        },
        truncate_redo: false,
    })
    .await
    .map_err(boxed)?;
    let after = store.get_project(scenario_id).await?;
    assert!(
        !after
            .document
            .domain
            .entities
            .values()
            .any(|raw| raw.get("kind").and_then(Value::as_str) == Some("manualSelection"))
    );
    let connection = rusqlite::Connection::open(directory.path().join("eutheto.sqlite"))?;
    let original_inverse: String = connection.query_row(
        "SELECT inverse_json FROM command_journal WHERE id = ?1",
        [command_id.to_string()],
        |row| row.get(0),
    )?;
    let mut inverse: Value = serde_json::from_str(&original_inverse)?;
    let mut public_inverse: ScenarioCommand = serde_json::from_value(
        inverse
            .get("public_command")
            .ok_or("missing protected ordinary inverse")?
            .clone(),
    )?;
    let ScenarioCommand::ApplyBatch(batch) = &mut public_inverse else {
        return Err("compound command lost its compound inverse".into());
    };
    let mut altered_person = false;
    for command in &mut batch.commands {
        if let ScenarioCommand::ApplyDomainCommand(command) = command
            && let Some(entity) = command.payload.get_mut("entity")
            && entity.get("kind").and_then(Value::as_str) == Some("person")
        {
            entity["name"] = json!("Mallory");
            altered_person = true;
        }
    }
    assert!(altered_person);
    inverse["public_command"] = serde_json::to_value(public_inverse)?;
    connection.execute(
        "UPDATE command_journal SET inverse_json = ?1 WHERE id = ?2",
        rusqlite::params![serde_json::to_string(&inverse)?, command_id.to_string()],
    )?;
    let Err(error) = app
        .execute(AppCommand::Undo {
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: after.summary.revision,
        })
        .await
    else {
        return Err("a reversible overwrite authenticated wrong original values".into());
    };
    assert!(
        matches!(error, AppError::Storage(failure) if failure.code == "storage.operation_failed")
    );
    let unchanged = store.get_project(scenario_id).await?;
    assert_eq!(unchanged.summary.revision, after.summary.revision);
    assert_eq!(unchanged.document, after.document);
    connection.execute(
        "UPDATE command_journal SET inverse_json = ?1 WHERE id = ?2",
        rusqlite::params![original_inverse, command_id.to_string()],
    )?;
    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: after.summary.revision,
    })
    .await
    .map_err(boxed)?;
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        restored.document
    );
    app.execute(AppCommand::Redo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::new(6),
    })
    .await
    .map_err(boxed)?;
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        after.document
    );
    assert_eq!(store.load_accepted_result(result_id).await?, accepted);

    // Imported lineage stays foreign/inert, including the first local journal entry.
    let mut foreign = restored.document.clone();
    let foreign_id = ScenarioId::new(&SystemIdGenerator)?;
    foreign.scenario_id = foreign_id;
    foreign.metadata.created_at = Rfc3339Timestamp::parse("2000-01-01T00:00:00Z")?;
    assert_ne!(foreign.metadata.created_at, foreign.metadata.updated_at);
    // A fresh imported library has no ownership collision or local admission receipt.
    let foreign_dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("foreign.sqlite"),
            safety_backups: directory.path().join("foreign-backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let foreign_packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let foreign_solvers = Arc::new(real_registry().await?.0);
    let (foreign_store, initialization) =
        SqliteScenarioStore::open(&foreign_dependencies.paths.database).await?;
    let foreign_store = Arc::new(foreign_store);
    foreign_store
        .create_project(NewProject {
            document: foreign.clone(),
        })
        .await?;
    let foreign_app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&foreign_store),
        initialization,
        foreign_dependencies.clone(),
        Arc::clone(&foreign_packs),
        Arc::clone(&foreign_solvers),
    );
    let mut foreign_base = foreign
        .domain
        .entities
        .get(&base.id)
        .ok_or("missing imported base")?
        .clone();
    foreign_base["sourceRevision"] = json!(42);
    foreign_app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id: foreign_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Foreign replay regression".to_owned(),
                },
                source: CommandSource::Desktop,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity": foreign_base}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    let foreign_after = foreign_store.get_project(foreign_id).await?;
    foreign_app
        .execute(AppCommand::Undo {
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id: foreign_id,
            expected_revision: Revision::new(1),
        })
        .await
        .map_err(boxed)?;
    assert_eq!(
        foreign_store.get_project(foreign_id).await?.document,
        foreign
    );
    drop(foreign_app);
    drop(foreign_store);
    let (foreign_store, initialization) =
        SqliteScenarioStore::open(&foreign_dependencies.paths.database).await?;
    let foreign_store = Arc::new(foreign_store);
    let foreign_app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&foreign_store),
        initialization,
        foreign_dependencies,
        foreign_packs,
        foreign_solvers,
    );
    foreign_app
        .execute(AppCommand::Redo {
            request_id: RequestId::new(&SystemIdGenerator)?,
            scenario_id: foreign_id,
            expected_revision: Revision::new(2),
        })
        .await
        .map_err(boxed)?;
    assert_eq!(
        foreign_store.get_project(foreign_id).await?.document,
        foreign_after.document
    );
    Ok(())
}

fn assert_stored_bound_runtime(
    outcome: &StoredSolveOutcome,
    runtime: &BackendRuntimeIdentity,
) -> TestResult {
    let report = outcome
        .report
        .as_ref()
        .ok_or("missing stored solve report")?;
    assert_eq!(report.execution.invocation_count, 1);
    let [attempt] = report.execution.attempts.as_slice() else {
        return Err("stored solve did not retain one backend attempt".into());
    };
    assert!(!attempt.fallback_taken);
    let backend = attempt
        .outcome
        .as_ref()
        .ok_or("missing stored backend outcome")?;
    let evidence = &backend
        .evidence
        .execution
        .as_ref()
        .ok_or("missing stored runtime evidence")?
        .reproducibility;
    assert_eq!(&backend.backend_id, runtime.backend_id());
    assert_eq!(&outcome.state().input.backend_id, runtime.backend_id());
    assert_eq!(
        outcome.state().input.solve_options.backend,
        BackendSelection::Specific(runtime.backend_id().clone())
    );
    assert_eq!(evidence.worker_version, runtime.worker_version());
    assert_eq!(evidence.engine_version, runtime.solver_version());
    assert_eq!(evidence.applied_parameters.worker_threads, 1);
    Ok(())
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

fn reseal_forged_score(portable: &PortableAcceptedResult) -> TestResult<PortableAcceptedResult> {
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
    let manifest = RunManifest::new(
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
    )?
    .with_native_observations(manifest.native_observations.clone())?;
    Ok(PortableAcceptedResult::new(
        portable.run_input.clone(),
        manifest,
        accepted,
        portable.evidence.clone(),
    )?)
}

fn assert_forged_worker_results_rejected(
    service: &HeadlessService,
    snapshot: &ScenarioSnapshotV1,
    portable: &PortableAcceptedResult,
    control: &OperationControl,
) -> TestResult {
    // A valid public checksum does not authenticate a recorded routed fingerprint.
    let mut observations = portable
        .run_manifest
        .native_observations
        .clone()
        .ok_or("missing actual native observations")?;
    let attempt = observations
        .attempt
        .as_mut()
        .ok_or("missing native attempt")?;
    let replacement = if attempt.solve_fingerprint.starts_with('0') {
        "1"
    } else {
        "0"
    };
    attempt.solve_fingerprint.replace_range(..1, replacement);
    let manifest = portable
        .run_manifest
        .clone()
        .with_native_observations(Some(observations))?;
    let forged_fingerprint = PortableAcceptedResult::new(
        portable.run_input.clone(),
        manifest,
        portable.accepted_result.clone(),
        portable.evidence.clone(),
    )?;
    forged_fingerprint.validate()?;
    assert!(
        service
            .verify_result(snapshot, &forged_fingerprint, control)
            .is_err()
    );
    assert!(
        service
            .export_recipient_html(
                snapshot,
                &forged_fingerprint,
                eutheto_types::RecipientReportOptionsV1::default(),
                control,
            )
            .is_err()
    );
    // Reseal every checksum so independent source scoring must reject the forged score.
    let forged = reseal_forged_score(portable)?;
    forged.validate()?;
    assert!(service.verify_result(snapshot, &forged, control).is_err());
    assert!(
        service
            .export_recipient_html(
                snapshot,
                &forged,
                eutheto_types::RecipientReportOptionsV1::default(),
                control,
            )
            .is_err()
    );
    assert!(
        service
            .export_result_json(snapshot, &forged, control)
            .is_err()
    );
    assert!(
        service
            .export_assignments_csv(snapshot, &forged, control)
            .is_err()
    );
    assert!(
        service
            .export_result_summary_csv(snapshot, &forged, control)
            .is_err()
    );
    assert!(
        service
            .export_result_ics(snapshot, &forged, None, control)
            .is_err()
    );
    Ok(())
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
    let portable = PortableAcceptedResult::from_json(&serde_json::to_vec(&portable)?)?;
    let fresh = service
        .verify_result(&snapshot, &portable, &control)
        .map_err(boxed)?;
    assert!(fresh.accepted);
    assert_eq!(fresh.score, portable.accepted_result.verification.score);
    assert!(
        service
            .export_result_ics(&snapshot, &portable, None, &control)
            .is_ok()
    );
    assert_forged_worker_results_rejected(&service, &snapshot, &portable, &control)?;
    recipient_report::assert_file_recipient_report(&service, &snapshot, &portable, &control)?;
    let mut changed_source = snapshot.clone();
    changed_source.document = self::snapshot(2)?.document;
    assert!(
        service
            .verify_result(&changed_source, &portable, &control)
            .is_err()
    );
    assert!(
        service
            .export_result_json(&changed_source, &portable, &control)
            .is_err()
    );
    assert!(
        service
            .export_assignments_csv(&changed_source, &portable, &control)
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
async fn actual_worker_diagnostic_persists_a_mapped_conflict() -> TestResult {
    let (_directory, store, outcome, runtime, _app) =
        stored_real_run(snapshot(2)?, ExplanationMode::Standard).await?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let state = outcome.state();
    assert!(
        matches!(
            state
                .manifest
                .as_ref()
                .ok_or("missing stored manifest")?
                .outcome,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible
            }
        ),
        "actual ordinary terminal manifest: {:?}",
        state.manifest
    );
    let evidence = store
        .load_run_infeasibility_evidence(state.input.run_id)
        .await?
        .ok_or("approved worker did not retain diagnostic evidence")?;
    let proof = evidence.diagnostic.as_ref().ok_or_else(|| {
        diagnostic_failure(
            "mapped infeasibility omitted its native proof",
            &evidence.infeasibility,
        )
    })?;
    assert_eq!(
        proof.initial.binding.native_profile,
        DiagnosticNativeProfileV1::NonemptyAssumptionSatisfaction
    );
    assert!(!proof.initial.binding.submitted_assumptions.is_empty());
    match &evidence.infeasibility {
        InfeasibilityEvidenceV1::Conflict { groups, .. } => {
            assert!(!groups.is_empty());
            assert_eq!(groups, &proof.initial_core);
        }
        InfeasibilityEvidenceV1::Unavailable { .. } => {
            return Err(diagnostic_failure(
                "genuine mapped infeasibility became unavailable",
                &evidence.infeasibility,
            ));
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
// Keep the real-worker diagnostic export/copy checks in their private-data lifecycle.
#[allow(clippy::too_many_lines)]
async fn actual_worker_diagnostic_copy_undo_and_private_publication() -> TestResult {
    const PRIVATE_DESCRIPTION: &str = "private-diagnostic-source-description";
    let mut source = snapshot(2)?;
    source.document.metadata.description = PRIVATE_DESCRIPTION.repeat(256);
    let source_manual = attach_unadmitted_manual_selection(&mut source.document)?;
    let scenario_id = source.document.scenario_id;
    let (directory, store, outcome, runtime, app) =
        stored_real_run(source, ExplanationMode::Standard)
            .await
            .map_err(|error| format!("diagnostic source run: {error}"))?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let state = outcome.state();
    let manifest = state.manifest.as_ref().ok_or("missing native manifest")?;
    let evidence = store
        .load_run_infeasibility_evidence(state.input.run_id)
        .await?
        .ok_or("missing native evidence")?;
    let coverage_rule = id(32).parse()?;
    let group = match &evidence.infeasibility {
        InfeasibilityEvidenceV1::Conflict { groups, .. } => groups
            .iter()
            .find(|group| group.required_rules.contains(&coverage_rule))
            .ok_or("native conflict did not include authored coverage")?,
        InfeasibilityEvidenceV1::Unavailable { .. } => {
            return Err(diagnostic_failure(
                "missing mapped conflict",
                &evidence.infeasibility,
            ));
        }
    };
    let control = OperationControl::Cancellation(CancellationToken::new());
    let destination = directory.path().join("diagnostic.json");
    let export_request = DiagnosticExportRequestV1 {
        schema_version: DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION,
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        source_revision: Revision::INITIAL,
        run_id: state.input.run_id,
        run_manifest_checksum: manifest.checksum.clone(),
        evidence_checksum: evidence.checksum.clone(),
    };
    let original = store.get_project(scenario_id).await?;
    let observed_current_revision = original.summary.revision;
    let captured = app
        .prepare_diagnostic_export(export_request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    assert_eq!(
        captured.metadata().observed_current_revision,
        observed_current_revision
    );
    let prepared = captured.prepare_destination(&destination).map_err(boxed)?;
    assert!(!destination.exists());
    let receipt = prepared.publish_controlled().map_err(boxed)?;
    assert_eq!(receipt.source_revision, Revision::INITIAL);
    assert_eq!(receipt.observed_current_revision, observed_current_revision);
    let bytes = std::fs::read(&destination)?;
    let report: DiagnosticReport = serde_json::from_slice(&bytes)?;
    assert_eq!(report.run_id, state.input.run_id);
    assert_eq!(report.source_revision, Revision::INITIAL);
    assert_eq!(report.run_manifest_checksum, manifest.checksum);
    assert_eq!(report.evidence_checksum, evidence.checksum);
    assert_eq!(report.infeasibility, evidence.infeasibility);
    assert!(!std::str::from_utf8(&bytes)?.contains(PRIVATE_DESCRIPTION));
    let collision = app
        .prepare_diagnostic_export(export_request, control.clone())
        .await
        .map_err(boxed)?
        .prepare_destination(&destination)
        .map_err(boxed)?;
    let existing_content = b"existing destination content must not be overwritten";
    std::fs::write(&destination, existing_content)?;
    assert!(collision.publish_controlled().is_err());
    assert_eq!(std::fs::read(&destination)?, existing_content);

    let library = store.library_metadata_snapshot().await?;
    let copied = app
        .execute(AppCommand::CreateDiagnosticCopy {
            request: DiagnosticCopyRequestV1 {
                schema_version: DIAGNOSTIC_COPY_API_SCHEMA_VERSION,
                operation_id: OperationId::new(&SystemIdGenerator)?,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                source_revision: Revision::INITIAL,
                expected_library_revision: library.revision,
                run_id: state.input.run_id,
                run_manifest_checksum: manifest.checksum.clone(),
                evidence_checksum: evidence.checksum.clone(),
                group_id: group.group_id.clone(),
            },
            control,
        })
        .await
        .map_err(|error| format!("create diagnostic copy: {error:?}"))?;
    let AppCommandResult::DiagnosticCopy(copy) = copied else {
        return Err("native copy did not return its committed receipt".into());
    };
    assert_ne!(copy.project.scenario_id, scenario_id);
    assert!(serde_json::to_vec(&copy)?.len() < 136 * 1024);
    assert_eq!(
        copy.library_revision.value(),
        library
            .revision
            .value()
            .checked_add(1)
            .ok_or("revision overflow")?
    );
    let changed = store.get_project(copy.project.scenario_id).await?;
    let (copied_manual_id, copied_manual_value) = changed
        .document
        .domain
        .entities
        .iter()
        .find(|(_, value)| value.get("kind").and_then(Value::as_str) == Some("manualSelection"))
        .ok_or("diagnostic manual selection was not copied")?;
    let eutheto_workforce::model::WorkforceEntity::ManualSelection(copied_manual) =
        serde_json::from_value(copied_manual_value.clone())?
    else {
        return Err("diagnostic manual selection has an unexpected kind".into());
    };
    assert_eq!(*copied_manual_id, copied_manual.id);
    assert_ne!(copied_manual.id, source_manual.id);
    let copied_base = changed
        .document
        .domain
        .entities
        .values()
        .find_map(
            |value| match serde_json::from_value::<eutheto_workforce::model::WorkforceEntity>(
                value.clone(),
            )
            .ok()?
            {
                eutheto_workforce::model::WorkforceEntity::BaseSchedule(base) => Some(base),
                _ => None,
            },
        )
        .ok_or("diagnostic base schedule was not copied")?;
    let mut expected_original = source_manual.original.clone();
    expected_original.entity = copied_manual.original.entity.clone();
    assert_eq!(copied_manual.original, expected_original);
    assert_eq!(
        copied_manual.original.entity.id.as_str(),
        format!("official.workforce.{}", copied_base.id)
    );
    assert_eq!(
        copied_manual.admission_source_scenario_id,
        source_manual.admission_source_scenario_id
    );
    assert_eq!(
        copied_manual.admission_source_revision,
        source_manual.admission_source_revision
    );
    assert_eq!(
        copied_manual.admission_source_document_hash,
        source_manual.admission_source_document_hash
    );
    assert_eq!(
        copied_manual.admission_command_id,
        source_manual.admission_command_id
    );
    assert_eq!(copied_manual.assignments, copied_base.assignments);
    assert_eq!(
        copied_manual.checksum,
        eutheto_workforce::assignment_rules::workforce_manual_selection_checksum(
            &copied_manual,
            None,
        )?
    );
    let private_copy_lookup = store
        .lookup_private_history(
            copy.project.scenario_id,
            changed.summary.revision,
            copied_manual.admission_command_id,
            OperationControl::Cancellation(CancellationToken::new()),
            None,
        )
        .await?;
    assert!(private_copy_lookup.entry.is_none());
    assert_eq!(
        changed.document.metadata.description,
        PRIVATE_DESCRIPTION.repeat(256)
    );
    let coverage = changed
        .document
        .domain
        .rules
        .values()
        .find(|rule| rule["kind"] == "coverage")
        .ok_or("copied coverage missing")?;
    assert_eq!(coverage["active"], false);
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        original.document
    );
    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: copy.project.scenario_id,
        expected_revision: copy.project.revision,
    })
    .await
    .map_err(|error| format!("undo diagnostic copy: {error:?}"))?;
    let restored = store.get_project(copy.project.scenario_id).await?;
    assert!(
        restored
            .document
            .domain
            .rules
            .values()
            .all(|rule| rule["active"] == true)
    );
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        original.document
    );

    // This is a real complete owned-ID copy, not only a different scenario ID.
    assert_eq!(
        copied_manual.original.pair_content_hash,
        source_manual.original.pair_content_hash
    );
    assert_ne!(
        eutheto_workforce::assignment_rules::workforce_pair_content_hash(
            &copied_base.assignments,
            None,
        )?,
        source_manual.original.pair_content_hash,
    );
    let mut replacement_base = serde_json::to_value(
        eutheto_workforce::model::WorkforceEntity::BaseSchedule(copied_base),
    )?;
    replacement_base["sourceRevision"] = json!(42);
    app.execute(AppCommand::ApplyScenario {
        request_id: RequestId::new(&SystemIdGenerator)?,
        envelope: CommandEnvelope {
            command_id: CommandId::new(&SystemIdGenerator)?,
            scenario_id: copy.project.scenario_id,
            expected_revision: restored.summary.revision,
            actor: ActorRef {
                actor_id: None,
                display_name: "Copied foreign replay".to_owned(),
            },
            source: CommandSource::Desktop,
            command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                payload: json!({"entity": replacement_base}),
            }),
        },
        // Replaces the undone diagnostic question in this disposable copied scenario.
        truncate_redo: true,
    })
    .await
    .map_err(|error| format!("change copied foreign base: {error:?}"))?;
    let changed_base = store.get_project(copy.project.scenario_id).await?;
    assert!(
        changed_base
            .document
            .domain
            .entities
            .values()
            .all(|raw| raw.get("kind").and_then(Value::as_str) != Some("manualSelection"))
    );
    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: copy.project.scenario_id,
        expected_revision: changed_base.summary.revision,
    })
    .await
    .map_err(|error| format!("restore copied foreign base: {error:?}"))?;
    let before_reopen = store.get_project(copy.project.scenario_id).await?;
    assert_eq!(before_reopen.document, restored.document);
    drop(outcome);
    drop(app);
    drop(store);
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
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        Arc::new(
            DomainPackRegistry::builder()
                .register(eutheto_workforce::WorkforcePack)
                .build()?,
        ),
        Arc::new(real_registry().await?.0),
    );
    app.execute(AppCommand::Redo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: copy.project.scenario_id,
        expected_revision: before_reopen.summary.revision,
    })
    .await
    .map_err(|error| format!("redo copied foreign base: {error:?}"))?;
    assert_eq!(
        store.get_project(copy.project.scenario_id).await?.document,
        changed_base.document
    );
    assert_eq!(
        store.get_project(scenario_id).await?.document,
        original.document
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn actual_worker_diagnostic_distinguishes_zero_assumption_foundation() -> TestResult {
    let (_directory, store, outcome, runtime, _app) =
        stored_real_run(foundational_snapshot()?, ExplanationMode::Standard).await?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let state = outcome.state();
    assert!(matches!(
        state
            .manifest
            .as_ref()
            .ok_or("missing stored manifest")?
            .outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible
        }
    ));
    let evidence = store
        .load_run_infeasibility_evidence(state.input.run_id)
        .await?
        .ok_or("foundational run did not retain diagnostic evidence")?;
    if !matches!(
        &evidence.infeasibility,
        InfeasibilityEvidenceV1::Unavailable {
            reason: eutheto_domain_ir::ConflictUnavailableReason::FoundationalInfeasibility
        }
    ) {
        return Err(diagnostic_failure(
            "zero-assumption result was not classified as foundational",
            &evidence.infeasibility,
        ));
    }
    let proof = evidence.diagnostic.as_ref().ok_or_else(|| {
        diagnostic_failure(
            "foundational infeasibility omitted its native proof",
            &evidence.infeasibility,
        )
    })?;
    assert!(proof.initial_core.is_empty());
    assert!(proof.initial.binding.submitted_assumptions.is_empty());
    assert_eq!(
        proof.initial.binding.native_profile,
        DiagnosticNativeProfileV1::ZeroAssumptionSatisfaction
    );
    assert_eq!(proof.max_trials, 0);
    assert_eq!(proof.attempted_trials, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn actual_worker_diagnostic_runs_deletion_and_independently_checks_feasible_witness()
-> TestResult {
    let (_directory, store, outcome, runtime, _app) =
        stored_real_run(many_fixed_snapshot(None)?, ExplanationMode::Standard).await?;
    assert_stored_bound_runtime(&outcome, &runtime)?;
    let state = outcome.state();
    assert!(matches!(
        state
            .manifest
            .as_ref()
            .ok_or("missing stored manifest")?
            .outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Infeasible
        }
    ));
    let evidence = store
        .load_run_infeasibility_evidence(state.input.run_id)
        .await?
        .ok_or("shrinking run did not retain diagnostic evidence")?;
    let proof = evidence.diagnostic.as_ref().ok_or_else(|| {
        diagnostic_failure(
            "shrinking run omitted its native proof",
            &evidence.infeasibility,
        )
    })?;
    assert!(proof.initial_core.len() >= 17);
    assert!(proof.attempted_trials <= proof.max_trials);
    assert!(proof.attempted_trials > 0);
    assert!(proof.last_proven_infeasible.is_none());
    match &evidence.infeasibility {
        InfeasibilityEvidenceV1::Conflict {
            groups,
            minimality,
            shrink,
        } => {
            assert_eq!(groups, &proof.initial_core);
            assert_eq!(shrink.attempted_trials, proof.attempted_trials);
            assert_eq!(
                *minimality,
                if shrink.stop_reason == eutheto_domain_ir::ConflictShrinkStopReason::Completed {
                    eutheto_domain_ir::ConflictMinimality::ProvenMinimal
                } else {
                    eutheto_domain_ir::ConflictMinimality::Sufficient
                },
                "minimality must reflect actual completed proof, not fixture speed"
            );
        }
        InfeasibilityEvidenceV1::Unavailable { .. } => {
            return Err(diagnostic_failure(
                "deletion trials did not retain a sufficient conflict",
                &evidence.infeasibility,
            ));
        }
    }

    let (service, witness_runtime) = real_service().await?;
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let witness = operation
        .prepare(many_fixed_snapshot(Some(0))?)
        .map_err(boxed)?
        .execute(&mut Progress)
        .await
        .map_err(boxed)?;
    assert_bound_runtime(&witness, &witness_runtime)?;
    assert_eq!(witness.execution.terminal_status, SolveStatus::Optimal);
    let accepted = witness
        .accepted_result()
        .ok_or("deleting one required pin did not yield an independently feasible witness")?;
    assert!(accepted.verification.accepted);
    assert_eq!(accepted.verification.score.feasibility, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn explanation_none_does_not_dispatch_the_diagnostic_driver() -> TestResult {
    let (service, runtime) = real_service().await?;
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let outcome = operation
        .prepare(snapshot(2)?)
        .map_err(boxed)?
        .execute(&mut Progress)
        .await
        .map_err(boxed)?;
    assert_bound_runtime(&outcome, &runtime)?;
    assert_eq!(outcome.execution.terminal_status, SolveStatus::Infeasible);
    assert!(
        outcome
            .manifest()
            .phase_timings
            .optional_explanation_milliseconds
            .is_none()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn original_witness_alarm_survives_retained_request_cleanup_failure() -> TestResult {
    let (registry, runtime) = real_registry().await?;
    let inner = Arc::clone(
        registry
            .get(runtime.backend_id())
            .ok_or("missing approved real worker")?,
    );
    let retained = Arc::new(Mutex::new(None));
    let backend: Arc<dyn SolverBackend> = Arc::new(RetainingContradictoryWorker {
        inner,
        retained: Arc::clone(&retained),
        invocations: Arc::new(AtomicUsize::new(0)),
    });
    let registry = SolverRegistry::new(registry.matrix().clone(), [backend])?;
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
            options_with_explanation(ExplanationMode::Standard)?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let outcome = operation
        .prepare(feasible_no_assumption_snapshot()?)
        .map_err(boxed)?
        .execute(&mut Progress)
        .await
        .map_err(boxed)?;
    assert_bound_runtime(&outcome, &runtime)?;
    assert!(matches!(
        outcome.manifest().outcome,
        RunTerminalOutcomeV1::VerificationAlarm { ref diagnostic_code }
            if diagnostic_code == "verification.infeasibility_contradicted"
    ));
    assert_eq!(
        outcome
            .correctness_alarm()
            .ok_or("contradictory witness alarm was lost")?
            .diagnostic_code,
        "verification.infeasibility_contradicted"
    );
    assert!(
        retained
            .lock()
            .map_err(|_| "request custody lock poisoned")?
            .is_some()
    );
    Ok(())
}

async fn run_actual_conditional_worker(
    app: &EuthetoApp,
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    base_solution_id: SolutionId,
    condition: CounterfactualConditionPayloadV1,
) -> TestResult<CounterfactualResult> {
    let AppCommandResult::CounterfactualStarted(started) = app
        .execute(AppCommand::SolutionStartCounterfactual(
            SolutionStartCounterfactualRequestV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision,
                base_solution_id,
                condition: condition.clone(),
                total_budget_milliseconds: DurationMillis::new(30_000)?,
            },
        ))
        .await
        .map_err(boxed)?
    else {
        return Err("expected actual conditional worker start".into());
    };
    for _ in 0..10_000 {
        let job = store
            .load_counterfactual_job(started.job.request.job_id)
            .await?;
        if matches!(
            job.state,
            CounterfactualJobState::Completed
                | CounterfactualJobState::Failed
                | CounterfactualJobState::Cancelled
                | CounterfactualJobState::Interrupted
        ) {
            assert_eq!(job.state, CounterfactualJobState::Completed);
            return Ok(job.result.ok_or("conditional worker omitted its result")?);
        }
        tokio::task::yield_now().await;
    }
    Err("actual conditional worker did not become terminal".into())
}

async fn export_conditional_worker_diagnostic(
    app: &EuthetoApp,
    directory: &Path,
    original: &ScenarioSnapshotV1,
    result: &CounterfactualResult,
    evidence: &RunInfeasibilityEvidenceV1,
) -> TestResult {
    let destination = directory.join("conditional-diagnostic.json");
    let captured = app
        .prepare_diagnostic_export(
            DiagnosticExportRequestV1 {
                schema_version: DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION,
                operation_id: OperationId::new(&SystemIdGenerator)?,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id: original.document.scenario_id,
                source_revision: original.revision,
                run_id: result.run_input.run_id,
                run_manifest_checksum: result.run_manifest.checksum.clone(),
                evidence_checksum: evidence.checksum.clone(),
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    captured
        .prepare_destination(&destination)
        .map_err(boxed)?
        .publish_controlled()
        .map_err(boxed)?;
    let report: DiagnosticReport = serde_json::from_slice(&std::fs::read(destination)?)?;
    assert_eq!(report.condition.as_ref(), Some(&result.request.condition));
    assert_eq!(report.infeasibility, evidence.infeasibility);
    assert_eq!(report.run_id, result.run_input.run_id);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn actual_worker_conditional_questions_preserve_exact_boolean_binding() -> TestResult {
    let original = snapshot(1)?;
    let (directory, store, base, runtime, app) =
        stored_real_run(original.clone(), ExplanationMode::Standard).await?;
    assert_stored_bound_runtime(&base, &runtime)?;
    let base_solution_id = base
        .accepted_result_id()
        .ok_or("feasible base run was not accepted")?;
    let base_result = store.load_accepted_result(base_solution_id).await?;
    let assignment_id = base_result
        .portable
        .accepted_result
        .solution
        .assignments
        .first()
        .ok_or("accepted base run has no assignment")?
        .id
        .clone();
    for assigned in [true, false] {
        let condition = CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: assignment_id.clone(),
            value: AssignmentValue::Boolean(assigned),
        };
        let result = run_actual_conditional_worker(
            &app,
            &store,
            original.document.scenario_id,
            original.revision,
            base_solution_id,
            condition.clone(),
        )
        .await?;
        let retained_evidence = store
            .load_run_infeasibility_evidence(result.run_input.run_id)
            .await?;
        assert!(
            matches!(
                (assigned, &result.run_manifest.outcome, &result.conclusion),
                (
                    true,
                    RunTerminalOutcomeV1::Accepted { .. },
                    eutheto_domain_ir::CounterfactualConclusionV1::VerifiedAlternative { .. }
                ) | (
                    false,
                    RunTerminalOutcomeV1::NoResult {
                        status: SolveStatus::Infeasible
                    },
                    eutheto_domain_ir::CounterfactualConclusionV1::ProvenImpossible
                )
            ),
            "assigned={assigned}, outcome={:?}, conclusion={:?}, diagnostic={:?}",
            result.run_manifest.outcome,
            result.conclusion,
            retained_evidence
                .as_ref()
                .map(|evidence| &evidence.infeasibility)
        );
        match &result.request.condition.condition {
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: actual,
                value,
            } => {
                assert_eq!(actual, &assignment_id);
                assert_eq!(value, &AssignmentValue::Boolean(assigned));
            }
            _ => return Err("conditional worker changed the requested condition".into()),
        }
        if !assigned {
            let evidence = retained_evidence
                .as_ref()
                .ok_or("impossible conditional worker omitted diagnostic evidence")?;
            assert_eq!(evidence.condition.as_ref(), Some(&result.request.condition));
            let coverage_rule: eutheto_types::RuleId = id(32).parse()?;
            assert!(matches!(
                &evidence.infeasibility,
                InfeasibilityEvidenceV1::Conflict { groups, .. }
                    if groups.iter().any(|group| group.required_rules.contains(&coverage_rule))
            ));
            export_conditional_worker_diagnostic(
                &app,
                directory.path(),
                &original,
                &result,
                evidence,
            )
            .await?;
        }
    }
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
        expected_revision: Some(original.revision),
        options: options()?,
        objective_selection: None,
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

    // Export must reverify against the retained solved document, not the now-infeasible current one.
    let exported = app
        .export_solution_json(scenario_id, result_id)
        .await
        .map_err(boxed)?;
    assert_eq!(exported.scenario_revision, original.revision);
    assert_eq!(exported.current_revision, current.summary.revision);
    assert_eq!(
        PortableAcceptedResult::from_json(&exported.bytes)?,
        reopened.portable
    );
    let exported = app
        .export_solution_assignments_csv(scenario_id, result_id)
        .await
        .map_err(boxed)?;
    assert_eq!(exported.scenario_revision, original.revision);
    assert_eq!(exported.current_revision, current.summary.revision);
    let pairs = eutheto_workforce::assignments_csv::decode_assignments_csv(
        &mut exported.bytes.as_slice(),
        &verification_control,
    )?;
    let [pair] = pairs.as_slice() else {
        return Err("export lost the sole retained person/shift assignment".into());
    };
    assert_eq!(pair.person_id.to_string(), id(1));
    assert_eq!(pair.shift_id.to_string(), id(8));

    let exported_ics = app
        .export_solution_ics(scenario_id, result_id, None)
        .await
        .map_err(boxed)?;
    assert_eq!(exported_ics.scenario_revision, original.revision);
    assert_eq!(exported_ics.current_revision, current.summary.revision);
    let ics = std::str::from_utf8(&exported_ics.bytes)?;
    assert!(ics.contains("X-EUTHETO-STALE-AS-OF:TRUE\r\n"));
    assert!(ics.contains("X-EUTHETO-STALE:TRUE\r\n"));
    assert!(ics.contains("X-EUTHETO-STATUS:accepted\r\n"));
    assert!(ics.contains("X-EUTHETO-HISTORICAL-PROOF:UNVERIFIED\r\n"));
    assert!(!ics.contains("X-EUTHETO-CURRENT-REVISION:"));
    assert!(!ics.contains("X-EUTHETO-STATUS:optimal"));
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
    let published = PortableAcceptedResult::from_json(&std::fs::read(&published_path)?)?;
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
        Duration::from_millis(options.time_limit_milliseconds.value() + 1),
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
        expected_revision: Some(original.revision),
        options: options()?,
        objective_selection: None,
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

#[derive(Clone)]
enum WorkerFault {
    FailAfterRealOutcome,
    ClearAssignmentBooleans,
    HoldAfterRealOutcome {
        release: Arc<Notify>,
        held: Arc<Notify>,
        retired: Arc<Notify>,
    },
    HoldThenCorruptRealCandidate {
        release: Arc<Notify>,
        held: Arc<Notify>,
    },
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

    fn verified_worker_manifest_sha256(&self) -> Option<&[u8; 32]> {
        self.inner.verified_worker_manifest_sha256()
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
            match &self.fault {
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
                WorkerFault::HoldAfterRealOutcome {
                    release,
                    held,
                    retired,
                } => {
                    let outcome = self.inner.solve(request, output).await?;
                    let _retired = NotifyOnDrop(Arc::clone(retired));
                    held.notify_one();
                    release.notified().await;
                    Ok(outcome)
                }
                WorkerFault::HoldThenCorruptRealCandidate { release, held } => {
                    let mut capture = CapturingRealOutput {
                        inner: output,
                        last: None,
                    };
                    let outcome = self.inner.solve(request, &mut capture).await?;
                    held.notify_one();
                    release.notified().await;
                    let Some(mut candidate) = capture.last.take() else {
                        return Err(BackendError::new(
                            "tests.missing_real_candidate",
                            "The real worker produced no candidate.",
                        )?);
                    };
                    for value in candidate.values.booleans.values_mut() {
                        *value = false;
                    }
                    capture.inner.submit_candidate(candidate)?;
                    Ok(outcome)
                }
            }
        })
    }
}

struct NotifyOnDrop(Arc<Notify>);

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct CapturingRealOutput<'a> {
    inner: &'a mut dyn BackendOutputSink,
    last: Option<CandidateSubmission>,
}

impl BackendOutputSink for CapturingRealOutput<'_> {
    fn emit_progress(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        self.inner.emit_progress(event)
    }

    fn submit_candidate(
        &mut self,
        candidate: CandidateSubmission,
    ) -> Result<BackendCandidate, OutputError> {
        // Preserve the latest real observation: replaying the first timestamp would be
        // rejected at the output chronology boundary before native domain review.
        self.last = Some(candidate.clone());
        self.inner.submit_candidate(candidate)
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
struct RetainingContradictoryWorker {
    inner: Arc<dyn SolverBackend>,
    retained: Arc<Mutex<Option<SolveRequest>>>,
    invocations: Arc<AtomicUsize>,
}

struct InitialDiscardingOutput;

impl BackendOutputSink for InitialDiscardingOutput {
    fn emit_progress(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }

    fn submit_candidate(
        &mut self,
        candidate: CandidateSubmission,
    ) -> Result<BackendCandidate, OutputError> {
        Ok(BackendCandidate {
            sequence: 1,
            values: candidate.values,
            observed_after_milliseconds: candidate.observed_after_milliseconds,
            objective: candidate.objective,
            evidence_refs: candidate.evidence_refs,
        })
    }
}

impl SolverBackend for RetainingContradictoryWorker {
    fn descriptor(&self) -> &SolverDescriptor {
        self.inner.descriptor()
    }

    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        self.inner.runtime_identity()
    }

    fn verified_worker_manifest_sha256(&self) -> Option<&[u8; 32]> {
        self.inner.verified_worker_manifest_sha256()
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
            if self.invocations.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut discarded = InitialDiscardingOutput;
                let mut outcome = self.inner.solve(request, &mut discarded).await?;
                outcome.termination = BackendTerminationReason::InfeasibilityClaimed;
                outcome.evidence.first_incumbent_milliseconds = None;
                return Ok(outcome);
            }
            {
                let Ok(mut retained) = self.retained.lock() else {
                    return Err(BackendError::new(
                        "tests.request_custody_lock",
                        "The request custody test lock was poisoned.",
                    )?);
                };
                retained.replace(request.clone());
            }
            let mut outcome = self.inner.solve(request, output).await?;
            outcome.termination = BackendTerminationReason::InfeasibilityClaimed;
            Ok(outcome)
        })
    }
}

struct HeldWorkerFixture {
    directory: tempfile::TempDir,
    store: Arc<SqliteScenarioStore>,
    app: EuthetoApp,
    scenario_id: ScenarioId,
    release: Arc<Notify>,
    held: Arc<Notify>,
    retired: Arc<Notify>,
    invocations: Arc<AtomicUsize>,
}

async fn held_worker_fixture() -> TestResult<HeldWorkerFixture> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-headless-checkpoint-")
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
    let release = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    let retired = Arc::new(Notify::new());
    let (registry, invocations) = faulted_real_registry(WorkerFault::HoldAfterRealOutcome {
        release: Arc::clone(&release),
        held: Arc::clone(&held),
        retired: Arc::clone(&retired),
    })
    .await?;
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
            document: original.document,
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        packs,
        Arc::new(registry),
    );
    Ok(HeldWorkerFixture {
        directory,
        store,
        app,
        scenario_id,
        release,
        held,
        retired,
        invocations,
    })
}

async fn wait_for_verified_checkpoint<F>(
    fixture: &HeldWorkerFixture,
    request_id: RequestId,
    solve: &mut Pin<Box<F>>,
) -> TestResult<(SolveRunId, VerifiedCheckpoint)>
where
    F: Future<Output = Result<StoredSolveOutcome, StoredSolveFailure>>,
{
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            tokio::select! {
                result = solve.as_mut() => return Err(format!("worker completed before the first verified checkpoint: {result:?}").into()),
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            let Some(run) = fixture.store.load_solve_run_by_request(request_id).await? else {
                continue;
            };
            if let Some(checkpoint) = fixture
                .app
                .solve_get_verified_checkpoint(fixture.scenario_id, run.input.run_id)
                .await
                .map_err(boxed)?
            {
                assert!(run.manifest.is_none(), "checkpoint must precede terminal proof");
                break Ok::<_, Box<dyn Error>>((run.input.run_id, checkpoint));
            }
        }
    })
    .await?
}

async fn wait_for_held_outcome<F>(
    fixture: &HeldWorkerFixture,
    solve: &mut Pin<Box<F>>,
) -> TestResult
where
    F: Future<Output = Result<StoredSolveOutcome, StoredSolveFailure>>,
{
    // The checkpoint writer runs independently, but the backend producer still
    // needs its owning future polled until it reaches the post-worker hold.
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            () = fixture.held.notified() => Ok(()),
            result = solve.as_mut() => Err(format!("worker completed before the outcome hold: {result:?}").into()),
        }
    })
    .await?
}

async fn assert_checkpoint_view_and_verify(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    checkpoint: &VerifiedCheckpoint,
) -> TestResult<(
    eutheto_core::SolutionVerificationDtoV1,
    eutheto_core::CapturedRecipientReport,
)> {
    let report = recipient_report::capture_checkpoint_report(app, scenario_id, checkpoint).await?;
    let view = app
        .solution_view_with_cancellation(
            SolutionViewRequestV1 {
                schema_version: 1,
                scenario_id,
                solution_id: checkpoint.accepted_result.solution.solution_id,
                view_id: "official.workforce.result.schedule".to_owned(),
                parameters: json!({"kind":"assignments"}),
                continuation: None,
            },
            CancellationToken::new(),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(view.view.data["result"]["kind"], "assignments");
    assert_eq!(view.view.data["result"]["totalItems"], 1);
    let verified = match app
        .query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
            schema_version: 1,
            scenario_id,
            solution_id: checkpoint.accepted_result.solution.solution_id,
        }))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::SolutionVerification(verified) => verified,
        other => return Err(format!("unexpected solution verification result: {other:?}").into()),
    };
    assert_eq!(verified.scenario_id, scenario_id);
    assert_eq!(
        verified.scenario_revision,
        Revision::new(checkpoint.run_input.scenario_revision)
    );
    assert_eq!(verified.result, view.result);
    assert_eq!(
        verified.result.result_checksum,
        checkpoint.accepted_result.checksum
    );
    assert_eq!(
        verified.verification,
        checkpoint.accepted_result.verification
    );
    assert_eq!(
        verified.verification.checksum,
        checkpoint.accepted_result.verification.checksum
    );
    let wrong_scenario = ScenarioId::new(&SystemIdGenerator)?;
    assert!(matches!(
        app.query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
            schema_version: 1,
            scenario_id: wrong_scenario,
            solution_id: checkpoint.accepted_result.solution.solution_id,
        }))
        .await,
        Err(AppError::NotFound(_))
    ));
    Ok((*verified, report))
}

// An abandoned stored-solve caller must not erase a real, already durable checkpoint.
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn first_verified_worker_checkpoint_survives_caller_drop() -> TestResult {
    let fixture = held_worker_fixture().await?;
    let scenario_id = fixture.scenario_id;
    let store = &fixture.store;
    let app = &fixture.app;
    let mut request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(Revision::INITIAL),
        options: options()?,
        objective_selection: None,
    };
    request.options.collect_intermediate_solutions = true;
    let mut progress = Progress;
    let mut solve = Box::pin(app.solve_stored_with_cancellation(
        request.clone(),
        &mut progress,
        CancellationToken::new(),
    ));
    let (run_id, checkpoint) =
        wait_for_verified_checkpoint(&fixture, request.request_id, &mut solve).await?;
    assert_eq!(
        store.load_verified_checkpoint(run_id).await?,
        Some(checkpoint.clone()),
    );
    wait_for_held_outcome(&fixture, &mut solve).await?;
    drop(solve);
    tokio::time::timeout(Duration::from_secs(30), fixture.retired.notified()).await?;
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    let run = store
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("caller-drop run disappeared")?;
    assert_eq!(run.input.run_id, run_id);
    assert!(
        run.manifest.is_none(),
        "dropping the caller must not fabricate a terminal run outcome"
    );
    assert_eq!(
        store.load_verified_checkpoint(run_id).await?,
        Some(checkpoint.clone()),
    );
    assert_eq!(
        app.solve_get_verified_checkpoint(scenario_id, run_id)
            .await
            .map_err(boxed)?,
        Some(checkpoint.clone()),
    );
    assert_checkpoint_view_and_verify(app, scenario_id, &checkpoint).await?;
    let retry = app
        .solve_stored_with_cancellation(request, &mut progress, CancellationToken::new())
        .await
        .map_err(boxed)?;
    assert!(retry.reused);
    assert_eq!(retry.state().input.run_id, run_id);
    assert!(retry.state().manifest.is_none());
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    Ok(())
}

// Preserve the terminal cancellation contract as well as caller-drop custody.
#[allow(clippy::too_many_lines)]
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn first_verified_worker_checkpoint_is_viewable_before_proof_and_survives_cancellation()
-> TestResult {
    let fixture = held_worker_fixture().await?;
    let scenario_id = fixture.scenario_id;
    let mut request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(Revision::INITIAL),
        options: options()?,
        objective_selection: None,
    };
    request.options.collect_intermediate_solutions = true;
    let cancellation = CancellationToken::new();
    let mut progress = Progress;
    let mut solve = Box::pin(fixture.app.solve_stored_with_cancellation(
        request.clone(),
        &mut progress,
        cancellation.clone(),
    ));
    let (run_id, checkpoint) =
        wait_for_verified_checkpoint(&fixture, request.request_id, &mut solve).await?;
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.store.load_verified_checkpoint(run_id).await?,
        Some(checkpoint.clone()),
    );
    wait_for_held_outcome(&fixture, &mut solve).await?;
    let (verified, captured_report) =
        assert_checkpoint_view_and_verify(&fixture.app, scenario_id, &checkpoint).await?;
    let report_control = OperationControl::Cancellation(CancellationToken::new());
    let pending_charge = captured_report
        .retained_bytes(&report_control)
        .map_err(boxed)?;
    assert!(
        fixture
            .store
            .load_solve_run_by_request(request.request_id)
            .await?
            .ok_or("before-proof run disappeared")?
            .manifest
            .is_none(),
        "schedule export must use a before-proof checkpoint"
    );
    for (capture, preview) in recipient_report::capture_checkpoint_schedule_exports(
        &fixture.app,
        scenario_id,
        &checkpoint,
    )
    .await?
    {
        let destination = fixture
            .directory
            .path()
            .join(format!("before-proof-{:?}", preview.format));
        recipient_report::publish_schedule_exact(
            capture,
            preview,
            destination,
            false,
            report_control.clone(),
        )
        .await?;
    }
    let captured_exports = recipient_report::capture_checkpoint_schedule_exports(
        &fixture.app,
        scenario_id,
        &checkpoint,
    )
    .await?;
    let accepted_reference = verified.result.clone();
    let accepted_report = verified.verification.clone();
    let accepted_revision = verified.scenario_revision;
    let accepted_current_revision = verified.current_revision;
    cancellation.cancel();
    fixture.release.notify_one();
    let outcome = solve.await.map_err(boxed)?;
    assert!(matches!(
        outcome
            .state()
            .manifest
            .as_ref()
            .map(|manifest| &manifest.outcome),
        Some(RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Cancelled
        })
    ));
    tokio::time::timeout(Duration::from_secs(30), fixture.retired.notified()).await?;
    let retained = match fixture
        .app
        .query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
            schema_version: 1,
            scenario_id,
            solution_id: checkpoint.accepted_result.solution.solution_id,
        }))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::SolutionVerification(retained) => retained,
        other => return Err(format!("unexpected retained verification result: {other:?}").into()),
    };
    assert_eq!(retained.scenario_id, scenario_id);
    assert_eq!(retained.current_revision, accepted_current_revision);
    assert_eq!(retained.scenario_revision, accepted_revision);
    assert_eq!(retained.result, accepted_reference);
    assert_eq!(retained.verification, accepted_report);
    assert_eq!(
        fixture.store.load_verified_checkpoint(run_id).await?,
        Some(checkpoint.clone()),
    );
    assert_eq!(
        fixture
            .app
            .solve_get_verified_checkpoint(scenario_id, run_id)
            .await
            .map_err(boxed)?,
        Some(checkpoint),
    );
    let settled_charge = captured_report
        .retained_bytes(&report_control)
        .map_err(boxed)?;
    assert!(
        settled_charge <= pending_charge,
        "late native observations outgrew their reservation"
    );
    for (capture, preview) in captured_exports {
        let destination = fixture
            .directory
            .path()
            .join(format!("after-cancellation-{:?}", preview.format));
        recipient_report::publish_schedule_exact(
            capture,
            preview,
            destination,
            false,
            report_control.clone(),
        )
        .await?;
    }
    Ok(())
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
    let native = outcome
        .manifest()
        .native_observations
        .as_ref()
        .ok_or("missing failed-attempt native observations")?;
    assert!(
        native
            .attempt
            .as_ref()
            .ok_or("missing failed native attempt")?
            .outcome
            .is_none(),
        "a real candidate without an admitted terminal must not manufacture native outcome/runtime claims"
    );
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
        expected_revision: Some(original.revision),
        options: options()?,
        objective_selection: None,
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

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
// Keep this native-alarm lifecycle together: it proves checkpoint refusal through capacity and replay.
#[allow(clippy::too_many_lines)]
async fn later_native_alarm_refuses_checkpoint_consumers_when_quarantine_rolls_back() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("eutheto.sqlite");
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: database.clone(),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let release = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    let (registry, invocations) =
        faulted_real_registry(WorkerFault::HoldThenCorruptRealCandidate {
            release: Arc::clone(&release),
            held: Arc::clone(&held),
        })
        .await?;
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (store, initialization) = SqliteScenarioStore::open(&database).await?;
    let store = Arc::new(store);
    store
        .create_project(NewProject {
            document: original.document,
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        packs,
        Arc::new(registry),
    );
    let mut request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(Revision::INITIAL),
        options: options()?,
        objective_selection: None,
    };
    request.options.collect_intermediate_solutions = true;
    let mut progress = Progress;
    let (checkpoint, solution_id, view_request, captured_report, captured_exports, failure) = {
        let solve = app.solve_stored(request.clone(), &mut progress);
        tokio::pin!(solve);
        let checkpoint = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                tokio::select! {
                    result = &mut solve => return Err(boxed(result)),
                    () = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
                if let Some(run) = store.load_solve_run_by_request(request.request_id).await?
                    && let Some(checkpoint) = app
                        .solve_get_verified_checkpoint(scenario_id, run.input.run_id)
                        .await
                        .map_err(boxed)?
                {
                    return Ok::<_, Box<dyn Error>>(checkpoint);
                }
            }
        })
        .await??;
        let solution_id = checkpoint.accepted_result.solution.solution_id;
        let view_request = SolutionViewRequestV1 {
            schema_version: 1,
            scenario_id,
            solution_id,
            view_id: "official.workforce.result.schedule".to_owned(),
            parameters: json!({"kind":"assignments"}),
            continuation: None,
        };
        let before = app
            .solution_view_with_cancellation(view_request.clone(), CancellationToken::new())
            .await
            .map_err(boxed)?;
        assert_eq!(before.view.data["result"]["totalItems"], 1);
        let captured_report =
            recipient_report::capture_checkpoint_report(&app, scenario_id, &checkpoint).await?;
        let captured_exports =
            recipient_report::capture_checkpoint_schedule_exports(&app, scenario_id, &checkpoint)
                .await?;
        // Fail every quarantine attempt, including cleanup: the transaction must roll back and
        // retain its actual first checkpoint. Native custody, not the DB status, must then refuse it.
        rusqlite::Connection::open(&database)?.execute_batch(
            "CREATE TRIGGER reject_alarm_write BEFORE UPDATE ON solve_runs
         WHEN NEW.status = 'quarantined'
         BEGIN SELECT RAISE(ABORT, 'injected quarantine failure'); END;",
        )?;
        release.notify_one();
        let failure = solve.await;
        (
            checkpoint,
            solution_id,
            view_request,
            captured_report,
            captured_exports,
            failure,
        )
    };
    let terminal = failure
        .as_ref()
        .ok()
        .and_then(|outcome| outcome.state().manifest.as_ref())
        .map(|manifest| &manifest.outcome);
    let attempts = failure
        .as_ref()
        .ok()
        .and_then(|outcome| outcome.report.as_ref())
        .map(|report| {
            report
                .execution
                .attempts
                .iter()
                .map(|attempt| (&attempt.termination, &attempt.backend_failure_code))
                .collect::<Vec<_>>()
        });
    assert!(
        failure.is_err(),
        "quarantine trigger must reject durable finalization: {terminal:?}; {attempts:?}"
    );
    let run = store
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("actual run disappeared")?;
    assert!(run.manifest.is_none());
    assert_eq!(
        store.load_verified_checkpoint(run.input.run_id).await?,
        Some(checkpoint.clone())
    );
    repair::assert_quarantined_checkpoint_repair_refused(&app, &store, &checkpoint, run.started_at)
        .await?;
    let assert_alarm = |result: Result<(), AppError>| -> TestResult {
        let Err(AppError::Verification(failure)) = result else {
            return Err(format!("expected refused native source: {result:?}").into());
        };
        let known = failure.known_alarm.ok_or("native alarm binding was lost")?;
        assert_eq!(known.run_id(), run.input.run_id);
        assert_eq!(known.input_checksum(), run.input.checksum);
        assert_eq!(
            known.diagnostic_code().as_str(),
            "verification.required_rule_rejected"
        );
        assert_eq!(
            known.terminalization(),
            eutheto_types::VerificationAlarmTerminalization::Unconfirmed
        );
        Ok(())
    };
    let report_path = directory.path().join("recipient-alarm.html");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_alarm(
        captured_report
            .publish_html(
                report_path.clone(),
                false,
                OperationControl::Cancellation(cancelled),
            )
            .await
            .map(|_| ()),
    )?;
    assert!(!report_path.exists());
    assert_alarm(
        app.prepare_recipient_report(
            eutheto_core::RecipientReportRequestV1 {
                schema_version: 1,
                scenario_id,
                solution_id,
                expected_revision: Revision::INITIAL,
                options: eutheto_types::RecipientReportOptionsV1::default(),
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map(|_| ()),
    )?;
    for (capture, preview) in captured_exports {
        assert_alarm(capture.admit())?;
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let control = OperationControl::Cancellation(cancelled);
        assert_alarm(capture.retained_bytes(&control).map(|_| ()))?;
        let export_path = directory
            .path()
            .join(format!("schedule-alarm-{:?}", preview.format));
        assert_alarm(
            capture
                .publish(export_path.clone(), false, control)
                .await
                .map(|_| ()),
        )?;
        assert!(!export_path.exists());
        assert_alarm(
            app.prepare_schedule_export(
                eutheto_core::ScheduleExportRequestV1 {
                    schema_version: 1,
                    scenario_id,
                    solution_id,
                    expected_revision: Revision::INITIAL,
                    format: preview.format,
                    person_id: None,
                },
                OperationControl::Cancellation(CancellationToken::new()),
            )
            .await
            .map(|_| ()),
        )?;
    }
    let before_retry = invocations.load(Ordering::SeqCst);
    let replay = app.solve_stored(request.clone(), &mut progress).await;
    let Err(StoredSolveFailure::BeforeStart(error)) = replay else {
        return Err(format!("exact reuse escaped native custody: {replay:?}").into());
    };
    assert_alarm(Err(error))?;
    assert_eq!(invocations.load(Ordering::SeqCst), before_retry);
    assert_alarm(
        app.solve_get_verified_checkpoint(scenario_id, run.input.run_id)
            .await
            .map(|_| ()),
    )?;
    assert_alarm(
        app.solution_view_with_cancellation(view_request, CancellationToken::new())
            .await
            .map(|_| ()),
    )?;
    assert_alarm(
        app.export_solution_json(scenario_id, solution_id)
            .await
            .map(|_| ()),
    )?;
    let reference = AcceptedResultRefV1::from_result(&checkpoint.accepted_result)?;
    let assignment_id = checkpoint
        .accepted_result
        .solution
        .assignments
        .first()
        .ok_or("real accepted assignment disappeared")?
        .id
        .clone();
    assert_alarm(
        app.query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: 1,
                scenario_id,
                request: ExplanationRequestV1::new(ExplanationRequestSubjectV1::Assignment {
                    result: reference.clone(),
                    assignment_id,
                })?,
            },
            source_revision: Revision::new(run.input.scenario_revision),
            control: OperationControl::Cancellation(CancellationToken::new()),
        })
        .await
        .map(|_| ()),
    )?;
    assert_alarm(
        app.query(AppQuery::SolutionCompareV3(SolutionCompareRequestV3 {
            schema_version: SOLUTION_COMPARE_SCHEMA_VERSION,
            scenario_id,
            base_solution_id: SolutionId::new(&SystemIdGenerator)?,
            candidate_solution_id: solution_id,
        }))
        .await
        .map(|_| ()),
    )?;
    let absent_reference = AcceptedResultRefV1 {
        solution_id: SolutionId::new(&SystemIdGenerator)?,
        result_checksum: reference.result_checksum.clone(),
    };
    assert_alarm(
        app.query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                request: ExplanationRequestV1::new(
                    ExplanationRequestSubjectV1::SolutionDifference {
                        left: absent_reference.clone(),
                        right: reference.clone(),
                    },
                )?,
            },
            source_revision: Revision::new(run.input.scenario_revision),
            control: OperationControl::Cancellation(CancellationToken::new()),
        })
        .await
        .map(|_| ()),
    )?;
    assert_alarm(
        app.query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                request: ExplanationRequestV1::new(ExplanationRequestSubjectV1::Repair {
                    current: reference.clone(),
                    base: absent_reference.clone(),
                })?,
            },
            source_revision: Revision::new(run.input.scenario_revision),
            control: OperationControl::Cancellation(CancellationToken::new()),
        })
        .await
        .map(|_| ()),
    )?;
    assert!(matches!(
        app.query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                request: ExplanationRequestV1::new(ExplanationRequestSubjectV1::Repair {
                    current: reference,
                    base: absent_reference,
                })?,
            },
            source_revision: Revision::new(run.input.scenario_revision + 1),
            control: OperationControl::Cancellation(CancellationToken::new()),
        })
        .await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        app.export_solution_json(ScenarioId::new(&SystemIdGenerator)?, solution_id)
            .await,
        Err(AppError::NotFound(_))
    ));
    // Unconfirmed native alarms remain charged after both work owners drain. Fill the
    // current sixteen-owner boundary with real reviewed failures, not synthetic alarm JSON.
    for _ in 1..16 {
        let mut additional = request.clone();
        additional.request_id = RequestId::new(&SystemIdGenerator)?;
        release.notify_one();
        let outcome = app.solve_stored(additional.clone(), &mut progress).await;
        let Err(
            StoredSolveFailure::FinalizationUnconfirmed {
                known_alarm: Some(known),
                ..
            }
            | StoredSolveFailure::AfterStart {
                known_alarm: Some(known),
                ..
            },
        ) = outcome
        else {
            let terminal = outcome
                .as_ref()
                .ok()
                .and_then(|outcome| outcome.state().manifest.as_ref())
                .map(|manifest| &manifest.outcome);
            return Err(
                format!("failed quarantine did not retain native custody: {terminal:?}").into(),
            );
        };
        let retained = store
            .load_solve_run_by_request(additional.request_id)
            .await?
            .ok_or("charged alarm lost its actual run")?;
        assert_eq!(known.run_id(), retained.input.run_id);
        assert_eq!(known.input_checksum(), retained.input.checksum);
        assert_eq!(
            known.diagnostic_code().as_str(),
            "verification.required_rule_rejected"
        );
        assert_eq!(
            known.terminalization(),
            eutheto_types::VerificationAlarmTerminalization::Unconfirmed
        );
        assert!(retained.manifest.is_none());
    }
    assert_eq!(invocations.load(Ordering::SeqCst), 16);
    let mut overflow = request.clone();
    overflow.request_id = RequestId::new(&SystemIdGenerator)?;
    let refusal = app.solve_stored(overflow.clone(), &mut progress).await;
    assert!(matches!(
        refusal,
        Err(StoredSolveFailure::BeforeStart(AppError::Protocol(ref error)))
            if error.code == "operation.resource_limit"
    ));
    assert!(
        store
            .load_solve_run_by_request(overflow.request_id)
            .await?
            .is_none()
    );
    assert_eq!(invocations.load(Ordering::SeqCst), 16);
    // Capacity must not replace a scoped exact-reuse alarm with a generic slot error.
    let replay = app.solve_stored(request, &mut progress).await;
    let Err(StoredSolveFailure::BeforeStart(error)) = replay else {
        return Err(format!("full-capacity exact reuse escaped custody: {replay:?}").into());
    };
    assert_alarm(Err(error))?;
    assert_eq!(invocations.load(Ordering::SeqCst), 16);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn later_native_alarm_retains_observations_after_successful_quarantine() -> TestResult {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("eutheto.sqlite");
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: database.clone(),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let release = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    let (registry, invocations) =
        faulted_real_registry(WorkerFault::HoldThenCorruptRealCandidate {
            release: Arc::clone(&release),
            held: Arc::clone(&held),
        })
        .await?;
    let packs = Arc::new(
        DomainPackRegistry::builder()
            .register(eutheto_workforce::WorkforcePack)
            .build()?,
    );
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (store, initialization) = SqliteScenarioStore::open(&database).await?;
    let store = Arc::new(store);
    store
        .create_project(NewProject {
            document: original.document,
        })
        .await?;
    let app = EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&store),
        initialization,
        dependencies,
        packs,
        Arc::new(registry),
    );
    let mut request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(Revision::INITIAL),
        options: options()?,
        objective_selection: None,
    };
    request.options.collect_intermediate_solutions = true;
    let mut progress = Progress;
    let mut solve = Box::pin(app.solve_stored(request.clone(), &mut progress));
    let (run_id, checkpoint) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            () = held.notified() => {}
            result = solve.as_mut() => {
                return Err(format!("worker completed before the real candidate hold: {result:?}").into());
            }
        }
        loop {
            if let Some(run) = store.load_solve_run_by_request(request.request_id).await?
                && let Some(checkpoint) = app
                    .solve_get_verified_checkpoint(scenario_id, run.input.run_id)
                    .await
                    .map_err(boxed)?
            {
                assert!(run.manifest.is_none(), "checkpoint must precede terminal proof");
                break Ok::<_, Box<dyn Error>>((run.input.run_id, checkpoint));
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.load_verified_checkpoint(run_id).await?,
        Some(checkpoint.clone()),
    );

    release.notify_one();
    let outcome = solve.await.map_err(boxed)?;
    let manifest = outcome
        .state()
        .manifest
        .as_ref()
        .ok_or("successful quarantine did not retain its terminal manifest")?;
    assert_eq!(manifest.run_id, run_id);
    assert_eq!(manifest.run_input_checksum, checkpoint.run_input.checksum);
    assert!(matches!(
        &manifest.outcome,
        RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }
            if diagnostic_code == "verification.required_rule_rejected"
    ));
    let native = manifest
        .native_observations
        .as_ref()
        .ok_or("successful quarantine dropped actual native observations")?;
    assert_eq!(
        native.terminal_reason,
        RunNativeRouterReason::VerificationQuarantined
    );
    let attempt = native
        .attempt
        .as_ref()
        .ok_or("actual admitted worker attempt was not retained")?;
    assert_eq!(attempt.backend_id, ORTOOLS_BACKEND_ID.parse()?);
    assert!(attempt.invocation_index.is_some());
    assert!(attempt.approved_worker_manifest_sha256.is_some());
    assert!(matches!(
        &attempt.termination,
        RunNativeAttemptTermination::VerificationQuarantined { diagnostic_code }
            if diagnostic_code == "verification.required_rule_rejected"
    ));
    assert!(
        attempt
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.runtime.as_ref())
            .is_some(),
        "native runtime requires the real worker's admitted terminal and parameter digest"
    );

    let retained = store
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("successful quarantine lost the durable request")?;
    assert_eq!(retained.input.run_id, run_id);
    let retained_manifest = retained
        .manifest
        .as_ref()
        .ok_or("successful quarantine lost its durable alarm")?;
    assert_eq!(
        retained.input.checksum,
        retained_manifest.run_input_checksum
    );
    assert_eq!(retained_manifest, manifest);
    assert!(store.load_verified_checkpoint(run_id).await?.is_none());
    assert!(
        app.solve_get_verified_checkpoint(scenario_id, run_id)
            .await
            .map_err(boxed)?
            .is_none(),
        "a quarantined candidate must not remain publishable as a checkpoint"
    );

    let expected_manifest = manifest.clone();
    drop(app);
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&database).await?;
    let reopened_run = reopened
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("reopened store lost the alarmed request")?;
    assert_eq!(reopened_run.input.run_id, run_id);
    let reopened_manifest = reopened_run
        .manifest
        .as_ref()
        .ok_or("reopened store lost the durable alarm manifest")?;
    assert_eq!(
        reopened_run.input.checksum,
        reopened_manifest.run_input_checksum
    );
    assert_eq!(reopened_manifest, &expected_manifest);
    assert!(matches!(
        &reopened_manifest.outcome,
        RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }
            if diagnostic_code == "verification.required_rule_rejected"
    ));
    let reopened_native = reopened_manifest
        .native_observations
        .as_ref()
        .ok_or("reopened store lost native observations")?;
    assert_eq!(reopened_native, native);
    assert!(reopened.load_verified_checkpoint(run_id).await?.is_none());
    Ok(())
}
