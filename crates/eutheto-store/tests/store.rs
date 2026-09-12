use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, AssignmentValue,
    COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION, ComparisonContext, ComparisonRunManifests,
    CounterfactualCompilationBindingV1, CounterfactualConclusionV1,
    CounterfactualConditionPayloadV1, CounterfactualConditionV1, CounterfactualFailureKind,
    CounterfactualJobErrorV1, CounterfactualJobRequestV1, CounterfactualJobState,
    CounterfactualRequestSemanticsV1, CounterfactualResultV1, DomainAssignment, DomainAssignmentId,
    DomainEntityId, DomainEntityKindId, DomainEntityRef, NORMALIZED_SOLUTION_SCHEMA_VERSION,
    NormalizedSolution, PortableAcceptedResultV2, RunManifestV1, RunPhaseTimingsV1,
    RunTerminalOutcomeV1, ScoreVector, VerificationContextV1, VerificationReport,
    VerificationScope, blake3_hex, compare_accepted_results,
};
use eutheto_export::{ApplicationMetadata, validate_scenario_snapshot};
use eutheto_import::{
    ImportProvenance, PreviewBinding, RestoreAuthorization, RestoreMode, SafetyBackupEvidence,
    StagedBackupRestore, StagedDisposition, StagedImport, StagedScenario,
};
use eutheto_store::{
    AppSetting, CandidateDiagnosticsV1, CommandWrite, CounterfactualCancelOutcomeV1,
    CounterfactualJobTransitionV1, CounterfactualRunFinalizationV1, JournalWrite,
    MAX_ACTIVE_COUNTERFACTUAL_JOBS, NewProject, NewSolveRunV1, OpenOptions, ProjectListScope,
    RedoBranchPolicy, SafetyBackupFailureReceipt, SnapshotPolicy, SqliteScenarioStore,
    StagedLibraryApply, StoreError, StoredProject, ensure_private_application_directory,
};
#[cfg(debug_assertions)]
use eutheto_store::{
    CommandCommitTestHook, CommandCommitTestPhase, Failpoint, V2MigrationBeginTestHook,
    V3MigrationBeginTestHook, V4MigrationBeginTestHook,
};
use eutheto_types::{
    ActorRef, BackendId, BackendSelection, BundleId, CancellationToken, CommandId, CommandSource,
    CounterfactualJobId, DomainPackRef, DurationMillis, EntityId, ExplanationMode, GapPolicy,
    Horizon, IanaTimeZone, LocaleTag, MAX_SCENARIO_DOCUMENT_BYTES, OverlapPolicy, PackId,
    PortableAsset, PortableProjectMetadata, PreservationPolicy, ReproducibilityMode, RequestId,
    ResourceLimits, Revision, Rfc3339Timestamp, RuleId, ScenarioDocument, ScenarioDomain,
    ScenarioId, ScenarioMetadata, ScenarioSettings, ScenarioSnapshotV1, SemanticCapability,
    SolutionId, SolveMode, SolveOptions, SolveRunId, SolveStatus, SupplementalIdentity,
    SupplementalSectionKind, UnitSystem, WorkerThreadPolicy,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::tempdir;
use uuid::Uuid;

const CREATED: &str = "2026-08-28T23:00:00Z";
const UPDATED: &str = "2026-08-28T23:01:00Z";
const LATER: &str = "2026-08-28T23:02:00Z";

fn scenario_id(suffix: u16) -> Result<ScenarioId, uuid::Error> {
    let text = format!("018f47f2-e880-7000-8000-{suffix:012x}");
    Uuid::parse_str(&text).map(ScenarioId::from_uuid)
}

fn pack_id() -> Result<PackId, eutheto_types::NamespacedIdError> {
    PackId::new("official.test")
}

fn timestamp(value: &str) -> Result<Rfc3339Timestamp, jiff::Error> {
    Rfc3339Timestamp::parse(value)
}
fn shift_timestamp(
    value: Rfc3339Timestamp,
    milliseconds: u64,
) -> Result<Rfc3339Timestamp, jiff::Error> {
    value
        .as_timestamp()
        .checked_add(std::time::Duration::from_millis(milliseconds))
        .map(Rfc3339Timestamp::from_timestamp)
}

fn document(id: ScenarioId) -> Result<ScenarioDocument, Box<dyn Error>> {
    Ok(ScenarioDocument::new(
        id,
        DomainPackRef {
            id: pack_id()?,
            schema_version: 1,
        },
        ScenarioMetadata {
            title: "Clinic plan".to_owned(),
            description: "A persisted generic scenario".to_owned(),
            created_at: timestamp(CREATED)?,
            updated_at: timestamp(CREATED)?,
        },
        ScenarioSettings {
            time_zone: IanaTimeZone::parse("America/Chicago")?,
            locale: LocaleTag::parse("en-US")?,
            units: UnitSystem::UsCustomary,
            horizon: Horizon::new(
                timestamp("2026-09-01T05:00:00Z")?,
                timestamp("2026-10-01T05:00:00Z")?,
            )?,
            gap_policy: GapPolicy::Reject,
            overlap_policy: OverlapPolicy::Earlier,
        },
        ScenarioDomain::default(),
        BTreeMap::from([("vendor.example".to_owned(), json!({"preserved": true}))]),
    ))
}

fn solve_options() -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(120_000)?,
        memory_limit_bytes: Some(64 * 1024 * 1024),
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 7,
        solution_limit: Some(1),
        stop_after_first_feasible: true,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::Standard,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 1_000,
            max_rules: 1_000,
            max_variables: 10_000,
            max_constraints: 10_000,
        },
    })
}

fn solve_request(
    scenario_id: ScenarioId,
    run_suffix: u16,
    request_suffix: u16,
) -> Result<NewSolveRunV1, Box<dyn Error>> {
    Ok(NewSolveRunV1 {
        run_id: SolveRunId::from_uuid(Uuid::parse_str(&format!(
            "018f47f2-e880-7000-8001-{run_suffix:012x}"
        ))?),
        request_id: RequestId::from_uuid(Uuid::parse_str(&format!(
            "018f47f2-e880-7000-8002-{request_suffix:012x}"
        ))?),
        scenario_id,
        expected_revision: Revision::INITIAL,
        planning_ir_schema_version: 2,
        compiler_version: "1.0.0".to_owned(),
        application_version: "1.0.0".to_owned(),
        backend_id: BackendId::new("ortools.cp-sat")?,
        backend_version: "9.15.0".to_owned(),
        adapter_version: "1.0.0".to_owned(),
        worker_version: "1.0.0".to_owned(),
        solver_version: "9.15.0".to_owned(),
        protocol_major: 1,
        protocol_minor: 0,
        model_hash: blake3_hex(b"planning-model"),
        objective_policy_hash: blake3_hex(b"objective-policy"),
        solve_options: solve_options()?,
        temporary_condition_hash: None,
        started_at: Rfc3339Timestamp::from_timestamp(jiff::Timestamp::now()),
    })
}

fn accepted_result_for(
    input: &eutheto_domain_ir::RunInputV1,
    solution_suffix: u16,
) -> Result<AcceptedResult, Box<dyn Error>> {
    let solution = NormalizedSolution {
        schema_version: NORMALIZED_SOLUTION_SCHEMA_VERSION,
        pack_id: input.pack_id.clone(),
        scenario_id: input.scenario_id,
        scenario_revision: input.scenario_revision,
        projection_version: 1,
        solution_id: SolutionId::from_uuid(Uuid::parse_str(&format!(
            "018f47f2-e880-7000-8003-{solution_suffix:012x}"
        ))?),
        assignments: vec![DomainAssignment {
            id: DomainAssignmentId::new("assignment.target")?,
            entity: DomainEntityRef {
                kind: DomainEntityKindId::new("tests.entity")?,
                id: DomainEntityId::new("tests.entity")?,
            },
            value: AssignmentValue::Boolean(true),
            evidence: Vec::new(),
        }],
    };
    let scope = VerificationScope::new(input.scenario_id, input.scenario_revision, Vec::new())?;
    let context = VerificationContextV1::new(
        input.scenario_id,
        input.scenario_revision,
        input.snapshot_document_hash.clone(),
        input.model_hash.clone(),
        solution.canonical_hash()?,
        scope.checksum,
    )?;
    let report = VerificationReport::new(
        &context,
        Vec::new(),
        ScoreVector {
            feasibility: 0,
            levels: Vec::new(),
        },
        Vec::new(),
        BTreeMap::new(),
    )?;
    Ok(AcceptedResult::new(solution, report)?)
}

fn terminal_manifest(
    input: &eutheto_domain_ir::RunInputV1,
    started_at: Rfc3339Timestamp,
    outcome: RunTerminalOutcomeV1,
) -> Result<RunManifestV1, Box<dyn Error>> {
    let accepted = matches!(&outcome, RunTerminalOutcomeV1::Accepted { .. });
    let interrupted = matches!(&outcome, RunTerminalOutcomeV1::Interrupted);
    let finished_at = shift_timestamp(started_at, 1_000)?;
    Ok(RunManifestV1::new(
        input.run_id,
        input.checksum.clone(),
        outcome,
        started_at,
        finished_at,
        (!interrupted)
            .then(|| DurationMillis::new(1_000))
            .transpose()?,
        None,
        accepted.then(|| DurationMillis::new(500)).transpose()?,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?)
}

fn counterfactual_job_id(suffix: u16) -> Result<CounterfactualJobId, uuid::Error> {
    Uuid::parse_str(&format!("018f47f2-e880-7000-8004-{suffix:012x}"))
        .map(CounterfactualJobId::from_uuid)
}

fn counterfactual_request_id(suffix: u16) -> Result<RequestId, uuid::Error> {
    Uuid::parse_str(&format!("018f47f2-e880-7000-8005-{suffix:012x}")).map(RequestId::from_uuid)
}

struct CounterfactualStoreFixture {
    request: CounterfactualJobRequestV1,
    base_input: eutheto_domain_ir::RunInputV1,
    base_manifest: RunManifestV1,
    base_accepted: AcceptedResult,
}

async fn counterfactual_store_fixture(
    store: &SqliteScenarioStore,
    suffix: u16,
) -> Result<CounterfactualStoreFixture, Box<dyn Error>> {
    let scenario_id = scenario_id(700_u16.saturating_add(suffix))?;
    store
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;
    let base = store
        .start_solve_run(solve_request(
            scenario_id,
            700_u16.saturating_add(suffix),
            700_u16.saturating_add(suffix),
        )?)
        .await?;
    let accepted = accepted_result_for(&base.input, 700_u16.saturating_add(suffix))?;
    let base_manifest = terminal_manifest(
        &base.input,
        base.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(accepted.clone(), base_manifest.clone(), BTreeMap::new())
        .await?;
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: DomainAssignmentId::new("assignment.target")?,
            value: AssignmentValue::Boolean(true),
        })?;
    let semantics = CounterfactualRequestSemanticsV1 {
        schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
        scenario_id,
        scenario_revision: base.input.scenario_revision,
        snapshot_id: base.input.snapshot_id,
        snapshot_document_hash: base.input.snapshot_document_hash.clone(),
        base: AcceptedResultRefV1::from_result(&accepted)?,
        base_run_id: base.input.run_id,
        base_run_input_checksum: base.input.checksum.clone(),
        base_model_hash: base.input.model_hash.clone(),
        objective_policy_hash: base.input.objective_policy_hash.clone(),
        condition_checksum: condition.checksum.clone(),
        total_budget_milliseconds: DurationMillis::new(5_000)?,
    };
    let request = CounterfactualJobRequestV1::new(
        counterfactual_job_id(suffix)?,
        counterfactual_request_id(suffix)?,
        semantics,
        condition,
        base_manifest.finished_at,
    )?;
    Ok(CounterfactualStoreFixture {
        request,
        base_input: base.input,
        base_manifest,
        base_accepted: accepted,
    })
}

fn retry_counterfactual_request(
    fixture: &CounterfactualStoreFixture,
    suffix: u16,
) -> Result<CounterfactualJobRequestV1, Box<dyn Error>> {
    Ok(CounterfactualJobRequestV1::new(
        counterfactual_job_id(suffix)?,
        counterfactual_request_id(suffix)?,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        fixture.request.created_at,
    )?)
}

async fn counterfactual_result(
    store: &SqliteScenarioStore,
    fixture: &CounterfactualStoreFixture,
    suffix: u16,
    persist_terminal: bool,
) -> Result<CounterfactualResultV1, Box<dyn Error>> {
    let mut request = solve_request(
        fixture.request.semantics.scenario_id,
        800_u16.saturating_add(suffix),
        800_u16.saturating_add(suffix),
    )?;
    request.model_hash = blake3_hex(format!("derived-model-{suffix}").as_bytes());
    request.objective_policy_hash = fixture.request.semantics.objective_policy_hash.clone();
    request.temporary_condition_hash = Some(fixture.request.condition.checksum.clone());
    request.solve_options.time_limit_milliseconds =
        fixture.request.semantics.total_budget_milliseconds;
    request.started_at = fixture.request.created_at;
    let derived = store.start_solve_run(request).await?;
    let manifest = terminal_manifest(
        &derived.input,
        derived.started_at,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::NoSolutionWithinLimit,
        },
    )?;
    if persist_terminal {
        store.finalize_terminal_run(manifest.clone()).await?;
    }
    let compilation = CounterfactualCompilationBindingV1::new(
        fixture.request.semantics.base_model_hash.clone(),
        fixture.request.condition.checksum.clone(),
        derived.input.model_hash.clone(),
        fixture.request.semantics.objective_policy_hash.clone(),
    )?;
    Ok(CounterfactualResultV1::new(
        fixture.request.clone(),
        fixture.base_input.clone(),
        fixture.base_manifest.clone(),
        compilation,
        derived.input,
        manifest,
        CounterfactualConclusionV1::NotDistinguishedWithinBudget,
    )?)
}

async fn accepted_counterfactual_results(
    store: &SqliteScenarioStore,
    fixture: &CounterfactualStoreFixture,
    suffix: u16,
) -> Result<(CounterfactualResultV1, CounterfactualResultV1), Box<dyn Error>> {
    let mut request = solve_request(
        fixture.request.semantics.scenario_id,
        900_u16.saturating_add(suffix),
        900_u16.saturating_add(suffix),
    )?;
    request.model_hash = blake3_hex(format!("accepted-derived-model-{suffix}").as_bytes());
    request.objective_policy_hash = fixture.request.semantics.objective_policy_hash.clone();
    request.temporary_condition_hash = Some(fixture.request.condition.checksum.clone());
    request.solve_options.time_limit_milliseconds =
        fixture.request.semantics.total_budget_milliseconds;
    request.started_at = fixture.request.created_at;
    let derived = store.start_solve_run(request).await?;
    let accepted = accepted_result_for(&derived.input, 900_u16.saturating_add(suffix))?;
    let manifest = terminal_manifest(
        &derived.input,
        derived.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(accepted.clone(), manifest.clone(), BTreeMap::new())
        .await?;
    let compilation = CounterfactualCompilationBindingV1::new(
        fixture.request.semantics.base_model_hash.clone(),
        fixture.request.condition.checksum.clone(),
        derived.input.model_hash.clone(),
        fixture.request.semantics.objective_policy_hash.clone(),
    )?;
    let exact_comparison = compare_accepted_results(
        &fixture.base_accepted,
        &accepted,
        Some(&ComparisonContext {
            locks: &[],
            manifests: Some(ComparisonRunManifests {
                base: &fixture.base_manifest,
                candidate: &manifest,
            }),
        }),
    )?;
    let forged_comparison = compare_accepted_results(
        &fixture.base_accepted,
        &accepted,
        Some(&ComparisonContext {
            locks: &[],
            manifests: None,
        }),
    )?;
    let alternative = AcceptedResultRefV1::from_result(&accepted)?;
    let result = |comparison: eutheto_domain_ir::SolutionComparisonV1| {
        let ordering = comparison.ordering;
        CounterfactualResultV1::new(
            fixture.request.clone(),
            fixture.base_input.clone(),
            fixture.base_manifest.clone(),
            compilation.clone(),
            derived.input.clone(),
            manifest.clone(),
            CounterfactualConclusionV1::VerifiedAlternative {
                alternative: alternative.clone(),
                comparison: Box::new(comparison),
                ordering,
            },
        )
    };
    Ok((result(forged_comparison)?, result(exact_comparison)?))
}

fn owned_counterfactual_solve_request(
    fixture: &CounterfactualStoreFixture,
    suffix: u16,
) -> Result<NewSolveRunV1, Box<dyn Error>> {
    let mut request = solve_request(
        fixture.request.semantics.scenario_id,
        1_100_u16.saturating_add(suffix),
        1_100_u16.saturating_add(suffix),
    )?;
    request.started_at = fixture.request.created_at;
    request.model_hash = blake3_hex(format!("owned-derived-model-{suffix}").as_bytes());
    request
        .objective_policy_hash
        .clone_from(&fixture.request.semantics.objective_policy_hash);
    request.temporary_condition_hash = Some(fixture.request.condition.checksum.clone());
    request.solve_options = fixture.base_input.solve_options.clone();
    request.solve_options.backend =
        BackendSelection::Specific(fixture.base_input.backend_id.clone());
    request.solve_options.time_limit_milliseconds =
        fixture.request.semantics.total_budget_milliseconds;
    Ok(request)
}

async fn start_owned_counterfactual_run(
    store: &SqliteScenarioStore,
    fixture: &CounterfactualStoreFixture,
    suffix: u16,
) -> Result<eutheto_store::StartedSolveRunV1, Box<dyn Error>> {
    store
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    store
        .transition_counterfactual_job(
            fixture.request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: fixture.request.created_at,
            },
        )
        .await?;
    Ok(store
        .start_counterfactual_run(
            fixture.request.job_id,
            owned_counterfactual_solve_request(fixture, suffix)?,
        )
        .await?)
}

fn completed_no_result_finalization(
    fixture: &CounterfactualStoreFixture,
    started: &eutheto_store::StartedSolveRunV1,
) -> Result<CounterfactualRunFinalizationV1, Box<dyn Error>> {
    let manifest = terminal_manifest(
        &started.input,
        started.started_at,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::NoSolutionWithinLimit,
        },
    )?;
    let compilation = CounterfactualCompilationBindingV1::new(
        fixture.request.semantics.base_model_hash.clone(),
        fixture.request.condition.checksum.clone(),
        started.input.model_hash.clone(),
        fixture.request.semantics.objective_policy_hash.clone(),
    )?;
    let result = CounterfactualResultV1::new(
        fixture.request.clone(),
        fixture.base_input.clone(),
        fixture.base_manifest.clone(),
        compilation,
        started.input.clone(),
        manifest.clone(),
        CounterfactualConclusionV1::NotDistinguishedWithinBudget,
    )?;
    Ok(CounterfactualRunFinalizationV1::CompletedNoResult {
        manifest,
        result: Box::new(result),
    })
}

fn completed_accepted_finalization(
    fixture: &CounterfactualStoreFixture,
    started: &eutheto_store::StartedSolveRunV1,
    suffix: u16,
) -> Result<CounterfactualRunFinalizationV1, Box<dyn Error>> {
    let accepted = accepted_result_for(&started.input, 1_100_u16.saturating_add(suffix))?;
    let manifest = terminal_manifest(
        &started.input,
        started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    let compilation = CounterfactualCompilationBindingV1::new(
        fixture.request.semantics.base_model_hash.clone(),
        fixture.request.condition.checksum.clone(),
        started.input.model_hash.clone(),
        fixture.request.semantics.objective_policy_hash.clone(),
    )?;
    let comparison = compare_accepted_results(
        &fixture.base_accepted,
        &accepted,
        Some(&ComparisonContext {
            locks: &[],
            manifests: Some(ComparisonRunManifests {
                base: &fixture.base_manifest,
                candidate: &manifest,
            }),
        }),
    )?;
    let ordering = comparison.ordering;
    let result = CounterfactualResultV1::new(
        fixture.request.clone(),
        fixture.base_input.clone(),
        fixture.base_manifest.clone(),
        compilation,
        started.input.clone(),
        manifest.clone(),
        CounterfactualConclusionV1::VerifiedAlternative {
            alternative: AcceptedResultRefV1::from_result(&accepted)?,
            comparison: Box::new(comparison),
            ordering,
        },
    )?;
    Ok(CounterfactualRunFinalizationV1::CompletedAccepted {
        accepted_result: Box::new(accepted),
        evidence: BTreeMap::new(),
        manifest,
        result: Box::new(result),
    })
}

fn set_marker(
    mut document: ScenarioDocument,
    marker: Option<i64>,
    updated_at: &str,
) -> Result<ScenarioDocument, StoreError> {
    match marker {
        Some(value) => {
            document
                .extensions
                .insert("test.marker".to_owned(), json!(value));
        }
        None => {
            document.extensions.remove("test.marker");
        }
    }
    document.metadata.updated_at = Rfc3339Timestamp::parse(updated_at)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    Ok(document)
}

fn journal(
    command: Value,
    inverse: Option<Value>,
    created_at: &str,
) -> Result<JournalWrite, StoreError> {
    Ok(JournalWrite {
        command_type: "set_marker".to_owned(),
        command,
        command_id: CommandId::from_uuid(Uuid::now_v7()),
        inverse,
        actor: ActorRef {
            actor_id: Some("store-test".to_owned()),
            display_name: "Store test".to_owned(),
        },
        source: CommandSource::System,
        summary: "Set marker".to_owned(),
        created_at: Rfc3339Timestamp::parse(created_at)
            .map_err(|error| StoreError::Integrity(error.to_string()))?,
    })
}

fn staged_import(
    local_library_revision: Revision,
    scenarios: Vec<(Revision, ScenarioDocument, StagedDisposition)>,
    source_created_at: Rfc3339Timestamp,
) -> StagedImport {
    StagedImport {
        binding: PreviewBinding {
            file_sha256: "staged-test".to_owned(),
            options_sha256: "staged-test".to_owned(),
            local_library_revision,
            format_version: 1,
            schema_version: 1,
        },
        mode: RestoreMode::ImportScenario,
        scenarios: scenarios
            .into_iter()
            .map(|(revision, document, disposition)| StagedScenario {
                original_id: document.scenario_id,
                source_revision: revision,
                disposition,
                scenario: ScenarioSnapshotV1::current(revision, document, BTreeSet::new()),
                id_remap: BTreeMap::new(),
            })
            .collect(),
        scenario_revisions: Vec::new(),
        results: BTreeMap::new(),
        shared_records: BTreeMap::new(),
        preferences: BTreeMap::new(),
        manifest_extensions: BTreeMap::new(),
        nonsemantic_extensions: BTreeSet::new(),
        assets: BTreeMap::new(),
        supplemental_replacements: BTreeSet::new(),
        provenance: ImportProvenance {
            source_bundle_id: BundleId::from_uuid(Uuid::now_v7()),
            source_application: ApplicationMetadata {
                name: "store-test".to_owned(),
                version: "1".to_owned(),
            },
            original_format_version: 1,
            original_schema_version: 1,
            source_file_sha256: "staged-test".to_owned(),
            source_created_at,
            applied_migrations: Vec::new(),
        },
    }
}

#[tokio::test]
async fn malformed_open_rolls_back_last_opened_and_library_revision() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(94)?;
    let expected = document(id)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: expected.clone(),
        })
        .await?;
    let before_open = store.library_metadata_snapshot().await?;
    let opened = store.open_project(id, timestamp(UPDATED)?).await?;
    assert_eq!(opened.summary.last_opened_at, Some(timestamp(UPDATED)?));
    assert_eq!(opened.document, expected);
    let before_failure = store.library_metadata_snapshot().await?;
    assert_eq!(
        before_failure.revision,
        before_open.revision.checked_next()?
    );

    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE scenarios SET document_json = 'not-json' WHERE id = ?1",
        [id.to_string()],
    )?;
    assert!(matches!(
        store.open_project(id, timestamp(LATER)?).await,
        Err(StoreError::Json(_))
    ));
    assert_eq!(store.library_metadata_snapshot().await?, before_failure);
    let last_opened: String = connection.query_row(
        "SELECT last_opened_at FROM scenarios WHERE id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(timestamp(&last_opened)?, timestamp(UPDATED)?);
    connection.execute(
        "UPDATE scenarios SET document_json = ?2 WHERE id = ?1",
        params![id.to_string(), serde_json::to_string(&expected)?],
    )?;
    drop(connection);
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(reopened.library_metadata_snapshot().await?, before_failure);
    let persisted = reopened.get_project(id).await?;
    assert_eq!(persisted.summary.last_opened_at, Some(timestamp(UPDATED)?));
    assert_eq!(persisted.document, expected);
    Ok(())
}

#[tokio::test]
async fn staged_apply_cancelled_before_work_preserves_library() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let (store, _) = SqliteScenarioStore::open(directory.path().join("library.sqlite3")).await?;
    let staged = staged_import(
        Revision::INITIAL,
        vec![(
            Revision::new(1),
            document(scenario_id(95)?)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(staged),
                timestamp(LATER)?,
                cancellation,
            )
            .await,
        Err(StoreError::OperationCancelled)
    ));
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.revision, Revision::INITIAL);
    assert!(snapshot.projects.is_empty());
    assert!(snapshot.scenario_revision_high_water.is_empty());
    assert!(snapshot.provenance.is_empty());
    Ok(())
}

#[tokio::test]
async fn project_crud_and_document_survive_reopen() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(1)?;
    let expected = document(id)?;

    let (store, first_start) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(first_start.applied_migrations, vec![1, 2, 3, 4, 5]);
    let created = store
        .create_project(NewProject {
            document: expected.clone(),
        })
        .await?;
    assert_eq!(created.summary.revision, Revision::INITIAL);
    assert_eq!(created.document, expected);
    store
        .archive_project(id, Revision::INITIAL, timestamp(UPDATED)?)
        .await?;
    assert_eq!(
        store.list_projects(ProjectListScope::Active).await?.len(),
        0
    );
    assert_eq!(
        store.list_projects(ProjectListScope::Archived).await?.len(),
        1
    );
    store.unarchive_project(id, Revision::INITIAL).await?;
    drop(store);

    let (reopened, second_start) = SqliteScenarioStore::open(&path).await?;
    assert!(second_start.applied_migrations.is_empty());
    let persisted = reopened.get_project(id).await?;
    assert_eq!(persisted.summary.revision, Revision::INITIAL);
    assert_eq!(persisted.document, expected);
    assert_eq!(
        reopened
            .list_projects(ProjectListScope::Active)
            .await?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn direct_create_rejects_duplicate_identity_occurrences_without_artifacts()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let root_id = scenario_id(110)?;
    let mut root_collision = document(root_id)?;
    let root_person = EntityId::from_uuid(root_id.as_uuid());
    root_collision
        .domain
        .entities
        .insert(root_person, json!({"id": root_person}));
    let entity_rule_id = Uuid::now_v7();
    let mut typed_collision = document(scenario_id(111)?)?;
    let person = EntityId::from_uuid(entity_rule_id);
    let rule = RuleId::from_uuid(entity_rule_id);
    typed_collision
        .domain
        .entities
        .insert(person, json!({"id": person}));
    typed_collision
        .domain
        .rules
        .insert(rule, json!({"id": rule}));

    let (store, _) = SqliteScenarioStore::open(&path).await?;
    for invalid in [root_collision, typed_collision] {
        assert!(matches!(
            store.create_project(NewProject { document: invalid }).await,
            Err(StoreError::InvalidScenarioIdentity(_))
        ));
        let snapshot = store.library_snapshot().await?;
        assert_eq!(snapshot.revision, Revision::INITIAL);
        assert!(snapshot.projects.is_empty());
        assert!(snapshot.scenario_revision_high_water.is_empty());
    }
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let snapshot = reopened.library_snapshot().await?;
    assert!(snapshot.projects.is_empty());
    assert!(snapshot.scenario_revision_high_water.is_empty());
    Ok(())
}

#[tokio::test]
async fn staged_apply_rejects_global_identity_collision_atomically() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let (store, _) = SqliteScenarioStore::open(&directory.path().join("library.sqlite3")).await?;
    let id = scenario_id(91)?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let before = store.library_snapshot().await?;
    let mut staged = staged_import(before.revision, Vec::new(), timestamp(CREATED)?);
    let key = format!("{id}.json");
    staged
        .shared_records
        .insert(key.clone(), br#"{"value":"synthetic"}"#.to_vec());

    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(staged),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::IdentityCollision(_))
    ));
    let after = store.library_snapshot().await?;
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.projects.len(), before.projects.len());
    assert_eq!(after.projects[0].summary.id, before.projects[0].summary.id);
    assert_eq!(
        after.projects[0].summary.revision,
        before.projects[0].summary.revision
    );
    assert_eq!(after.projects[0].document, before.projects[0].document);
    assert!(!after.sections.shared_records.contains_key(&key));
    Ok(())
}

#[tokio::test]
async fn command_rejects_identity_owned_by_another_project_atomically() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let (store, _) = SqliteScenarioStore::open(&directory.path().join("library.sqlite3")).await?;
    let owner_id = scenario_id(92)?;
    let target_id = scenario_id(93)?;
    let shared_uuid = scenario_id(94)?.as_uuid();
    let person_id: EntityId = shared_uuid.to_string().parse()?;
    let rule_id: RuleId = shared_uuid.to_string().parse()?;
    let mut owner = document(owner_id)?;
    owner.domain.entities.insert(
        person_id,
        json!({"id": shared_uuid, "name": "existing owner"}),
    );
    store.create_project(NewProject { document: owner }).await?;
    store
        .create_project(NewProject {
            document: document(target_id)?,
        })
        .await?;
    let before = store.library_snapshot().await?;

    assert!(matches!(
        store
            .execute_command(
                target_id,
                Revision::INITIAL,
                RedoBranchPolicy::Reject,
                CancellationToken::new(),
                move |current| {
                    let mut updated = current.clone();
                    updated
                        .domain
                        .rules
                        .insert(rule_id, json!({"id": shared_uuid, "required": true}));
                    Ok(CommandWrite {
                        document: updated,
                        journal: journal(json!({"addRule": shared_uuid}), None, UPDATED)?,
                        output: (),
                    })
                },
            )
            .await,
        Err(StoreError::IdentityCollision(id)) if id == shared_uuid
    ));
    let after = store.library_snapshot().await?;
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        store.get_project(target_id).await?.summary.revision,
        Revision::INITIAL
    );
    assert!(
        store
            .get_project(target_id)
            .await?
            .document
            .domain
            .rules
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn removed_identity_remains_reserved_for_undo() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let (store, _) = SqliteScenarioStore::open(&directory.path().join("library.sqlite3")).await?;
    let owner_id = scenario_id(95)?;
    let target_id = scenario_id(96)?;
    let reserved_uuid = scenario_id(97)?.as_uuid();
    let person_id: EntityId = reserved_uuid.to_string().parse()?;
    let rule_id: RuleId = reserved_uuid.to_string().parse()?;
    store
        .create_project(NewProject {
            document: document(owner_id)?,
        })
        .await?;
    store
        .execute_command(
            owner_id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            move |current| {
                let mut updated = current.clone();
                updated.domain.entities.insert(
                    person_id,
                    json!({"id": reserved_uuid, "name": "temporarily present"}),
                );
                Ok(CommandWrite {
                    document: updated,
                    journal: journal(
                        json!({"addPerson": reserved_uuid}),
                        Some(json!({"removePerson": reserved_uuid})),
                        UPDATED,
                    )?,
                    output: (),
                })
            },
        )
        .await?;
    store
        .execute_command(
            owner_id,
            Revision::new(1),
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            move |current| {
                let mut updated = current.clone();
                updated.domain.entities.remove(&person_id);
                Ok(CommandWrite {
                    document: updated,
                    journal: journal(
                        json!({"removePerson": reserved_uuid}),
                        Some(json!({"addPerson": reserved_uuid})),
                        LATER,
                    )?,
                    output: (),
                })
            },
        )
        .await?;
    store
        .create_project(NewProject {
            document: document(target_id)?,
        })
        .await?;

    assert_eq!(
        store
            .library_snapshot()
            .await?
            .scenario_identity_owners
            .get(&reserved_uuid),
        Some(&owner_id)
    );
    assert!(matches!(
        store
            .execute_command(
                target_id,
                Revision::INITIAL,
                RedoBranchPolicy::Reject,
                CancellationToken::new(),
                move |current| {
                    let mut updated = current.clone();
                    updated.domain.rules.insert(
                        rule_id,
                        json!({"id": reserved_uuid, "required": true}),
                    );
                    Ok(CommandWrite {
                        document: updated,
                        journal: journal(json!({"addRule": reserved_uuid}), None, LATER)?,
                        output: (),
                    })
                },
            )
            .await,
        Err(StoreError::IdentityCollision(id)) if id == reserved_uuid
    ));

    store
        .undo(
            owner_id,
            Revision::new(2),
            timestamp(LATER)?,
            move |history| {
                let mut restored = history.document;
                restored.domain.entities.insert(
                    person_id,
                    json!({"id": reserved_uuid, "name": "restored by undo"}),
                );
                Ok((restored, ()))
            },
        )
        .await?;
    assert!(
        store
            .get_project(owner_id)
            .await?
            .document
            .domain
            .entities
            .contains_key(&person_id)
    );
    Ok(())
}

#[tokio::test]
async fn publication_revision_lease_blocks_other_store_instances() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (first, _) = SqliteScenarioStore::open(&path).await?;
    let (second, _) = SqliteScenarioStore::open(&path).await?;
    let first = Arc::new(first);
    let second = Arc::new(second);
    let expected = first.library_metadata_snapshot().await?.revision;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let lease_store = Arc::clone(&first);
    let lease = tokio::spawn(async move {
        lease_store
            .with_publication_revision_lease(expected, None, move || {
                let _ = entered_tx.send(());
                release_rx.recv()?;
                Ok::<(), std::sync::mpsc::RecvError>(())
            })
            .await
    });
    entered_rx.await?;

    let (attempt_tx, attempt_rx) = tokio::sync::oneshot::channel();
    let updated = timestamp(UPDATED)?;
    let mutation_store = Arc::clone(&second);
    let mutation = tokio::spawn(async move {
        let _ = attempt_tx.send(());
        mutation_store
            .replace_settings(
                &["appearance"],
                mutation_store.settings_snapshot(&["appearance"]).await?,
                BTreeMap::from([(
                    "appearance".to_owned(),
                    AppSetting {
                        value: json!({"theme": "dark"}),
                        updated_at: updated,
                    },
                )]),
                CancellationToken::new(),
            )
            .await
    });
    attempt_rx.await?;
    tokio::task::yield_now().await;
    assert!(!mutation.is_finished());
    release_tx.send(())?;
    lease.await???;
    mutation.await??;

    let next_revision = expected.checked_next()?;
    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_flag = Arc::clone(&callback_called);
    assert!(matches!(
        first
            .with_publication_revision_lease(expected, None, move || {
                callback_flag.store(true, Ordering::SeqCst);
                Ok::<(), std::convert::Infallible>(())
            })
            .await,
        Err(StoreError::LibraryConflict {
            expected: stale,
            actual
        }) if stale == expected && actual == next_revision
    ));
    assert!(!callback_called.load(Ordering::SeqCst));
    Ok(())
}

async fn apply_and_assert_revision_bound_import(
    store: &SqliteScenarioStore,
    existing_id: ScenarioId,
    created_id: ScenarioId,
    copied_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    assert_eq!(store.library_snapshot().await?.revision, Revision::INITIAL);
    store
        .create_project(NewProject {
            document: document(existing_id)?,
        })
        .await?;

    let mut replacement = document(existing_id)?;
    "Replaced plan".clone_into(&mut replacement.metadata.title);
    let staged = staged_import(
        Revision::new(1),
        vec![
            (Revision::new(7), replacement, StagedDisposition::Replace),
            (
                Revision::new(8),
                document(created_id)?,
                StagedDisposition::Create,
            ),
            (
                Revision::new(9),
                document(copied_id)?,
                StagedDisposition::CreateCopy,
            ),
        ],
        timestamp(CREATED)?,
    );
    let outcome = store
        .apply_staged_library(
            StagedLibraryApply::Import(staged.clone()),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(outcome.library_revision, Revision::new(2));
    assert_eq!(outcome.created, 2);
    assert_eq!(outcome.replaced, 1);
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.revision, Revision::new(2));
    assert_eq!(snapshot.projects.len(), 3);
    assert_eq!(
        store
            .get_project(existing_id)
            .await?
            .document
            .metadata
            .title,
        "Replaced plan"
    );
    assert_eq!(
        store.get_project(existing_id).await?.summary.revision,
        Revision::new(7)
    );
    assert_eq!(
        store.get_project(created_id).await?.summary.revision,
        Revision::new(8)
    );
    assert_eq!(
        store.get_project(copied_id).await?.summary.revision,
        Revision::new(9)
    );
    assert!(matches!(
        store
            .apply_staged_library(StagedLibraryApply::Import(staged), timestamp(LATER)?, CancellationToken::new())
            .await,
        Err(StoreError::LibraryConflict { expected, actual })
            if expected == Revision::new(1) && actual == Revision::new(2)
    ));
    Ok(())
}

async fn assert_failed_staged_imports_are_atomic(
    store: &SqliteScenarioStore,
    existing_id: ScenarioId,
    created_id: ScenarioId,
    rolled_back_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let impossible = staged_import(
        Revision::new(2),
        vec![
            (
                Revision::new(10),
                document(rolled_back_id)?,
                StagedDisposition::Create,
            ),
            (
                Revision::new(7),
                document(existing_id)?,
                StagedDisposition::Replace,
            ),
        ],
        timestamp(CREATED)?,
    );
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(impossible),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    assert!(matches!(
        store.get_project(rolled_back_id).await,
        Err(StoreError::ScenarioNotFound(id)) if id == rolled_back_id
    ));
    let preserved = store.get_project(existing_id).await?;
    assert_eq!(preserved.summary.revision, Revision::new(7));
    assert_eq!(preserved.document.metadata.title, "Replaced plan");
    assert_eq!(store.library_snapshot().await?.revision, Revision::new(2));

    let invalid = staged_import(
        Revision::new(2),
        vec![
            (
                Revision::new(10),
                document(rolled_back_id)?,
                StagedDisposition::Create,
            ),
            (
                Revision::new(11),
                document(created_id)?,
                StagedDisposition::Create,
            ),
        ],
        timestamp(CREATED)?,
    );
    assert!(matches!(
        store
            .apply_staged_library(StagedLibraryApply::Import(invalid), timestamp(LATER)?, CancellationToken::new())
            .await,
        Err(StoreError::ScenarioAlreadyExists(id)) if id == created_id
    ));
    assert!(matches!(
        store.get_project(rolled_back_id).await,
        Err(StoreError::ScenarioNotFound(id)) if id == rolled_back_id
    ));
    assert_eq!(store.library_snapshot().await?.revision, Revision::new(2));
    Ok(())
}

async fn apply_and_assert_backup_restore(
    store: &SqliteScenarioStore,
    existing_id: ScenarioId,
    created_id: ScenarioId,
    copied_id: ScenarioId,
    restored_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let mut replacement = staged_import(
        Revision::new(2),
        vec![(
            Revision::new(12),
            document(restored_id)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    replacement.mode = RestoreMode::ReplaceLibrary;
    let restore = StagedBackupRestore {
        import: replacement,
        remove_scenario_ids: BTreeSet::from([existing_id, created_id, copied_id]),
        authorization: RestoreAuthorization {
            destructive_action_confirmed: true,
            prospective_failure_receipt_token: None,
            collision_plan_sha256: Some("a".repeat(64)),
            safety_backup: SafetyBackupEvidence::Verified {
                bundle_sha256: "verified-test-backup".to_owned(),
            },
        },
    };
    let outcome = store
        .apply_staged_library(
            StagedLibraryApply::BackupRestore {
                restore,
                settings: BTreeMap::new(),
            },
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(outcome.library_revision, Revision::new(3));
    assert_eq!(outcome.removed, 3);
    assert_eq!(store.library_snapshot().await?.projects.len(), 1);
    let restored = store.get_project(restored_id).await?;
    assert_eq!(restored.document.scenario_id, restored_id);
    assert_eq!(restored.summary.revision, Revision::new(12));
    Ok(())
}

#[tokio::test]
async fn staged_library_apply_is_atomic_and_revision_bound() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let existing_id = scenario_id(20)?;
    let created_id = scenario_id(21)?;
    let rolled_back_id = scenario_id(22)?;
    let copied_id = scenario_id(23)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;

    apply_and_assert_revision_bound_import(&store, existing_id, created_id, copied_id).await?;
    assert_failed_staged_imports_are_atomic(&store, existing_id, created_id, rolled_back_id)
        .await?;
    apply_and_assert_backup_restore(&store, existing_id, created_id, copied_id, rolled_back_id)
        .await?;
    Ok(())
}

async fn seed_portable_import(
    store: &SqliteScenarioStore,
    local_id: ScenarioId,
) -> Result<BundleId, Box<dyn Error>> {
    store
        .create_project(NewProject {
            document: document(local_id)?,
        })
        .await?;
    store
        .replace_settings(
            &["appearance"],
            store.settings_snapshot(&["appearance"]).await?,
            BTreeMap::from([(
                "appearance".to_owned(),
                AppSetting {
                    value: json!({"theme": "dark"}),
                    updated_at: timestamp(UPDATED)?,
                },
            )]),
            CancellationToken::new(),
        )
        .await?;
    let mut imported = staged_import(
        Revision::new(2),
        vec![(
            Revision::new(7),
            document(local_id)?,
            StagedDisposition::Replace,
        )],
        timestamp(CREATED)?,
    );
    imported.results.insert(
        "result-a".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": "018f1e2d-3c4b-7a69-8def-200000000001",
            "scenarioId": local_id,
            "scenarioRevision": 7,
            "value": 1
        }))?,
    );
    imported
        .manifest_extensions
        .insert("vendor.manifest".to_owned(), json!({"preserved": true}));
    imported
        .nonsemantic_extensions
        .insert("vendor.manifest".to_owned());
    imported
        .shared_records
        .insert("shared-a".to_owned(), br#"{"value":2}"#.to_vec());
    imported
        .preferences
        .insert("preference-a".to_owned(), br#"{"value":3}"#.to_vec());
    imported.assets.insert(
        "asset-a".to_owned(),
        PortableAsset {
            bytes: b"asset bytes".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: false,
        },
    );
    imported.scenarios[0]
        .scenario
        .required_capabilities
        .insert(SemanticCapability {
            id: "vendor.semantic".to_owned(),
            version: 2,
        });
    imported.scenarios[0]
        .scenario
        .semantic_extensions
        .insert("vendor.semantic".to_owned(), json!({"meaning": 1}));
    imported.scenarios[0]
        .scenario
        .extensions
        .insert("vendor.display".to_owned(), json!({"color": "blue"}));
    let source_bundle_id = imported.provenance.source_bundle_id;
    store
        .apply_staged_library(
            StagedLibraryApply::Import(imported),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    Ok(source_bundle_id)
}

async fn apply_add_backup(
    store: &SqliteScenarioStore,
    archived_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let mut added = staged_import(
        Revision::new(3),
        vec![(
            Revision::new(12),
            document(archived_id)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    added.mode = RestoreMode::AddBackup;
    added.scenarios[0].scenario.project = Some(PortableProjectMetadata {
        archived_at: Some(timestamp(UPDATED)?),
    });
    added.results.insert(
        "result-b".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": archived_id,
            "scenarioRevision": 12,
            "value": 4
        }))?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::BackupRestore {
                restore: StagedBackupRestore {
                    import: added,
                    remove_scenario_ids: BTreeSet::new(),
                    authorization: RestoreAuthorization {
                        destructive_action_confirmed: false,
                        prospective_failure_receipt_token: None,
                        collision_plan_sha256: None,
                        safety_backup: SafetyBackupEvidence::NotRequired,
                    },
                },
                settings: BTreeMap::from([(
                    "backup-added".to_owned(),
                    AppSetting {
                        value: json!({"enabled": true}),
                        updated_at: timestamp(UPDATED)?,
                    },
                )]),
            },
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    Ok(())
}

fn failed_receipt_restore(
    local_revision: Revision,
    scenario_id: ScenarioId,
    proof: &str,
    collision_plan_sha256: &str,
) -> Result<StagedLibraryApply, Box<dyn Error>> {
    let mut import = staged_import(
        local_revision,
        vec![(
            Revision::new(5),
            document(scenario_id)?,
            StagedDisposition::Replace,
        )],
        timestamp(CREATED)?,
    );
    import.mode = RestoreMode::ReplaceLibrary;
    Ok(StagedLibraryApply::BackupRestore {
        restore: StagedBackupRestore {
            import,
            remove_scenario_ids: BTreeSet::from([scenario_id]),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: Some(collision_plan_sha256.to_owned()),
                safety_backup: SafetyBackupEvidence::FailedWithStrongConfirmation {
                    proof: proof.to_owned(),
                },
            },
        },
        settings: BTreeMap::new(),
    })
}

fn staged_apply_binding(staged: &StagedLibraryApply) -> PreviewBinding {
    match staged {
        StagedLibraryApply::Import(import) => import.binding.clone(),
        StagedLibraryApply::BackupRestore { restore, .. } => restore.import.binding.clone(),
    }
}

fn receipt_count(path: &std::path::Path) -> Result<i64, rusqlite::Error> {
    Connection::open(path)?.query_row(
        "SELECT COUNT(*) FROM safety_backup_failure_receipts",
        [],
        |row| row.get(0),
    )
}

async fn record_failure_receipt(
    store: &SqliteScenarioStore,
    proof: &str,
    binding: PreviewBinding,
    collision_plan_sha256: &str,
) -> Result<(), Box<dyn Error>> {
    store
        .record_safety_backup_failure_receipt(SafetyBackupFailureReceipt {
            proof: proof.to_owned(),
            binding,
            collision_plan_sha256: collision_plan_sha256.to_owned(),
            safe_reason: "destination refused the backup".to_owned(),
            created_at: timestamp(UPDATED)?,
        })
        .await?;
    Ok(())
}

#[cfg(debug_assertions)]
// Keep the final cancellation boundary and reopened durable state in one fixture.
#[allow(clippy::too_many_lines)]
async fn staged_apply_cancellation_at_final_boundary(
    phase: CommandCommitTestPhase,
    change: bool,
) -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(96)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store
        .replace_settings(
            &["appearance"],
            store.settings_snapshot(&["appearance"]).await?,
            BTreeMap::from([(
                "appearance".to_owned(),
                AppSetting {
                    value: json!({"theme": "dark"}),
                    updated_at: timestamp(UPDATED)?,
                },
            )]),
            CancellationToken::new(),
        )
        .await?;
    let before = store.library_snapshot().await?;
    let staged = if change {
        let proof = "cancelled-restore-receipt";
        let plan = "d".repeat(64);
        let staged = failed_receipt_restore(before.revision, id, proof, &plan)?;
        record_failure_receipt(&store, proof, staged_apply_binding(&staged), &plan).await?;
        staged
    } else {
        StagedLibraryApply::Import(staged_import(
            before.revision,
            Vec::new(),
            timestamp(CREATED)?,
        ))
    };
    drop(store);
    let hook = CommandCommitTestHook::new(phase);
    let (store, _) = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::default().with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let cancellation = CancellationToken::new();
    let actor_cancellation = cancellation.clone();
    let actor_store = store.clone();
    let applied_at = timestamp(LATER)?;
    let command = tokio::spawn(async move {
        actor_store
            .apply_staged_library(staged, applied_at, actor_cancellation)
            .await
    });
    let wait_hook = hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_until_reached()).await?;
    cancellation.cancel();
    hook.release();
    let result = command.await?;
    let committed = phase == CommandCommitTestPhase::AfterFinalCancellationCheck;
    let changed = committed && change;
    let expected_revision = if changed {
        before.revision.checked_next()?
    } else {
        before.revision
    };
    if committed {
        let outcome = result?;
        assert_eq!(outcome.library_revision, expected_revision);
        assert_eq!(outcome.created, 0);
        assert_eq!(outcome.replaced, usize::from(change));
        assert_eq!(outcome.removed, usize::from(change));
    } else {
        assert!(matches!(result, Err(StoreError::OperationCancelled)));
    }
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let durable = reopened.library_snapshot().await?;
    assert_eq!(durable.revision, expected_revision);
    assert_eq!(durable.projects.len(), 1);
    assert_eq!(durable.projects[0].document, before.projects[0].document);
    let expected_project_revision = if changed {
        Revision::new(5)
    } else {
        Revision::INITIAL
    };
    assert_eq!(
        durable.projects[0].summary.revision,
        expected_project_revision
    );
    assert_eq!(
        durable.scenario_revision_high_water.get(&id),
        Some(&expected_project_revision)
    );
    assert_eq!(
        durable.settings,
        if changed {
            BTreeMap::new()
        } else {
            before.settings
        }
    );
    assert_eq!(durable.provenance.len(), usize::from(changed));
    assert_eq!(receipt_count(&path)?, i64::from(change && !committed));
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn staged_apply_final_cancellation_rolls_back_restore_and_cancels_noop()
-> Result<(), Box<dyn Error>> {
    staged_apply_cancellation_at_final_boundary(
        CommandCommitTestPhase::BeforeFinalCancellationCheck,
        true,
    )
    .await?;
    staged_apply_cancellation_at_final_boundary(
        CommandCommitTestPhase::BeforeFinalCancellationCheck,
        false,
    )
    .await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn staged_apply_completion_wins_cancellation_after_final_check_including_noop()
-> Result<(), Box<dyn Error>> {
    staged_apply_cancellation_at_final_boundary(
        CommandCommitTestPhase::AfterFinalCancellationCheck,
        true,
    )
    .await?;
    staged_apply_cancellation_at_final_boundary(
        CommandCommitTestPhase::AfterFinalCancellationCheck,
        false,
    )
    .await
}

#[tokio::test]
async fn failure_receipt_persists_restarts_consumes_once_and_is_bounded()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(92)?;
    let proof = "owner-private-first-use-proof";
    let plan = "a".repeat(64);
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let staged = failed_receipt_restore(Revision::new(1), id, proof, &plan)?;
    record_failure_receipt(&store, proof, staged_apply_binding(&staged), &plan).await?;
    drop(store);

    let connection = Connection::open(&path)?;
    let (stored_hash, stored_binding, stored_plan, stored_reason): (
        String,
        Vec<u8>,
        String,
        String,
    ) = connection.query_row(
        "SELECT proof_sha256, binding_json, collision_plan_sha256, safe_reason FROM safety_backup_failure_receipts",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(stored_hash, eutheto_export::sha256_hex(proof.as_bytes()));
    assert!(!String::from_utf8(stored_binding)?.contains(proof));
    assert!(!stored_plan.contains(proof));
    assert!(!stored_reason.contains(proof));
    drop(connection);
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .apply_staged_library(staged, timestamp(LATER)?, CancellationToken::new())
        .await?;
    assert_eq!(receipt_count(&path)?, 0);

    let replay = failed_receipt_restore(Revision::new(2), id, proof, &plan)?;
    assert!(matches!(
        store
            .apply_staged_library(replay, timestamp(LATER)?, CancellationToken::new())
            .await,
        Err(StoreError::SafetyBackupFailureReceiptRejected)
    ));
    for index in 0..17 {
        let proof = format!("bounded-proof-{index}");
        let binding = staged_import(Revision::new(2), Vec::new(), timestamp(CREATED)?).binding;
        record_failure_receipt(&store, &proof, binding, &plan).await?;
    }
    assert_eq!(receipt_count(&path)?, 16);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn failure_receipt_mismatch_and_failed_apply_do_not_consume() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(93)?;
    let proof = "rollback-bound-proof";
    let plan = "b".repeat(64);
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let staged = failed_receipt_restore(Revision::new(1), id, proof, &plan)?;
    record_failure_receipt(&store, proof, staged_apply_binding(&staged), &plan).await?;

    let plan_mismatch = failed_receipt_restore(Revision::new(1), id, proof, &"c".repeat(64))?;
    assert!(matches!(
        store
            .apply_staged_library(plan_mismatch, timestamp(LATER)?, CancellationToken::new())
            .await,
        Err(StoreError::SafetyBackupFailureReceiptRejected)
    ));
    let mut binding_mismatch = staged.clone();
    if let StagedLibraryApply::BackupRestore { restore, .. } = &mut binding_mismatch {
        restore.import.binding.options_sha256 = "different-binding".to_owned();
    }
    assert!(matches!(
        store
            .apply_staged_library(
                binding_mismatch,
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::SafetyBackupFailureReceiptRejected)
    ));
    assert_eq!(receipt_count(&path)?, 1);
    assert_eq!(
        store.get_project(id).await?.summary.revision,
        Revision::INITIAL
    );

    store.set_failpoint(Failpoint::AfterSupplementalWrite)?;
    assert!(matches!(
        store
            .apply_staged_library(staged.clone(), timestamp(LATER)?, CancellationToken::new())
            .await,
        Err(StoreError::InjectedFailure)
    ));
    assert_eq!(receipt_count(&path)?, 1);
    assert_eq!(
        store.get_project(id).await?.summary.revision,
        Revision::INITIAL
    );
    store
        .apply_staged_library(staged, timestamp(LATER)?, CancellationToken::new())
        .await?;
    assert_eq!(receipt_count(&path)?, 0);
    Ok(())
}

#[tokio::test]
async fn portable_sections_wrapper_metadata_and_provenance_survive_reopen()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let local_id = scenario_id(30)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let source_bundle_id = seed_portable_import(&store, local_id).await?;
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let imported_project = reopened.get_project(local_id).await?;
    assert_eq!(
        imported_project
            .portable
            .required_capabilities
            .iter()
            .next()
            .map(|capability| (capability.id.as_str(), capability.version)),
        Some(("vendor.semantic", 2))
    );
    assert_eq!(imported_project.summary.revision, Revision::new(7));
    assert_eq!(
        imported_project.portable.semantic_extensions["vendor.semantic"],
        json!({"meaning": 1})
    );
    assert_eq!(
        imported_project.portable.extensions["vendor.display"],
        json!({"color": "blue"})
    );
    let snapshot = reopened.library_snapshot().await?;
    assert_eq!(
        snapshot.settings["appearance"].value,
        json!({"theme": "dark"})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&snapshot.sections.results["result-a"])?,
        json!({
            "resultId": "018f1e2d-3c4b-7a69-8def-200000000001",
            "scenarioId": local_id,
            "scenarioRevision": 7,
            "value": 1
        })
    );
    assert_eq!(
        snapshot.manifest_extensions["vendor.manifest"],
        json!({"preserved": true})
    );
    assert!(snapshot.nonsemantic_extensions.contains("vendor.manifest"));
    assert_eq!(
        snapshot.sections.shared_records["shared-a"],
        br#"{"value":2}"#
    );
    assert_eq!(
        snapshot.sections.preferences["preference-a"],
        br#"{"value":3}"#
    );
    assert_eq!(snapshot.sections.assets["asset-a"].bytes, b"asset bytes");
    assert_eq!(snapshot.sections.assets["asset-a"].media_type, "text/plain");
    assert!(!snapshot.sections.assets["asset-a"].redistribution_permitted);
    assert_eq!(
        snapshot.supplemental_identities,
        BTreeSet::from([
            SupplementalIdentity {
                section: SupplementalSectionKind::Results,
                key: "result-a".to_owned(),
            },
            SupplementalIdentity {
                section: SupplementalSectionKind::SharedRecords,
                key: "shared-a".to_owned(),
            },
            SupplementalIdentity {
                section: SupplementalSectionKind::Preferences,
                key: "preference-a".to_owned(),
            },
            SupplementalIdentity {
                section: SupplementalSectionKind::Assets,
                key: "asset-a".to_owned(),
            },
        ])
    );
    assert_eq!(snapshot.provenance.len(), 1);
    assert_eq!(snapshot.provenance[0].source_bundle_id, source_bundle_id);
    assert_eq!(
        snapshot.provenance[0].scenario_sources[0].source_revision,
        Revision::new(7)
    );
    Ok(())
}

#[tokio::test]
async fn supplemental_overwrite_requires_exact_replace_authorization_and_skip_is_absent()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let replace_identity = SupplementalIdentity {
        section: SupplementalSectionKind::SharedRecords,
        key: "replace-me".to_owned(),
    };
    let skip_identity = SupplementalIdentity {
        section: SupplementalSectionKind::SharedRecords,
        key: "keep-me".to_owned(),
    };
    let mut baseline = staged_import(Revision::INITIAL, Vec::new(), timestamp(CREATED)?);
    baseline.shared_records.insert(
        replace_identity.key.clone(),
        br#"{"value":"before"}"#.to_vec(),
    );
    baseline
        .shared_records
        .insert(skip_identity.key.clone(), br#"{"value":"keep"}"#.to_vec());
    store
        .apply_staged_library(
            StagedLibraryApply::Import(baseline),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;

    let mut unauthorized = staged_import(Revision::new(1), Vec::new(), timestamp(CREATED)?);
    unauthorized.shared_records.insert(
        replace_identity.key.clone(),
        br#"{"value":"unauthorized"}"#.to_vec(),
    );
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(unauthorized),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    let after_unauthorized = store.library_snapshot().await?;
    assert_eq!(after_unauthorized.revision, Revision::new(1));
    assert_eq!(
        after_unauthorized.sections.shared_records[&replace_identity.key],
        br#"{"value":"before"}"#
    );

    let mut absent_skip = staged_import(Revision::new(1), Vec::new(), timestamp(CREATED)?);
    absent_skip
        .supplemental_replacements
        .insert(skip_identity.clone());
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(absent_skip),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));

    let mut authorized = staged_import(Revision::new(1), Vec::new(), timestamp(CREATED)?);
    authorized.shared_records.insert(
        replace_identity.key.clone(),
        br#"{"value":"after"}"#.to_vec(),
    );
    authorized
        .supplemental_replacements
        .insert(replace_identity.clone());
    store
        .apply_staged_library(
            StagedLibraryApply::Import(authorized),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let snapshot = store.library_snapshot().await?;
    assert_eq!(
        snapshot.sections.shared_records[&replace_identity.key],
        br#"{"value":"after"}"#
    );
    assert_eq!(
        snapshot.sections.shared_records[&skip_identity.key],
        br#"{"value":"keep"}"#
    );
    Ok(())
}

#[tokio::test]
async fn add_backup_merges_settings_sections_and_archived_state() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let local_id = scenario_id(30)?;
    let archived_id = scenario_id(31)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_portable_import(&store, local_id).await?;
    apply_add_backup(&store, archived_id).await?;

    let archived = store.get_project(archived_id).await?;
    assert_eq!(archived.summary.revision, Revision::new(12));
    assert_eq!(archived.summary.archived_at, Some(timestamp(UPDATED)?));
    let snapshot = store.library_snapshot().await?;
    assert!(snapshot.sections.results.contains_key("result-a"));
    assert!(snapshot.sections.results.contains_key("result-b"));
    assert_eq!(snapshot.provenance.len(), 2);
    assert_eq!(
        snapshot.settings["backup-added"].value,
        json!({"enabled": true})
    );
    assert_eq!(
        snapshot.settings["appearance"].value,
        json!({"theme": "dark"})
    );
    Ok(())
}

#[tokio::test]
async fn replace_backup_replaces_settings_sections_and_preserves_archive_on_reopen()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let local_id = scenario_id(30)?;
    let archived_id = scenario_id(31)?;
    let restored_id = scenario_id(32)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_portable_import(&store, local_id).await?;
    apply_add_backup(&store, archived_id).await?;
    let mut replacement = staged_import(
        Revision::new(4),
        vec![(
            Revision::new(13),
            document(restored_id)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    replacement.mode = RestoreMode::ReplaceLibrary;
    replacement.scenarios[0].scenario.project = Some(PortableProjectMetadata {
        archived_at: Some(timestamp(LATER)?),
    });
    replacement
        .preferences
        .insert("replacement".to_owned(), br#"{"only":true}"#.to_vec());
    store
        .apply_staged_library(
            StagedLibraryApply::BackupRestore {
                restore: StagedBackupRestore {
                    import: replacement,
                    remove_scenario_ids: BTreeSet::from([local_id, archived_id]),
                    authorization: RestoreAuthorization {
                        destructive_action_confirmed: true,
                        prospective_failure_receipt_token: None,
                        collision_plan_sha256: Some("a".repeat(64)),
                        safety_backup: SafetyBackupEvidence::Verified {
                            bundle_sha256: "verified-replacement".to_owned(),
                        },
                    },
                },
                settings: BTreeMap::from([(
                    "appearance".to_owned(),
                    AppSetting {
                        value: json!({"theme": "restored"}),
                        updated_at: timestamp(LATER)?,
                    },
                )]),
            },
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let restored = reopened.get_project(restored_id).await?;
    assert_eq!(restored.summary.revision, Revision::new(13));
    assert_eq!(restored.summary.archived_at, Some(timestamp(LATER)?));
    let snapshot = reopened.library_snapshot().await?;
    assert!(snapshot.sections.results.is_empty());
    assert!(snapshot.sections.shared_records.is_empty());
    assert!(snapshot.sections.assets.is_empty());
    assert!(snapshot.manifest_extensions.is_empty());
    assert!(snapshot.nonsemantic_extensions.is_empty());
    assert_eq!(
        snapshot.sections.preferences["replacement"],
        br#"{"only":true}"#
    );
    assert_eq!(snapshot.provenance.len(), 1);
    assert_eq!(
        snapshot.settings["appearance"].value,
        json!({"theme": "restored"})
    );
    assert!(!snapshot.settings.contains_key("backup-added"));
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
// The same failed transaction must preserve scenarios, sections and provenance.
#[allow(clippy::too_many_lines)]
async fn supplemental_failpoint_rolls_back_scenarios_sections_and_provenance()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let existing_id = scenario_id(33)?;
    let imported_id = scenario_id(34)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(existing_id)?,
        })
        .await?;
    let mut baseline = staged_import(Revision::new(1), Vec::new(), timestamp(CREATED)?);
    baseline
        .shared_records
        .insert("stable".to_owned(), br#"{"value":"before"}"#.to_vec());
    store
        .apply_staged_library(
            StagedLibraryApply::Import(baseline),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    store
        .replace_settings(
            &["stable-setting"],
            store.settings_snapshot(&["stable-setting"]).await?,
            BTreeMap::from([(
                "stable-setting".to_owned(),
                AppSetting {
                    value: json!({"value": "before"}),
                    updated_at: timestamp(UPDATED)?,
                },
            )]),
            CancellationToken::new(),
        )
        .await?;
    store.set_failpoint(Failpoint::AfterSupplementalWrite)?;

    let mut staged = staged_import(
        Revision::new(3),
        vec![(
            Revision::new(4),
            document(imported_id)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    staged
        .shared_records
        .insert("stable".to_owned(), br#"{"value":"after"}"#.to_vec());
    staged.assets.insert(
        "rolled-back".to_owned(),
        PortableAsset {
            bytes: b"rolled back".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: true,
        },
    );
    staged.mode = RestoreMode::ReplaceLibrary;
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::BackupRestore {
                    restore: StagedBackupRestore {
                        import: staged,
                        remove_scenario_ids: BTreeSet::from([existing_id]),
                        authorization: RestoreAuthorization {
                            destructive_action_confirmed: true,
                            prospective_failure_receipt_token: None,
                            collision_plan_sha256: Some("a".repeat(64)),
                            safety_backup: SafetyBackupEvidence::Verified {
                                bundle_sha256: "verified-failpoint".to_owned(),
                            },
                        },
                    },
                    settings: BTreeMap::from([(
                        "rolled-back-setting".to_owned(),
                        AppSetting {
                            value: json!({"value": "after"}),
                            updated_at: timestamp(LATER)?,
                        },
                    )]),
                },
                timestamp(LATER)?,
                CancellationToken::new(),
            )
            .await,
        Err(StoreError::InjectedFailure)
    ));
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.revision, Revision::new(3));
    assert_eq!(snapshot.projects.len(), 1);
    assert_eq!(snapshot.projects[0].summary.id, existing_id);
    assert_eq!(
        snapshot.sections.shared_records["stable"],
        br#"{"value":"before"}"#
    );
    assert!(snapshot.sections.assets.is_empty());
    assert_eq!(snapshot.provenance.len(), 1);
    assert_eq!(
        snapshot.settings["stable-setting"].value,
        json!({"value": "before"})
    );
    assert!(!snapshot.settings.contains_key("rolled-back-setting"));
    Ok(())
}

#[tokio::test]
async fn lifecycle_mutations_require_the_current_scenario_revision() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(35)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), None, UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    assert!(matches!(
        store
            .archive_project(id, Revision::INITIAL, timestamp(UPDATED)?)
            .await,
        Err(StoreError::Conflict { expected, actual })
            if expected == Revision::INITIAL && actual == Revision::new(1)
    ));
    assert!(store.get_project(id).await?.summary.archived_at.is_none());
    store
        .archive_project(id, Revision::new(1), timestamp(UPDATED)?)
        .await?;
    assert!(matches!(
        store.unarchive_project(id, Revision::INITIAL).await,
        Err(StoreError::Conflict { .. })
    ));
    assert!(matches!(
        store.delete_project(id, Revision::INITIAL).await,
        Err(StoreError::Conflict { .. })
    ));
    assert!(store.get_project(id).await?.summary.archived_at.is_some());
    store.unarchive_project(id, Revision::new(1)).await?;
    store.delete_project(id, Revision::new(1)).await?;
    assert!(matches!(
        store.get_project(id).await,
        Err(StoreError::ScenarioNotFound(found)) if found == id
    ));
    Ok(())
}

fn oversized_document(id: ScenarioId) -> Result<ScenarioDocument, Box<dyn Error>> {
    let mut oversized = document(id)?;
    let max_document_bytes = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)?;
    oversized.extensions.insert(
        "oversized".to_owned(),
        Value::String("x".repeat(max_document_bytes)),
    );
    Ok(oversized)
}

#[tokio::test]
// One fixture checks the shared document limit across every persistence entry point.
#[allow(clippy::too_many_lines)]
async fn authoritative_document_limit_covers_creation_commands_import_restore_and_snapshot_policy()
-> Result<(), Box<dyn Error>> {
    let interval = NonZeroU32::new(1).ok_or("nonzero interval")?;
    assert!(matches!(
        SnapshotPolicy::new(interval, MAX_SCENARIO_DOCUMENT_BYTES - 1, 3),
        Err(StoreError::InvalidSnapshotPolicy)
    ));
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let oversized_id = scenario_id(36)?;
    let legal_id = scenario_id(37)?;
    let imported_id = scenario_id(38)?;
    let mut oversized = oversized_document(oversized_id)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    assert!(matches!(
        store
            .create_project(NewProject {
                document: oversized.clone(),
            })
            .await,
        Err(StoreError::ScenarioDocumentTooLarge)
    ));
    store
        .create_project(NewProject {
            document: document(legal_id)?,
        })
        .await?;
    let mut command_document = oversized.clone();
    command_document.scenario_id = legal_id;
    assert!(matches!(
        store
            .execute_command(
                legal_id,
                Revision::INITIAL,
                RedoBranchPolicy::Reject,
                CancellationToken::new(),
                move |_| Ok(CommandWrite {
                    document: command_document,
                    journal: journal(json!({"oversized": true}), None, UPDATED)?,
                    output: (),
                }),
            )
            .await,
        Err(StoreError::ScenarioDocumentTooLarge)
    ));
    assert_eq!(
        store.get_project(legal_id).await?.summary.revision,
        Revision::INITIAL
    );
    oversized.scenario_id = imported_id;
    let staged = staged_import(
        Revision::new(1),
        vec![(Revision::new(5), oversized, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(staged),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::ScenarioDocumentTooLarge)
    ));
    assert!(matches!(
        store.get_project(imported_id).await,
        Err(StoreError::ScenarioNotFound(found)) if found == imported_id
    ));
    let restored_id = scenario_id(40)?;
    let mut restore = staged_import(
        Revision::new(1),
        vec![(
            Revision::new(6),
            oversized_document(restored_id)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    restore.mode = RestoreMode::AddBackup;
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::BackupRestore {
                    restore: StagedBackupRestore {
                        import: restore,
                        remove_scenario_ids: BTreeSet::new(),
                        authorization: RestoreAuthorization {
                            destructive_action_confirmed: false,
                            prospective_failure_receipt_token: None,
                            collision_plan_sha256: None,
                            safety_backup: SafetyBackupEvidence::NotRequired,
                        },
                    },
                    settings: BTreeMap::new(),
                },
                timestamp(LATER)?,
                CancellationToken::new(),
            )
            .await,
        Err(StoreError::ScenarioDocumentTooLarge)
    ));
    assert!(matches!(
        store.get_project(restored_id).await,
        Err(StoreError::ScenarioNotFound(found)) if found == restored_id
    ));
    Ok(())
}
#[tokio::test]
async fn replace_revision_advances_past_a_newer_local_revision() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(39)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), None, UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    let staged = staged_import(
        Revision::new(2),
        vec![(Revision::new(2), document(id)?, StagedDisposition::Replace)],
        timestamp(CREATED)?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(staged),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let project = store.get_project(id).await?;
    assert_eq!(project.summary.revision, Revision::new(2));
    let provenance = &store.library_snapshot().await?.provenance[0];
    assert_eq!(
        provenance.scenario_sources[0].source_revision,
        Revision::new(2)
    );
    Ok(())
}

fn aba_replace_restore(
    id: ScenarioId,
    removed_id: ScenarioId,
    source_document: ScenarioDocument,
) -> Result<StagedLibraryApply, Box<dyn Error>> {
    let mut restore = staged_import(
        Revision::new(3),
        vec![(
            Revision::new(7),
            set_marker(document(id)?, Some(7), LATER)?,
            StagedDisposition::Replace,
        )],
        timestamp(CREATED)?,
    );
    restore.mode = RestoreMode::ReplaceLibrary;
    restore.scenario_revisions.push(ScenarioSnapshotV1::current(
        Revision::new(6),
        source_document,
        BTreeSet::new(),
    ));
    restore.results.insert(
        "aba-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 6,
            "result": {"score": 6}
        }))?,
    );
    Ok(StagedLibraryApply::BackupRestore {
        restore: StagedBackupRestore {
            import: restore,
            remove_scenario_ids: BTreeSet::from([id, removed_id]),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: Some("d".repeat(64)),
                safety_backup: SafetyBackupEvidence::Verified {
                    bundle_sha256: "verified-aba-backup".to_owned(),
                },
            },
        },
        settings: BTreeMap::new(),
    })
}

#[tokio::test]
// The ordered create/replace/delete/restart flow is the ABA contract under test.
#[allow(clippy::too_many_lines)]
async fn scenario_revision_high_water_prevents_aba_and_survives_restore_restart()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(106)?;
    let removed_id = scenario_id(107)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let initial = staged_import(
        Revision::INITIAL,
        vec![
            (Revision::new(5), document(id)?, StagedDisposition::Create),
            (
                Revision::new(4),
                document(removed_id)?,
                StagedDisposition::Create,
            ),
        ],
        timestamp(CREATED)?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(initial),
            timestamp(UPDATED)?,
            CancellationToken::new(),
        )
        .await?;
    store.delete_project(id, Revision::new(5)).await?;
    assert_eq!(
        store.library_snapshot().await?.scenario_revision_high_water[&id],
        Revision::new(5)
    );

    let stale = staged_import(
        Revision::new(2),
        vec![(Revision::new(2), document(id)?, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(stale),
                timestamp(UPDATED)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    assert!(matches!(
        store.get_project(id).await,
        Err(StoreError::ScenarioNotFound(found)) if found == id
    ));

    let mut unrepresented_source = staged_import(
        Revision::new(2),
        vec![(Revision::new(6), document(id)?, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    unrepresented_source.scenarios[0].source_revision = Revision::new(2);
    unrepresented_source.results.insert(
        "missing-source-history-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 2,
            "result": {"score": 2}
        }))?,
    );
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(unrepresented_source),
                timestamp(UPDATED)?,
                CancellationToken::new(),
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));

    let mut target_six = staged_import(
        Revision::new(2),
        vec![(Revision::new(6), document(id)?, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    target_six.scenarios[0].source_revision = Revision::new(2);
    target_six
        .scenario_revisions
        .push(ScenarioSnapshotV1::current(
            Revision::new(2),
            document(id)?,
            BTreeSet::new(),
        ));
    target_six.results.insert(
        "source-revision-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 2,
            "result": {"score": 2}
        }))?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(target_six),
            timestamp(UPDATED)?,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        store.get_project(id).await?.summary.revision,
        Revision::new(6)
    );
    let snapshot = store.library_snapshot().await?;
    assert!(snapshot.provenance.iter().any(|provenance| {
        provenance
            .scenario_sources
            .iter()
            .any(|source| source.scenario_id == id && source.source_revision == Revision::new(2))
    }));
    assert!(snapshot.scenario_revisions.iter().any(|historical| {
        historical.scenario.document.scenario_id == id
            && historical.scenario.revision == Revision::new(2)
    }));
    drop(store);

    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let snapshot = store.library_snapshot().await?;
    assert!(snapshot.provenance.iter().any(|provenance| {
        provenance
            .scenario_sources
            .iter()
            .any(|source| source.scenario_id == id && source.source_revision == Revision::new(2))
    }));
    let source_document = store.get_project(id).await?.document;
    store
        .apply_staged_library(
            aba_replace_restore(id, removed_id, source_document.clone())?,
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.projects[0].summary.revision, Revision::new(7));
    assert_eq!(snapshot.scenario_revision_high_water[&id], Revision::new(7));
    assert_eq!(
        snapshot.scenario_revision_high_water[&removed_id],
        Revision::new(4)
    );
    assert_eq!(
        snapshot.scenario_revisions[0].scenario.document,
        source_document
    );
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let snapshot = reopened.library_snapshot().await?;
    assert_eq!(snapshot.scenario_revision_high_water[&id], Revision::new(7));
    assert_eq!(
        snapshot.scenario_revision_high_water[&removed_id],
        Revision::new(4)
    );
    assert!(matches!(
        reopened
            .execute_command(id, Revision::new(2), RedoBranchPolicy::Reject, CancellationToken::new(), |_| {
                Err::<CommandWrite<()>, StoreError>(StoreError::CommandApplication {
                    code: "must-not-run".to_owned(),
                    message: "stale callback ran".to_owned(),
                })
            })
            .await,
        Err(StoreError::Conflict { actual, .. }) if actual == Revision::new(7)
    ));
    Ok(())
}

#[tokio::test]
async fn no_result_source_revision_bumps_create_tombstone_and_replace_without_history()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(113)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store.delete_project(id, Revision::INITIAL).await?;

    let mut recreate = staged_import(
        Revision::new(2),
        vec![(Revision::new(2), document(id)?, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    recreate.scenarios[0].source_revision = Revision::INITIAL;
    store
        .apply_staged_library(
            StagedLibraryApply::Import(recreate),
            timestamp(UPDATED)?,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        store.get_project(id).await?.summary.revision,
        Revision::new(2)
    );
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .is_empty()
    );

    let mut replace = staged_import(
        Revision::new(3),
        vec![(Revision::new(3), document(id)?, StagedDisposition::Replace)],
        timestamp(CREATED)?,
    );
    replace.scenarios[0].source_revision = Revision::new(1);
    store
        .apply_staged_library(
            StagedLibraryApply::Import(replace),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.projects[0].summary.revision, Revision::new(3));
    assert!(snapshot.scenario_revisions.is_empty());
    assert!(snapshot.provenance.iter().any(|provenance| {
        provenance
            .scenario_sources
            .iter()
            .any(|source| source.scenario_id == id && source.source_revision == Revision::new(1))
    }));
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let snapshot = reopened.library_snapshot().await?;
    assert_eq!(snapshot.projects[0].summary.revision, Revision::new(3));
    assert!(snapshot.scenario_revisions.is_empty());
    Ok(())
}

#[tokio::test]
async fn startup_backfills_revision_high_water_from_durable_history() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(108)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    for (expected, marker) in [(Revision::INITIAL, 1), (Revision::new(1), 2)] {
        store
            .execute_command(
                id,
                expected,
                RedoBranchPolicy::Reject,
                CancellationToken::new(),
                move |current| {
                    Ok(CommandWrite {
                        document: set_marker(current.clone(), Some(marker), LATER)?,
                        journal: journal(json!({"advance": marker}), None, LATER)?,
                        output: (),
                    })
                },
            )
            .await?;
    }
    drop(store);
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE scenario_revision_high_water SET highest_revision = 0 WHERE scenario_id = ?1",
        [id.to_string()],
    )?;
    drop(connection);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened
            .library_snapshot()
            .await?
            .scenario_revision_high_water[&id],
        Revision::new(2)
    );
    Ok(())
}

#[tokio::test]
async fn duplicate_settings_and_online_backup_are_persistent() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let backup_path = directory.path().join("safety.sqlite3");
    let source_id = scenario_id(10)?;
    let copy_id = scenario_id(11)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(source_id)?,
        })
        .await?;
    let copy = store
        .duplicate_project(
            source_id,
            Revision::INITIAL,
            copy_id,
            BTreeMap::from([(source_id.as_uuid(), copy_id.as_uuid())]),
            "Clinic plan copy".to_owned(),
            timestamp(UPDATED)?,
        )
        .await?;
    assert_eq!(copy.summary.id, copy_id);
    assert_eq!(copy.summary.title, "Clinic plan copy");
    assert_eq!(copy.summary.revision, Revision::INITIAL);
    assert_eq!(copy.document.scenario_id, copy_id);
    assert_eq!(copy.document.metadata.title, "Clinic plan copy");

    store
        .replace_settings(
            &["appearance"],
            store.settings_snapshot(&["appearance"]).await?,
            BTreeMap::from([(
                "appearance".to_owned(),
                AppSetting {
                    value: json!({"theme": "system"}),
                    updated_at: timestamp(UPDATED)?,
                },
            )]),
            CancellationToken::new(),
        )
        .await?;
    let setting = store
        .get_setting::<Value>("appearance".to_owned())
        .await?
        .ok_or("setting was not persisted")?;
    assert_eq!(setting.value, json!({"theme": "system"}));
    store.safety_backup(&backup_path).await?;
    assert!(matches!(
        store.safety_backup(&backup_path).await,
        Err(StoreError::BackupDestinationExists)
    ));

    let backup = Connection::open(&backup_path)?;
    let copied_rows: u32 =
        backup.query_row("SELECT COUNT(*) FROM scenarios", [], |row| row.get(0))?;
    let setting_rows: u32 =
        backup.query_row("SELECT COUNT(*) FROM app_settings", [], |row| row.get(0))?;
    assert_eq!(copied_rows, 2);
    assert_eq!(setting_rows, 1);
    Ok(())
}

#[tokio::test]
async fn duplicate_refuses_missing_extra_and_occupied_identity_mappings_without_mutation()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let source_id = scenario_id(101)?;
    let occupied_id = scenario_id(102)?;
    let copy_id = scenario_id(103)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    for id in [source_id, occupied_id] {
        store
            .create_project(NewProject {
                document: document(id)?,
            })
            .await?;
    }
    let revision_before = store.library_snapshot().await?.revision;
    for (new_id, mapping) in [
        (copy_id, BTreeMap::new()),
        (
            copy_id,
            BTreeMap::from([
                (source_id.as_uuid(), copy_id.as_uuid()),
                (scenario_id(104)?.as_uuid(), scenario_id(105)?.as_uuid()),
            ]),
        ),
        (
            occupied_id,
            BTreeMap::from([(source_id.as_uuid(), occupied_id.as_uuid())]),
        ),
    ] {
        assert!(matches!(
            store
                .duplicate_project(
                    source_id,
                    Revision::INITIAL,
                    new_id,
                    mapping,
                    "Rejected copy".to_owned(),
                    timestamp(LATER)?,
                )
                .await,
            Err(StoreError::InvalidDuplicateMapping(_))
        ));
    }
    assert_eq!(store.library_snapshot().await?.revision, revision_before);
    assert!(matches!(
        store.get_project(copy_id).await,
        Err(StoreError::ScenarioNotFound(id)) if id == copy_id
    ));
    store.delete_project(occupied_id, Revision::INITIAL).await?;
    assert!(matches!(
        store
            .create_project(NewProject {
                document: document(occupied_id)?,
            })
            .await,
        Err(StoreError::ScenarioAlreadyExists(id)) if id == occupied_id
    ));
    assert!(matches!(
        store
            .duplicate_project(
                source_id,
                Revision::INITIAL,
                occupied_id,
                BTreeMap::from([(source_id.as_uuid(), occupied_id.as_uuid())]),
                "Rejected tombstone copy".to_owned(),
                timestamp(LATER)?,
            )
            .await,
        Err(StoreError::InvalidDuplicateMapping(_))
    ));
    Ok(())
}

fn assert_remapped_duplicate_graph(
    copy: &StoredProject,
    person: EntityId,
    rule: RuleId,
    semantic_id: Uuid,
    mapped_document_nonsemantic: Uuid,
    mapped_portable_nonsemantic: Uuid,
    unknown_nonsemantic: Uuid,
) -> Result<(), Box<dyn Error>> {
    let copied_domain = serde_json::to_value(&copy.document.domain)?;
    let copied_person = copied_domain["entities"]
        .as_object()
        .and_then(|values| values.keys().next())
        .ok_or("copied entity missing")?;
    let copied_rule = copied_domain["rules"]
        .as_object()
        .and_then(|values| values.keys().next())
        .ok_or("copied rule missing")?;
    assert_ne!(copied_person, &person.to_string());
    assert_ne!(copied_rule, &rule.to_string());
    assert_eq!(
        copied_domain["entities"][copied_person]["id"].as_str(),
        Some(copied_person.as_str())
    );
    assert_eq!(
        copied_domain["entities"][copied_person]["ruleId"].as_str(),
        Some(copied_rule.as_str())
    );
    assert_eq!(
        copied_domain["entities"][copied_person]["externalId"],
        person.to_string()
    );
    assert_eq!(
        copied_domain["entities"][copied_person]["note"],
        person.to_string()
    );
    assert_eq!(
        copied_domain["rules"][copied_rule]["participantId"].as_str(),
        Some(copied_person.as_str())
    );
    assert_eq!(
        copied_domain["rules"][copied_rule]["participantIds"][0].as_str(),
        Some(copied_person.as_str())
    );
    for (extension, mapped_nonsemantic) in [
        (
            &copy.document.extensions["vendor.document"],
            mapped_document_nonsemantic,
        ),
        (
            &copy.portable.extensions["vendor.display"],
            mapped_portable_nonsemantic,
        ),
    ] {
        assert_eq!(extension["scenarioId"], copy.summary.id.to_string());
        assert_eq!(
            extension["selectedEntityId"].as_str(),
            Some(copied_person.as_str())
        );
        assert_eq!(extension["externalId"], person.to_string());
        assert_eq!(extension["note"], person.to_string());
        assert_eq!(
            extension["definitions"][mapped_nonsemantic.to_string()]["id"],
            mapped_nonsemantic.to_string()
        );
        assert_eq!(
            extension["definitions"][unknown_nonsemantic.to_string()]["note"],
            "unknown definition key"
        );
    }
    let semantic = &copy.portable.semantic_extensions["vendor.semantic"];
    let copied_semantic_id = semantic["definitions"]
        .as_object()
        .and_then(|values| values.keys().next())
        .ok_or("copied semantic definition missing")?;
    assert_ne!(copied_semantic_id, &semantic_id.to_string());
    assert_eq!(
        semantic["definitions"][copied_semantic_id]["participantId"].as_str(),
        Some(copied_person.as_str())
    );
    Ok(())
}

#[tokio::test]
// One end-to-end restart fixture keeps the complete identity graph and metadata coupled.
#[allow(clippy::too_many_lines)]
async fn duplicate_remaps_owned_graph_and_preserves_portable_metadata_after_restart()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let source_id = scenario_id(90)?;
    let copy_id = scenario_id(91)?;
    let person = EntityId::from_uuid(Uuid::now_v7());
    let rule = RuleId::from_uuid(Uuid::now_v7());
    let semantic_id = Uuid::now_v7();
    let document_nonsemantic_owned = Uuid::now_v7();
    let mapped_document_nonsemantic = scenario_id(109)?.as_uuid();
    let portable_nonsemantic_owned = Uuid::now_v7();
    let mapped_portable_nonsemantic = scenario_id(112)?.as_uuid();
    let unknown_nonsemantic = Uuid::now_v7();
    let mut source_document = document(source_id)?;
    source_document.domain.entities.insert(
        person,
        json!({"id": person, "ruleId": rule, "externalId": person.to_string(), "note": person.to_string()}),
    );
    source_document.domain.rules.insert(
        rule,
        json!({"id": rule, "participantId": person, "participantIds": [person]}),
    );
    let document_nonsemantic = json!({
        "scenarioId": source_id,
        "selectedEntityId": person,
        "externalId": person,
        "note": person.to_string(),
        "definitions": {
            (document_nonsemantic_owned.to_string()): {
                "id": document_nonsemantic_owned,
                "selectedEntityId": person
            },
            (unknown_nonsemantic.to_string()): {
                "note": "unknown definition key"
            }
        }
    });
    source_document
        .extensions
        .insert("vendor.document".to_owned(), document_nonsemantic);
    let portable_nonsemantic = json!({
        "scenarioId": source_id,
        "selectedEntityId": person,
        "externalId": person,
        "note": person.to_string(),
        "definitions": {
            (portable_nonsemantic_owned.to_string()): {
                "id": portable_nonsemantic_owned,
                "selectedEntityId": person
            },
            (unknown_nonsemantic.to_string()): {
                "note": "unknown definition key"
            }
        }
    });
    let mut imported = staged_import(
        Revision::INITIAL,
        vec![(Revision::new(4), source_document, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    imported.scenarios[0]
        .scenario
        .required_capabilities
        .insert(SemanticCapability {
            id: "vendor.semantic".to_owned(),
            version: 1,
        });
    imported.scenarios[0].scenario.semantic_extensions.insert(
        "vendor.semantic".to_owned(),
        json!({
            "definitions": {
                (semantic_id.to_string()): {
                    "id": semantic_id,
                    "participantId": person
                }
            }
        }),
    );
    imported.scenarios[0]
        .scenario
        .extensions
        .insert("vendor.display".to_owned(), portable_nonsemantic);
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .apply_staged_library(
            StagedLibraryApply::Import(imported),
            timestamp(UPDATED)?,
            CancellationToken::new(),
        )
        .await?;

    let id_remap = BTreeMap::from([
        (source_id.as_uuid(), copy_id.as_uuid()),
        (person.as_uuid(), scenario_id(98)?.as_uuid()),
        (rule.as_uuid(), scenario_id(99)?.as_uuid()),
        (semantic_id, scenario_id(100)?.as_uuid()),
        (document_nonsemantic_owned, mapped_document_nonsemantic),
        (portable_nonsemantic_owned, mapped_portable_nonsemantic),
    ]);
    let copy = store
        .duplicate_project(
            source_id,
            Revision::new(4),
            copy_id,
            id_remap,
            "Graph copy".to_owned(),
            timestamp(LATER)?,
        )
        .await?;
    assert_remapped_duplicate_graph(
        &copy,
        person,
        rule,
        semantic_id,
        mapped_document_nonsemantic,
        mapped_portable_nonsemantic,
        unknown_nonsemantic,
    )?;
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let persisted = reopened.get_project(copy_id).await?;
    assert_eq!(persisted.summary.revision, copy.summary.revision);
    assert_eq!(persisted.document, copy.document);
    assert_eq!(persisted.portable, copy.portable);
    let mut exportable = ScenarioSnapshotV1::current(
        persisted.summary.revision,
        persisted.document,
        persisted.portable.required_capabilities,
    );
    exportable.semantic_extensions = persisted.portable.semantic_extensions;
    exportable.extensions = persisted.portable.extensions;
    validate_scenario_snapshot(&exportable)?;
    assert_eq!(
        serde_json::to_value(&exportable)?["extensions"]["vendor.display"],
        copy.portable.extensions["vendor.display"]
    );
    Ok(())
}

#[tokio::test]
async fn stale_duplicate_is_rejected_without_mutation_and_survives_reopen()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let source_id = scenario_id(12)?;
    let copy_id = scenario_id(13)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(source_id)?,
        })
        .await?;
    store
        .execute_command(
            source_id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    let revision_before = store.library_snapshot().await?.revision;

    assert!(matches!(
        store
            .duplicate_project(
                source_id,
                Revision::INITIAL,
                copy_id,
                BTreeMap::from([(source_id.as_uuid(), copy_id.as_uuid())]),
                "Stale copy".to_owned(),
                timestamp(LATER)?,
            )
            .await,
        Err(StoreError::Conflict {
            expected: Revision::INITIAL,
            actual
        }) if actual == Revision::new(1)
    ));
    assert_eq!(store.library_snapshot().await?.revision, revision_before);
    assert!(matches!(
        store.get_project(copy_id).await,
        Err(StoreError::ScenarioNotFound(id)) if id == copy_id
    ));
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(reopened.library_snapshot().await?.revision, revision_before);
    assert_eq!(
        reopened.get_project(source_id).await?.summary.revision,
        Revision::new(1)
    );
    assert!(matches!(
        reopened.get_project(copy_id).await,
        Err(StoreError::ScenarioNotFound(id)) if id == copy_id
    ));
    Ok(())
}

#[tokio::test]
async fn stale_revision_is_rejected_before_callback_or_mutation() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(2)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;

    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;

    let callback_ran = Arc::new(AtomicBool::new(false));
    let callback_flag = Arc::clone(&callback_ran);
    let result = store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            move |current| {
                callback_flag.store(true, Ordering::SeqCst);
                Ok(CommandWrite {
                    document: current.clone(),
                    journal: journal(json!({}), None, LATER)?,
                    output: (),
                })
            },
        )
        .await;
    assert!(matches!(
        result,
        Err(StoreError::Conflict { expected, actual })
            if expected == Revision::INITIAL && actual == Revision::new(1)
    ));
    assert!(!callback_ran.load(Ordering::SeqCst));
    assert_eq!(
        store.get_project(id).await?.summary.revision,
        Revision::new(1)
    );
    assert_eq!(store.history(id).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn command_cancelled_while_queued_never_calls_apply() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(120)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let initial = document(id)?;
    store
        .create_project(NewProject {
            document: initial.clone(),
        })
        .await?;
    let before = store.library_snapshot().await?;
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let callback_entered = Arc::clone(&entered);
    let callback_release = Arc::clone(&release);
    let blocking_store = store.clone();
    let blocker_cancellation = CancellationToken::new();
    let actor_cancellation = blocker_cancellation.clone();
    let blocker = tokio::spawn(async move {
        blocking_store
            .execute_command(
                id,
                Revision::INITIAL,
                RedoBranchPolicy::Reject,
                actor_cancellation,
                move |current| {
                    callback_entered.wait();
                    callback_release.wait();
                    Ok(CommandWrite {
                        document: set_marker(current.clone(), Some(1), UPDATED)?,
                        journal: journal(json!({"marker": 1}), None, UPDATED)?,
                        output: (),
                    })
                },
            )
            .await
    });
    tokio::task::spawn_blocking(move || entered.wait()).await?;

    let cancellation = CancellationToken::new();
    let callback_ran = Arc::new(AtomicBool::new(false));
    let callback_flag = Arc::clone(&callback_ran);
    let mut queued = Box::pin(store.execute_command(
        id,
        Revision::INITIAL,
        RedoBranchPolicy::Reject,
        cancellation.clone(),
        move |current| {
            callback_flag.store(true, Ordering::SeqCst);
            Ok(CommandWrite {
                document: set_marker(current.clone(), Some(2), LATER)?,
                journal: journal(json!({"marker": 2}), None, LATER)?,
                output: (),
            })
        },
    ));
    // Poll once to enqueue behind the blocked actor, without a scheduler/timing assumption.
    std::future::poll_fn(|context| {
        assert!(queued.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    cancellation.cancel();
    blocker_cancellation.cancel();
    release.wait();

    assert!(matches!(
        blocker.await?,
        Err(StoreError::OperationCancelled)
    ));
    assert!(matches!(queued.await, Err(StoreError::OperationCancelled)));
    assert!(!callback_ran.load(Ordering::SeqCst));
    assert_eq!(store.get_project(id).await?.document, initial);
    assert!(store.history(id).await?.is_empty());
    assert_eq!(store.library_snapshot().await?.revision, before.revision);
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened.get_project(id).await?.summary.revision,
        Revision::INITIAL
    );
    assert_eq!(reopened.get_project(id).await?.document, initial);
    assert!(reopened.history(id).await?.is_empty());
    Ok(())
}

#[cfg(debug_assertions)]
#[derive(Debug, Eq, PartialEq)]
struct CommandSnapshotRow {
    id: String,
    revision: i64,
    compressed_document: Vec<u8>,
    created_at: String,
    reason: String,
}

#[cfg(debug_assertions)]
fn command_snapshots(path: &std::path::Path) -> Result<Vec<CommandSnapshotRow>, rusqlite::Error> {
    let connection = Connection::open(path)?;
    connection.prepare(
        "SELECT id, revision, document_json_zstd, created_at, reason FROM scenario_snapshots ORDER BY id",
    )?.query_map([], |row| {
        Ok(CommandSnapshotRow {
            id: row.get(0)?,
            revision: row.get(1)?,
            compressed_document: row.get(2)?,
            created_at: row.get(3)?,
            reason: row.get(4)?,
        })
    })?.collect()
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_lines)]
async fn cancelled_branch_transaction_rolls_back(
    abandon_receiver: bool,
) -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(121)?;
    let policy = SnapshotPolicy::new(
        NonZeroU32::new(1).ok_or("nonzero interval")?,
        MAX_SCENARIO_DOCUMENT_BYTES,
        3,
    )?;
    let (store, _) =
        SqliteScenarioStore::open_with_options(&path, OpenOptions::new(policy)).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    store
        .undo(id, Revision::new(1), timestamp(LATER)?, |history| {
            let target = history.target_document_updated_at.to_string();
            Ok((set_marker(history.document, None, &target)?, ()))
        })
        .await?;
    drop(store);

    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::BeforeFinalCancellationCheck);
    let (store, _) = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::new(policy).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let before = store.library_snapshot().await?;
    let history_before = store.history(id).await?;
    let snapshots_before = command_snapshots(&path)?;
    let identity = scenario_id(122)?.as_uuid();
    let entity: EntityId = identity.to_string().parse()?;
    let cancellation = CancellationToken::new();
    let actor_cancellation = cancellation.clone();
    let actor_store = store.clone();
    let command = tokio::spawn(async move {
        actor_store
            .execute_command(
                id,
                Revision::new(2),
                RedoBranchPolicy::Truncate,
                actor_cancellation,
                move |current| {
                    let mut updated = set_marker(current.clone(), Some(2), LATER)?;
                    updated
                        .domain
                        .entities
                        .insert(entity, json!({"id": identity}));
                    Ok(CommandWrite {
                        document: updated,
                        journal: journal(
                            json!({"marker": 2}),
                            Some(json!({"marker": null})),
                            LATER,
                        )?,
                        output: (),
                    })
                },
            )
            .await
    });
    let wait_hook = hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_until_reached()).await?;
    cancellation.cancel();
    if abandon_receiver {
        command.abort();
        assert!(command.await.is_err_and(|error| error.is_cancelled()));
    } else {
        hook.release();
        assert!(matches!(
            command.await?,
            Err(StoreError::OperationCancelled)
        ));
    }
    if abandon_receiver {
        hook.release();
    }
    // This actor round trip also drains work whose response receiver was abandoned.
    let after = store.library_snapshot().await?;
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        after.scenario_identity_owners,
        before.scenario_identity_owners
    );
    assert_eq!(
        after.scenario_revision_high_water,
        before.scenario_revision_high_water
    );
    assert_eq!(after.scenario_revisions, before.scenario_revisions);
    assert_eq!(after.projects[0].document, before.projects[0].document);
    assert_eq!(after.projects[0].summary, before.projects[0].summary);
    assert_eq!(store.history(id).await?, history_before);
    assert_eq!(command_snapshots(&path)?, snapshots_before);
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let durable = reopened.library_snapshot().await?;
    assert_eq!(durable.revision, before.revision);
    assert_eq!(
        durable.scenario_identity_owners,
        before.scenario_identity_owners
    );
    assert_eq!(
        durable.scenario_revision_high_water,
        before.scenario_revision_high_water
    );
    assert_eq!(durable.scenario_revisions, before.scenario_revisions);
    assert_eq!(durable.projects[0].document, before.projects[0].document);
    assert_eq!(durable.projects[0].summary, before.projects[0].summary);
    assert_eq!(reopened.history(id).await?, history_before);
    assert_eq!(command_snapshots(&path)?, snapshots_before);
    // The original branch must still be redoable, not merely present in the journal.
    let redone = reopened
        .redo(id, Revision::new(2), timestamp(LATER)?, |history| {
            let target = history.target_document_updated_at.to_string();
            Ok((set_marker(history.document, Some(1), &target)?, ()))
        })
        .await?;
    assert_eq!(redone.new_revision, Revision::new(3));
    assert_eq!(
        reopened.get_project(id).await?.document,
        set_marker(document(id)?, Some(1), UPDATED)?
    );
    let history = reopened.history(id).await?;
    assert_eq!(
        history[0].branch_generation,
        history_before[0].branch_generation
    );
    assert!(history[0].applied);
    reopened
        .execute_command(
            id,
            Revision::new(3),
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(3), LATER)?,
                    journal: journal(json!({"marker": 3}), Some(json!({"marker": 1})), LATER)?,
                    output: (),
                })
            },
        )
        .await?;
    let continued_history = reopened.history(id).await?;
    assert_eq!(continued_history.len(), 2);
    assert_eq!(continued_history[1].history_sequence, 2);
    assert_eq!(
        continued_history[1].branch_generation,
        history_before[0].branch_generation
    );
    // Rollback must not leave the candidate identity reserved for a nonexistent command.
    let mut another = document(scenario_id(123)?)?;
    another
        .domain
        .entities
        .insert(entity, json!({"id": identity}));
    reopened
        .create_project(NewProject { document: another })
        .await?;
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn command_cancelled_before_final_check_restores_branch_and_survives_reopen()
-> Result<(), Box<dyn Error>> {
    cancelled_branch_transaction_rolls_back(false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn command_cancelled_with_abandoned_receiver_rolls_back() -> Result<(), Box<dyn Error>> {
    cancelled_branch_transaction_rolls_back(true).await
}

#[cfg(debug_assertions)]
async fn command_commit_wins_late_cancellation(
    abandon_receiver: bool,
) -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(124)?;
    let expected_document = set_marker(document(id)?, Some(1), UPDATED)?;
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::AfterFinalCancellationCheck);
    let policy = SnapshotPolicy::new(
        NonZeroU32::new(1).ok_or("nonzero interval")?,
        MAX_SCENARIO_DOCUMENT_BYTES,
        3,
    )?;
    let (store, _) = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::new(policy).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let before = store.library_snapshot().await?;
    let cancellation = CancellationToken::new();
    let actor_cancellation = cancellation.clone();
    let actor_store = store.clone();
    let command = tokio::spawn(async move {
        actor_store
            .execute_command(
                id,
                Revision::INITIAL,
                RedoBranchPolicy::Reject,
                actor_cancellation,
                |current| {
                    Ok(CommandWrite {
                        document: set_marker(current.clone(), Some(1), UPDATED)?,
                        journal: journal(
                            json!({"marker": 1}),
                            Some(json!({"marker": null})),
                            UPDATED,
                        )?,
                        output: 42,
                    })
                },
            )
            .await
    });
    let wait_hook = hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_until_reached()).await?;
    cancellation.cancel();
    if abandon_receiver {
        command.abort();
        assert!(command.await.is_err_and(|error| error.is_cancelled()));
    } else {
        hook.release();
        let committed = command.await??;
        assert_eq!(committed.new_revision, Revision::new(1));
        assert_eq!(committed.output, 42);
    }
    if abandon_receiver {
        hook.release();
    }
    let after = store.library_snapshot().await?;
    assert_eq!(after.revision, before.revision.checked_next()?);
    assert_eq!(
        after.scenario_revision_high_water.get(&id),
        Some(&Revision::new(1))
    );
    let persisted = store.get_project(id).await?;
    assert_eq!(persisted.summary.revision, Revision::new(1));
    assert_eq!(persisted.document, expected_document);
    let history = store.history(id).await?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].command, json!({"marker": 1}));
    assert!(history[0].applied);
    let snapshots = command_snapshots(&path)?;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].revision, 1);
    let snapshot_document: ScenarioDocument = serde_json::from_slice(&zstd::stream::decode_all(
        snapshots[0].compressed_document.as_slice(),
    )?)?;
    assert_eq!(snapshot_document, expected_document);
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened.get_project(id).await?.summary.revision,
        Revision::new(1)
    );
    assert_eq!(reopened.get_project(id).await?.document, snapshot_document);
    assert_eq!(reopened.history(id).await?, history);
    assert_eq!(command_snapshots(&path)?, snapshots);
    reopened
        .undo(id, Revision::new(1), timestamp(LATER)?, |history| {
            let target = history.target_document_updated_at.to_string();
            Ok((set_marker(history.document, None, &target)?, ()))
        })
        .await?;
    assert_eq!(reopened.get_project(id).await?.document, document(id)?);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn command_cancelled_after_final_check_returns_committed_success()
-> Result<(), Box<dyn Error>> {
    command_commit_wins_late_cancellation(false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn command_cancelled_after_final_check_commits_with_abandoned_receiver()
-> Result<(), Box<dyn Error>> {
    command_commit_wins_late_cancellation(true).await
}

#[tokio::test]
async fn actor_self_drop_detaches_without_deadlock_and_allows_reopen() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(94)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let callback_store = store.clone();
    let task_store = store.clone();
    drop(store);
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let callback_entered = Arc::clone(&entered);
    let callback_release = Arc::clone(&release);
    let (finished_sender, finished_receiver) = std::sync::mpsc::sync_channel(1);
    let task = tokio::spawn(async move {
        task_store
            .execute_command(
                id,
                Revision::INITIAL,
                RedoBranchPolicy::Reject,
                CancellationToken::new(),
                move |current| {
                    callback_entered.wait();
                    callback_release.wait();
                    let write = CommandWrite {
                        document: set_marker(current.clone(), Some(1), UPDATED)?,
                        journal: journal(
                            json!({"marker": 1}),
                            Some(json!({"marker": null})),
                            UPDATED,
                        )?,
                        output: (),
                    };
                    drop(callback_store);
                    finished_sender
                        .send(())
                        .map_err(|_| StoreError::ActorUnavailable)?;
                    Ok(write)
                },
            )
            .await
    });
    tokio::task::spawn_blocking(move || {
        entered.wait();
    })
    .await?;
    task.abort();
    let _aborted = task.await;
    release.wait();
    tokio::task::spawn_blocking(move || {
        finished_receiver.recv_timeout(std::time::Duration::from_secs(2))
    })
    .await??;

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened.get_project(id).await?.summary.revision,
        Revision::new(1)
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn document_write_failpoint_rolls_back_document_journal_revision_and_snapshot()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(3)?;
    let interval = NonZeroU32::new(1).ok_or("nonzero interval")?;
    let options = OpenOptions::new(SnapshotPolicy::new(
        interval,
        MAX_SCENARIO_DOCUMENT_BYTES,
        3,
    )?);
    let (store, _) = SqliteScenarioStore::open_with_options(&path, options).await?;
    let initial = document(id)?;
    store
        .create_project(NewProject {
            document: initial.clone(),
        })
        .await?;
    store.set_failpoint(Failpoint::AfterDocumentWrite)?;

    let result = store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await;
    assert!(matches!(result, Err(StoreError::InjectedFailure)));
    let persisted = store.get_project(id).await?;
    assert_eq!(persisted.summary.revision, Revision::INITIAL);
    assert_eq!(persisted.document, initial);
    assert!(store.history(id).await?.is_empty());

    let connection = Connection::open(&path)?;
    let snapshots: u32 = connection.query_row(
        "SELECT COUNT(*) FROM scenario_snapshots WHERE scenario_id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(snapshots, 0);
    Ok(())
}

#[tokio::test]
async fn undo_and_redo_survive_restart_and_branch_truncation_is_explicit()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(4)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    let initial_document_timestamp = timestamp(CREATED)?;
    store
        .undo(id, Revision::new(1), timestamp(LATER)?, move |history| {
            assert_eq!(
                history.target_document_updated_at,
                initial_document_timestamp
            );
            let target = history.target_document_updated_at.to_string();
            Ok((set_marker(history.document, None, &target)?, ()))
        })
        .await?;
    assert_eq!(store.get_project(id).await?.document, document(id)?);
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let command_document_timestamp = timestamp(UPDATED)?;
    reopened
        .redo(id, Revision::new(2), timestamp(LATER)?, move |history| {
            assert_eq!(
                history.target_document_updated_at,
                command_document_timestamp
            );
            let target = history.target_document_updated_at.to_string();
            Ok((set_marker(history.document, Some(1), &target)?, ()))
        })
        .await?;
    let initial_document_timestamp = timestamp(CREATED)?;
    reopened
        .undo(id, Revision::new(3), timestamp(LATER)?, move |history| {
            assert_eq!(
                history.target_document_updated_at,
                initial_document_timestamp
            );
            let target = history.target_document_updated_at.to_string();
            Ok((set_marker(history.document, None, &target)?, ()))
        })
        .await?;

    let replacement = |current: &ScenarioDocument| {
        Ok(CommandWrite {
            document: set_marker(current.clone(), Some(2), LATER)?,
            journal: journal(json!({"marker": 2}), Some(json!({"marker": null})), LATER)?,
            output: (),
        })
    };
    let rejected = reopened
        .execute_command(
            id,
            Revision::new(4),
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            replacement,
        )
        .await;
    assert!(matches!(
        rejected,
        Err(StoreError::RedoBranchRequiresTruncation)
    ));
    reopened
        .execute_command(
            id,
            Revision::new(4),
            RedoBranchPolicy::Truncate,
            CancellationToken::new(),
            replacement,
        )
        .await?;
    assert_eq!(reopened.history(id).await?.len(), 1);
    assert!(matches!(
        reopened
            .redo(id, Revision::new(5), timestamp(LATER)?, |history| Ok((
                history.document,
                ()
            )))
            .await,
        Err(StoreError::NoRedo)
    ));
    Ok(())
}

async fn seed_scenario_referencing_supplemental(
    store: &SqliteScenarioStore,
    id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let mut supplemental = staged_import(Revision::new(2), Vec::new(), timestamp(CREATED)?);
    supplemental.results.insert(
        "result-referencing".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 1,
            "result": {"score": 7}
        }))?,
    );
    supplemental.shared_records.insert(
        "shared-referencing".to_owned(),
        serde_json::to_vec(&json!({"scenarioId": id, "value": 1}))?,
    );
    supplemental.shared_records.insert(
        "shared-unrelated".to_owned(),
        serde_json::to_vec(&json!({"externalId": id.to_string()}))?,
    );
    supplemental.preferences.insert(
        "preference-referencing".to_owned(),
        serde_json::to_vec(&json!({"scenarioId": id, "value": 2}))?,
    );
    supplemental.preferences.insert(
        "preference-unrelated".to_owned(),
        serde_json::to_vec(&json!({"prose": format!("scenario {id}")}))?,
    );
    supplemental.assets.insert(
        "shared-asset".to_owned(),
        PortableAsset {
            bytes: b"shared inert bytes".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: false,
        },
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(supplemental),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn deleting_project_cascades_owned_rows_and_referencing_supplemental()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(5)?;
    let interval = NonZeroU32::new(1).ok_or("nonzero interval")?;
    let (store, _) = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::new(SnapshotPolicy::new(
            interval,
            MAX_SCENARIO_DOCUMENT_BYTES,
            3,
        )?),
    )
    .await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    seed_scenario_referencing_supplemental(&store, id).await?;

    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.execute(
        "INSERT INTO solve_runs (id, scenario_id, scenario_revision, input_hash, backend_id, backend_version, status, options_json, started_at) VALUES ('run-1', ?1, 1, 'hash', 'backend', '1', 'completed', '{}', ?2)",
        params![id.to_string(), CREATED],
    )?;
    connection.execute(
        "INSERT INTO solutions (id, solve_run_id, scenario_id, scenario_revision, status, accepted, normalized_solution_json, score_json, verification_report_json, created_at) VALUES ('solution-1', 'run-1', ?1, 1, 'verified', 1, '{}', '{}', '{}', ?2)",
        params![id.to_string(), CREATED],
    )?;
    connection.execute(
        "INSERT INTO ai_conversations (id, scenario_id, title, provider_id, model_id, created_at, updated_at) VALUES ('conversation-1', ?1, 'Conversation', 'provider', 'model', ?2, ?2)",
        params![id.to_string(), CREATED],
    )?;
    connection.execute(
        "INSERT INTO ai_messages (id, conversation_id, role, content_json, created_at) VALUES ('message-1', 'conversation-1', 'user', '{}', ?1)",
        [CREATED],
    )?;
    drop(connection);

    store.delete_project(id, Revision::new(1)).await?;
    let supplemental = store.library_snapshot().await?.sections;
    assert!(!supplemental.results.contains_key("result-referencing"));
    assert!(
        !supplemental
            .shared_records
            .contains_key("shared-referencing")
    );
    assert!(supplemental.shared_records.contains_key("shared-unrelated"));
    assert!(
        !supplemental
            .preferences
            .contains_key("preference-referencing")
    );
    assert!(
        supplemental
            .preferences
            .contains_key("preference-unrelated")
    );
    assert_eq!(
        supplemental.assets["shared-asset"],
        PortableAsset {
            bytes: b"shared inert bytes".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: false,
        }
    );
    let connection = Connection::open(&path)?;
    for table in [
        "scenarios",
        "scenario_snapshots",
        "retained_scenario_revisions",
        "command_journal",
        "scenario_history_state",
        "solve_runs",
        "solutions",
        "ai_conversations",
        "ai_messages",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let count: u32 = connection.query_row(&sql, [], |row| row.get(0))?;
        assert_eq!(count, 0, "rows remained in {table}");
    }
    Ok(())
}
#[cfg(unix)]
#[test]
fn private_application_directory_is_owner_only_and_refuses_symlinks() -> Result<(), Box<dyn Error>>
{
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = tempdir()?;
    let private = directory.path().join("application").join("backups");
    ensure_private_application_directory(&private)?;
    assert_eq!(
        std::fs::symlink_metadata(&private)?.permissions().mode() & 0o777,
        0o700
    );
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o777))?;
    ensure_private_application_directory(&private)?;
    assert_eq!(
        std::fs::symlink_metadata(&private)?.permissions().mode() & 0o777,
        0o700
    );

    let target = directory.path().join("attacker-controlled");
    std::fs::create_dir(&target)?;
    let linked = directory.path().join("linked-backups");
    symlink(&target, &linked)?;
    assert!(matches!(
        ensure_private_application_directory(&linked),
        Err(StoreError::PrivatePath(_))
    ));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn authoritative_database_and_safety_backup_are_private_and_refuse_indirection()
-> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    let directory = tempdir()?;
    let data_dir = directory.path().join("permissive-data");
    std::fs::create_dir(&data_dir)?;
    std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o777))?;
    let database = data_dir.join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&database).await?;
    assert_eq!(
        std::fs::metadata(&data_dir)?.permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&database)?.permissions().mode() & 0o777,
        0o600
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", database.display()));
        if sidecar.exists() {
            assert_eq!(
                std::fs::metadata(sidecar)?.permissions().mode() & 0o777,
                0o600
            );
        }
    }

    let backup_dir = directory.path().join("permissive-backups");
    std::fs::create_dir(&backup_dir)?;
    std::fs::set_permissions(&backup_dir, std::fs::Permissions::from_mode(0o777))?;
    let backup = backup_dir.join("safety.sqlite3");
    store.safety_backup(&backup).await?;
    assert_eq!(
        std::fs::metadata(&backup_dir)?.permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&backup)?.permissions().mode() & 0o777,
        0o600
    );

    let target = directory.path().join("attacker.sqlite3");
    std::fs::write(&target, b"not a database")?;
    let linked_database = data_dir.join("linked.sqlite3");
    symlink(&target, &linked_database)?;
    assert!(matches!(
        SqliteScenarioStore::open(&linked_database).await,
        Err(StoreError::PrivatePath(_))
    ));
    let linked_backup = backup_dir.join("linked-backup.sqlite3");
    symlink(&target, &linked_backup)?;
    assert!(matches!(
        store.safety_backup(&linked_backup).await,
        Err(StoreError::PrivatePath(_))
    ));
    let hard_linked_database = data_dir.join("hard-linked.sqlite3");
    std::fs::hard_link(&target, &hard_linked_database)?;
    assert!(matches!(
        SqliteScenarioStore::open(&hard_linked_database).await,
        Err(StoreError::PrivatePath(_))
    ));

    let actual_parent = directory.path().join("actual-private-parent");
    std::fs::create_dir(&actual_parent)?;
    let linked_parent = directory.path().join("linked-parent");
    symlink(&actual_parent, &linked_parent)?;
    assert!(matches!(
        SqliteScenarioStore::open(linked_parent.join("library.sqlite3")).await,
        Err(StoreError::PrivatePath(_))
    ));

    let socket_path = data_dir.join("special.sqlite3");
    let _socket = UnixListener::bind(&socket_path)?;
    assert!(matches!(
        SqliteScenarioStore::open(&socket_path).await,
        Err(StoreError::PrivatePath(_))
    ));
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn windows_platform_app_data_supports_private_store_creation() -> Result<(), Box<dyn Error>> {
    let app_data = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or("APPDATA is not configured")?;
    for precreate in [false, true] {
        let data_dir = app_data.join(format!("eutheto-store-test-{}", Uuid::now_v7()));
        if precreate {
            std::fs::create_dir(&data_dir)?;
        }
        let database = data_dir.join("library.sqlite3");
        let (store, _) = SqliteScenarioStore::open(&database)
            .await
            .map_err(|error| format!("platform app-data store initialization failed: {error:?}"))?;
        drop(store);
        std::fs::remove_dir_all(data_dir)?;
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn windows_authoritative_paths_apply_private_acls_and_refuse_links()
-> Result<(), Box<dyn Error>> {
    use std::os::windows::fs::symlink_file;

    let directory = tempdir()?;
    let data_dir = directory.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let database = data_dir.join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&database).await?;
    let backup_dir = directory.path().join("backups");
    std::fs::create_dir(&backup_dir)?;
    let backup = backup_dir.join("safety.sqlite3");
    store.safety_backup(&backup).await?;

    let target = directory.path().join("attacker.sqlite3");
    std::fs::write(&target, b"not a database")?;
    let hard_linked = data_dir.join("hard-linked.sqlite3");
    std::fs::hard_link(&target, &hard_linked)?;
    assert!(matches!(
        SqliteScenarioStore::open(&hard_linked).await,
        Err(StoreError::PrivatePath(_))
    ));
    let linked = data_dir.join("linked.sqlite3");
    if symlink_file(&target, &linked).is_ok() {
        assert!(matches!(
            SqliteScenarioStore::open(&linked).await,
            Err(StoreError::PrivatePath(_))
        ));
    }
    Ok(())
}

#[cfg(debug_assertions)]
async fn seed_and_advance_retained_revision(
    store: &SqliteScenarioStore,
    id: ScenarioId,
    revision_seven: &ScenarioDocument,
) -> Result<(), Box<dyn Error>> {
    let mut initial = staged_import(
        Revision::INITIAL,
        vec![(
            Revision::new(7),
            revision_seven.clone(),
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    initial.results.insert(
        "immutable-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 7,
            "result": {"score": 7}
        }))?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(initial),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .is_empty()
    );
    store
        .execute_command(
            id,
            Revision::new(7),
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(8), UPDATED)?,
                    journal: journal(json!({"marker": 8}), Some(json!({"marker": 7})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    let advanced = store.library_snapshot().await?;
    assert_eq!(advanced.projects[0].summary.revision, Revision::new(8));
    assert_eq!(advanced.scenario_revisions.len(), 1);
    assert_eq!(
        advanced.scenario_revisions[0].scenario.document,
        *revision_seven
    );
    assert_eq!(
        advanced.scenario_revisions[0].scenario.revision,
        Revision::new(7)
    );
    Ok(())
}

#[cfg(debug_assertions)]
fn exact_revision_restore(
    id: ScenarioId,
    revision_seven: &ScenarioDocument,
) -> Result<StagedLibraryApply, Box<dyn Error>> {
    let mut restore = staged_import(
        Revision::new(2),
        vec![(
            Revision::new(20),
            set_marker(document(id)?, Some(20), LATER)?,
            StagedDisposition::Replace,
        )],
        timestamp(CREATED)?,
    );
    restore.mode = RestoreMode::ReplaceLibrary;
    restore.scenario_revisions.push(ScenarioSnapshotV1::current(
        Revision::new(7),
        revision_seven.clone(),
        BTreeSet::new(),
    ));
    restore.results.insert(
        "immutable-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 7,
            "result": {"score": 7}
        }))?,
    );
    Ok(StagedLibraryApply::BackupRestore {
        restore: StagedBackupRestore {
            import: restore,
            remove_scenario_ids: BTreeSet::from([id]),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: Some("a".repeat(64)),
                safety_backup: SafetyBackupEvidence::Verified {
                    bundle_sha256: "verified-exact-revision".to_owned(),
                },
            },
        },
        settings: BTreeMap::new(),
    })
}

#[cfg(debug_assertions)]
async fn assert_atomic_restore_preserves_exact_revision(
    store: &SqliteScenarioStore,
    id: ScenarioId,
    staged_restore: StagedLibraryApply,
    revision_seven: &ScenarioDocument,
) -> Result<(), Box<dyn Error>> {
    store.set_failpoint(Failpoint::AfterSupplementalWrite)?;
    assert!(matches!(
        store
            .apply_staged_library(
                staged_restore.clone(),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InjectedFailure)
    ));
    let after_failure = store.library_snapshot().await?;
    assert_eq!(after_failure.projects[0].summary.revision, Revision::new(8));
    assert_eq!(
        after_failure.scenario_revision_high_water[&id],
        Revision::new(8)
    );
    assert_eq!(
        after_failure.scenario_revisions[0].scenario.document,
        *revision_seven
    );
    store
        .apply_staged_library(staged_restore, timestamp(LATER)?, CancellationToken::new())
        .await?;
    let restored = store.library_snapshot().await?;
    assert_eq!(restored.projects[0].summary.revision, Revision::new(20));
    assert_eq!(
        restored.scenario_revision_high_water[&id],
        Revision::new(20)
    );
    assert_eq!(restored.scenario_revisions.len(), 1);
    assert_eq!(
        restored.scenario_revisions[0].scenario.document,
        *revision_seven
    );
    Ok(())
}

#[cfg(debug_assertions)]
async fn replace_result_and_cleanup_retained_revision(
    store: &SqliteScenarioStore,
    id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let mut replace_result = staged_import(Revision::new(3), Vec::new(), timestamp(CREATED)?);
    replace_result.results.insert(
        "immutable-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 20,
            "result": {"score": 20}
        }))?,
    );
    replace_result
        .supplemental_replacements
        .insert(SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: "immutable-result".to_owned(),
        });
    store
        .apply_staged_library(
            StagedLibraryApply::Import(replace_result),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .is_empty()
    );
    store.delete_project(id, Revision::new(20)).await?;
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn retained_exact_revision_survives_advance_restart_atomic_restore_and_cleanup()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(80)?;
    let revision_seven = set_marker(document(id)?, Some(7), CREATED)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_and_advance_retained_revision(&store, id, &revision_seven).await?;
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened.library_snapshot().await?.scenario_revisions[0]
            .scenario
            .document,
        revision_seven
    );
    let staged_restore = exact_revision_restore(id, &revision_seven)?;
    assert_atomic_restore_preserves_exact_revision(&reopened, id, staged_restore, &revision_seven)
        .await?;
    drop(reopened);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened.library_snapshot().await?.scenario_revisions[0]
            .scenario
            .document,
        revision_seven
    );
    replace_result_and_cleanup_retained_revision(&reopened, id).await?;
    drop(reopened);

    let connection = Connection::open(&path)?;
    let retained_count: u32 = connection.query_row(
        "SELECT COUNT(*) FROM retained_scenario_revisions",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(retained_count, 0);
    Ok(())
}

#[tokio::test]
async fn all_skip_apply_is_a_true_no_effect() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let mut skipped = staged_import(Revision::INITIAL, Vec::new(), timestamp(CREATED)?);
    skipped
        .manifest_extensions
        .insert("vendor.skipped".to_owned(), json!({"ignored": true}));
    skipped
        .nonsemantic_extensions
        .insert("vendor.skipped".to_owned());
    let outcome = store
        .apply_staged_library(
            StagedLibraryApply::Import(skipped),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(outcome.library_revision, Revision::INITIAL);
    assert_eq!(outcome.created, 0);
    assert_eq!(outcome.replaced, 0);
    assert_eq!(outcome.removed, 0);
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.revision, Revision::INITIAL);
    assert!(snapshot.provenance.is_empty());
    assert!(snapshot.manifest_extensions.is_empty());
    assert!(snapshot.nonsemantic_extensions.is_empty());
    Ok(())
}

#[tokio::test]
async fn oversized_provenance_refuses_the_entire_transaction() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let existing_id = scenario_id(81)?;
    let imported_id = scenario_id(82)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(existing_id)?,
        })
        .await?;
    let mut staged = staged_import(
        Revision::new(1),
        vec![(
            Revision::new(5),
            document(imported_id)?,
            StagedDisposition::Create,
        )],
        timestamp(CREATED)?,
    );
    staged.provenance.source_file_sha256 = "x".repeat(4 * 1024 * 1024 + 1);
    assert!(matches!(
        store
            .apply_staged_library(
                StagedLibraryApply::Import(staged),
                timestamp(LATER)?,
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.revision, Revision::new(1));
    assert_eq!(snapshot.projects.len(), 1);
    assert_eq!(snapshot.projects[0].summary.id, existing_id);
    assert!(snapshot.provenance.is_empty());
    Ok(())
}

#[tokio::test]
async fn provenance_pruning_is_bounded_and_deterministic() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    drop(store);
    let connection = Connection::open(&path)?;
    for index in 0..130 {
        connection.execute(
            "INSERT INTO portable_import_provenance (source_bundle_id, source_application_json, original_format_version, original_schema_version, source_file_sha256, applied_migrations_json, binding_json, scenario_sources_json, source_created_at, applied_at) VALUES (?1, ?2, 1, 1, ?3, '[]', ?4, '[]', ?5, ?6)",
            params![
                BundleId::from_uuid(Uuid::now_v7()).to_string(),
                r#"{"name":"seed","version":"1"}"#,
                format!("seed-{index}"),
                r#"{"fileSha256":"seed","optionsSha256":"seed","localLibraryRevision":0,"formatVersion":1,"schemaVersion":1}"#,
                CREATED,
                UPDATED,
            ],
        )?;
    }
    drop(connection);

    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let mut effect = staged_import(Revision::INITIAL, Vec::new(), timestamp(CREATED)?);
    effect
        .shared_records
        .insert("retained".to_owned(), br#"{"value":true}"#.to_vec());
    store
        .apply_staged_library(
            StagedLibraryApply::Import(effect),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let provenance = store.library_snapshot().await?.provenance;
    assert_eq!(provenance.len(), 128);
    assert_eq!(provenance[0].source_file_sha256, "seed-3");
    assert_eq!(
        provenance
            .last()
            .map(|entry| entry.source_file_sha256.as_str()),
        Some("staged-test")
    );
    let latest = provenance.last().ok_or("new provenance row missing")?;
    assert_eq!(latest.source_created_at, timestamp(CREATED)?);
    assert_eq!(latest.applied_at, timestamp(LATER)?);
    drop(store);
    let connection = Connection::open(&path)?;
    let bytes: i64 = connection.query_row(
        "SELECT COALESCE(SUM(16 + length(CAST(source_bundle_id AS BLOB)) + length(CAST(source_application_json AS BLOB)) + length(CAST(source_file_sha256 AS BLOB)) + length(CAST(applied_migrations_json AS BLOB)) + length(CAST(binding_json AS BLOB)) + length(CAST(scenario_sources_json AS BLOB)) + length(CAST(source_created_at AS BLOB)) + length(CAST(applied_at AS BLOB))), 0) FROM portable_import_provenance",
        [],
        |row| row.get(0),
    )?;
    assert!(bytes <= 4 * 1024 * 1024);
    Ok(())
}

#[tokio::test]
async fn provenance_pruning_counts_multibyte_utf8_bytes() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    drop(store);
    let connection = Connection::open(&path)?;
    let source_application = serde_json::to_string(&ApplicationMetadata {
        name: "é".repeat(500_000),
        version: "1".to_owned(),
    })?;
    for index in 0..5 {
        connection.execute(
            "INSERT INTO portable_import_provenance (source_bundle_id, source_application_json, original_format_version, original_schema_version, source_file_sha256, applied_migrations_json, binding_json, scenario_sources_json, source_created_at, applied_at) VALUES (?1, ?2, 1, 1, ?3, '[]', ?4, '[]', ?5, ?6)",
            params![
                BundleId::from_uuid(Uuid::now_v7()).to_string(),
                source_application,
                format!("multibyte-{index}"),
                r#"{"fileSha256":"seed","optionsSha256":"seed","localLibraryRevision":0,"formatVersion":1,"schemaVersion":1}"#,
                CREATED,
                UPDATED,
            ],
        )?;
    }
    drop(connection);

    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let mut effect = staged_import(Revision::INITIAL, Vec::new(), timestamp(CREATED)?);
    effect
        .shared_records
        .insert("retained".to_owned(), br#"{"value":true}"#.to_vec());
    store
        .apply_staged_library(
            StagedLibraryApply::Import(effect),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    drop(store);

    let connection = Connection::open(&path)?;
    let bytes: i64 = connection.query_row(
        "SELECT COALESCE(SUM(16 + length(CAST(source_bundle_id AS BLOB)) + length(CAST(source_application_json AS BLOB)) + length(CAST(source_file_sha256 AS BLOB)) + length(CAST(applied_migrations_json AS BLOB)) + length(CAST(binding_json AS BLOB)) + length(CAST(scenario_sources_json AS BLOB)) + length(CAST(source_created_at AS BLOB)) + length(CAST(applied_at AS BLOB))), 0) FROM portable_import_provenance",
        [],
        |row| row.get(0),
    )?;
    assert!(bytes <= 4 * 1024 * 1024);
    Ok(())
}

#[tokio::test]
async fn results_excluded_replace_retains_the_exact_local_source_revision_after_restart()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let id = scenario_id(83)?;
    let local_revision = set_marker(document(id)?, Some(0), CREATED)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: local_revision.clone(),
        })
        .await?;
    let mut result = staged_import(Revision::new(1), Vec::new(), timestamp(CREATED)?);
    result.results.insert(
        "local-result".to_owned(),
        serde_json::to_vec(&json!({
            "resultId": Uuid::now_v7(),
            "scenarioId": id,
            "scenarioRevision": 0,
            "result": {"score": 1}
        }))?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(result),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;

    let replacement_document = set_marker(document(id)?, Some(5), LATER)?;
    let replacement = staged_import(
        Revision::new(2),
        vec![(
            Revision::new(5),
            replacement_document,
            StagedDisposition::Replace,
        )],
        timestamp(CREATED)?,
    );
    store
        .apply_staged_library(
            StagedLibraryApply::Import(replacement),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.projects[0].summary.revision, Revision::new(5));
    assert_eq!(snapshot.scenario_revisions.len(), 1);
    assert_eq!(
        snapshot.scenario_revisions[0].scenario.document,
        local_revision
    );
    assert!(snapshot.sections.results.contains_key("local-result"));
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let snapshot = reopened.library_snapshot().await?;
    assert_eq!(snapshot.scenario_revisions.len(), 1);
    assert_eq!(
        snapshot.scenario_revisions[0].scenario.document,
        local_revision
    );
    assert!(snapshot.sections.results.contains_key("local-result"));
    Ok(())
}

#[tokio::test]
async fn persisted_raw_secret_sentinel_is_rejected_on_snapshot_load() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    for (index, field) in [
        "secret",
        "providerApiKey",
        "oauthCredentialId",
        "apiKeyHandle",
        "apiKeyReference",
        "vendor.providerApiKey",
        "vendor:oauthCredentialId",
        "vendor/apiKeyHandle",
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path().join(format!("sentinel-{index}.sqlite3"));
        let (store, _) = SqliteScenarioStore::open(&path).await?;
        drop(store);
        let connection = Connection::open(&path)?;
        connection.execute(
            "INSERT INTO portable_sections (section, key, value) VALUES ('shared_records', 'sentinel', ?1)",
            [serde_json::to_vec(&json!({(field): "raw-secret-sentinel"}))?],
        )?;
        drop(connection);
        let (store, _) = SqliteScenarioStore::open(&path).await?;
        let metadata = store.library_metadata_snapshot().await?;
        assert_eq!(metadata.revision, Revision::INITIAL);
        assert_eq!(metadata.scenario_count, 0);
        assert!(matches!(
            store.library_snapshot().await,
            Err(StoreError::Integrity(_))
        ));
    }

    let safe_path = directory.path().join("safe-substring.sqlite3");
    let safe_value = serde_json::to_vec(&json!({"hockeyScore": 7}))?;
    let (store, _) = SqliteScenarioStore::open(&safe_path).await?;
    drop(store);
    let connection = Connection::open(&safe_path)?;
    connection.execute(
        "INSERT INTO portable_sections (section, key, value) VALUES ('shared_records', 'safe', ?1)",
        [&safe_value],
    )?;
    drop(connection);
    let (store, _) = SqliteScenarioStore::open(&safe_path).await?;
    assert_eq!(
        store.library_snapshot().await?.sections.shared_records["safe"],
        safe_value
    );
    Ok(())
}

#[tokio::test]
async fn retained_request_lookup_rejects_terminal_column_substitution() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("lookup.sqlite3");
    let scenario_id = scenario_id(301)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;
    let request = solve_request(scenario_id, 1, 1)?;
    let started = store.start_solve_run(request.clone()).await?;
    store
        .finalize_terminal_run(terminal_manifest(
            &started.input,
            started.started_at,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::BackendFailed,
            },
        )?)
        .await?;
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE solve_runs SET elapsed_ms = elapsed_ms + 1 WHERE id = ?1",
        [started.input.run_id.to_string()],
    )?;
    assert!(matches!(
        store.load_solve_run_by_request(request.request_id).await,
        Err(StoreError::InvalidPersistedRun(_))
    ));
    Ok(())
}

#[tokio::test]
async fn solve_start_is_idempotent_and_loads_only_the_exact_snapshot() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(301)?;
    let expected = document(scenario_id)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: expected.clone(),
        })
        .await?;

    let request = solve_request(scenario_id, 1, 1)?;
    assert!(
        store
            .load_solve_run_by_request(request.request_id)
            .await?
            .is_none()
    );
    let started = store.start_solve_run(request.clone()).await?;
    assert!(!started.reused);
    assert_eq!(started.started_at, request.started_at);
    let mut retry = request.clone();
    retry.run_id = SolveRunId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8001-000000000002")?);
    retry.started_at = timestamp(LATER)?;
    let mut before_snapshot = solve_request(scenario_id, 6, 6)?;
    before_snapshot.started_at = timestamp("2026-08-28T22:59:59Z")?;
    assert!(matches!(
        store.start_solve_run(before_snapshot).await,
        Err(StoreError::InvalidPersistedRun(_))
    ));
    let reused = store.start_solve_run(retry).await?;
    assert!(reused.reused);
    assert_eq!(reused.input, started.input);
    assert_eq!(reused.started_at, started.started_at);
    let retained = store
        .load_solve_run_by_request(request.request_id)
        .await?
        .ok_or("running request was not found")?;
    assert_eq!(retained.input, started.input);
    assert_eq!(retained.started_at, started.started_at);
    assert!(retained.manifest.is_none());

    let mut conflicting = request.clone();
    conflicting.model_hash = blake3_hex(b"different-model");
    assert!(matches!(
        store.start_solve_run(conflicting).await,
        Err(StoreError::SolveRequestIdConflict { request_id })
            if request_id == request.request_id
    ));
    let mut stale = solve_request(scenario_id, 3, 3)?;
    let mut colliding_run = solve_request(scenario_id, 5, 5)?;
    colliding_run.run_id = started.input.run_id;
    assert!(matches!(
        store.start_solve_run(colliding_run).await,
        Err(StoreError::SolveRunCollision(id)) if id == started.input.run_id
    ));
    stale.expected_revision = Revision::new(1);
    assert!(matches!(
        store.start_solve_run(stale).await,
        Err(StoreError::Conflict { expected, actual })
            if expected == Revision::new(1) && actual == Revision::INITIAL
    ));
    let loaded = store.load_solve_input(started.input.run_id).await?;
    #[cfg(debug_assertions)]
    {
        let rollback_request = solve_request(scenario_id, 4, 4)?;
        store.set_failpoint(Failpoint::AfterSolveRunInsert)?;
        assert!(matches!(
            store.start_solve_run(rollback_request.clone()).await,
            Err(StoreError::InjectedFailure)
        ));
        let connection = Connection::open(&path)?;
        let inserted: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM solve_runs WHERE id = ?1)",
            [rollback_request.run_id.to_string()],
            |row| row.get(0),
        )?;
        assert!(!inserted);
    }
    assert_eq!(loaded.input, started.input);
    assert_eq!(loaded.document, expected);
    drop(store);

    let mut altered = expected;
    altered.metadata.title = "Mutated snapshot".to_owned();
    let compressed = zstd::stream::encode_all(serde_json::to_vec(&altered)?.as_slice(), 3)?;
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE scenario_snapshots SET document_json_zstd = ?2 WHERE id = ?1",
        params![started.input.snapshot_id.to_string(), compressed],
    )?;
    drop(connection);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert!(matches!(
        reopened.load_solve_input(started.input.run_id).await,
        Err(StoreError::SnapshotMismatch(snapshot_id))
            if snapshot_id == started.input.snapshot_id
    ));
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
// One sequential fixture proves every terminal CAS path shares the same atomic boundary.
#[allow(clippy::too_many_lines)]
async fn terminal_finalizers_are_compare_and_set_and_atomic() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(302)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;

    let accepted_started = store
        .start_solve_run(solve_request(scenario_id, 10, 10)?)
        .await?;
    let accepted_run = accepted_started.input.clone();
    let accepted = accepted_result_for(&accepted_run, 10)?;
    let accepted_manifest = terminal_manifest(
        &accepted_run,
        accepted_started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    let evidence = BTreeMap::new();
    store
        .finalize_accepted_run(
            accepted.clone(),
            accepted_manifest.clone(),
            evidence.clone(),
        )
        .await?;
    assert!(matches!(
        store
            .finalize_terminal_run(terminal_manifest(
                &accepted_run,
                accepted_started.started_at,
                RunTerminalOutcomeV1::Interrupted,
            )?)
            .await,
        Err(StoreError::SolveRunTerminalConflict(id)) if id == accepted_run.run_id
    ));
    let snapshot = store.library_snapshot().await?;
    let canonical = snapshot
        .sections
        .results
        .get(&accepted.solution.solution_id.to_string())
        .ok_or_else(|| std::io::Error::other("canonical accepted result missing"))?;
    let portable = PortableAcceptedResultV2::from_json(canonical)?;
    assert_eq!(portable.accepted_result, accepted);
    assert_eq!(portable.evidence, evidence);

    let rolled_back_started = store
        .start_solve_run(solve_request(scenario_id, 11, 11)?)
        .await?;
    let rolled_back_run = rolled_back_started.input.clone();
    let rolled_back_result = accepted_result_for(&rolled_back_run, 11)?;
    let rolled_back_manifest = terminal_manifest(
        &rolled_back_run,
        rolled_back_started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            solution_id: rolled_back_result.solution.solution_id,
            accepted_result_checksum: rolled_back_result.checksum.clone(),
            verification_checksum: rolled_back_result.verification.checksum.clone(),
        },
    )?;
    store.set_failpoint(Failpoint::AfterAcceptedSolutionInsert)?;
    assert!(matches!(
        store
            .finalize_accepted_run(
                rolled_back_result.clone(),
                rolled_back_manifest.clone(),
                BTreeMap::new(),
            )
            .await,
        Err(StoreError::InjectedFailure)
    ));
    let connection = Connection::open(&path)?;
    let (status, solutions): (String, i64) = connection.query_row(
        "SELECT r.status, (SELECT COUNT(*) FROM solutions s WHERE s.solve_run_id = r.id) FROM solve_runs r WHERE r.id = ?1",
        [rolled_back_run.run_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(status, "running");
    assert_eq!(solutions, 0);
    drop(connection);
    store
        .finalize_accepted_run(rolled_back_result, rolled_back_manifest, BTreeMap::new())
        .await?;

    let quarantined_started = store
        .start_solve_run(solve_request(scenario_id, 12, 12)?)
        .await?;
    let quarantined_run = quarantined_started.input.clone();
    let quarantine_manifest = terminal_manifest(
        &quarantined_run,
        quarantined_started.started_at,
        RunTerminalOutcomeV1::VerificationAlarm {
            diagnostic_code: "verification.candidate-rejected".to_owned(),
        },
    )?;
    let diagnostics = CandidateDiagnosticsV1 {
        values: BTreeMap::from([(
            "candidate_count".to_owned(),
            eutheto_types::SafeDiagnosticValue::Integer(1),
        )]),
    };
    store.set_failpoint(Failpoint::AfterQuarantineWrite)?;
    assert!(matches!(
        store
            .finalize_quarantined_run(quarantine_manifest.clone(), diagnostics.clone())
            .await,
        Err(StoreError::InjectedFailure)
    ));
    store
        .finalize_quarantined_run(quarantine_manifest, diagnostics)
        .await?;

    for (suffix, status, expected_status) in [
        (13, SolveStatus::Infeasible, "infeasible"),
        (14, SolveStatus::Unbounded, "unbounded"),
        (
            15,
            SolveStatus::NoSolutionWithinLimit,
            "no_solution_within_limit",
        ),
        (16, SolveStatus::Cancelled, "cancelled"),
        (17, SolveStatus::InvalidModel, "invalid_model"),
        (18, SolveStatus::BackendUnavailable, "backend_unavailable"),
        (19, SolveStatus::BackendFailed, "backend_failed"),
    ] {
        let started = store
            .start_solve_run(solve_request(scenario_id, suffix, suffix)?)
            .await?;
        let input = started.input.clone();
        store
            .finalize_terminal_run(terminal_manifest(
                &input,
                started.started_at,
                RunTerminalOutcomeV1::NoResult { status },
            )?)
            .await?;
        let retained = store
            .load_solve_run_by_request(input.request_id)
            .await?
            .ok_or("terminal request was not found")?;
        assert_eq!(retained.input, input);
        assert_eq!(
            retained
                .manifest
                .ok_or("terminal manifest was not retained")?
                .outcome,
            RunTerminalOutcomeV1::NoResult { status }
        );
        let connection = Connection::open(&path)?;
        let stored_status: String = connection.query_row(
            "SELECT status FROM solve_runs WHERE id = ?1",
            [input.run_id.to_string()],
            |row| row.get(0),
        )?;
        assert_eq!(stored_status, expected_status);
    }

    let interrupted_started = store
        .start_solve_run(solve_request(scenario_id, 20, 20)?)
        .await?;
    let interrupted_run = interrupted_started.input.clone();
    store
        .finalize_terminal_run(terminal_manifest(
            &interrupted_run,
            interrupted_started.started_at,
            RunTerminalOutcomeV1::Interrupted,
        )?)
        .await?;

    let connection = Connection::open(&path)?;
    let quarantine: (String, i64) = connection.query_row(
        "SELECT r.status, (SELECT COUNT(*) FROM solutions s WHERE s.solve_run_id = r.id) FROM solve_runs r WHERE r.id = ?1",
        [quarantined_run.run_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let accepted_optimal_status: String = connection.query_row(
        "SELECT status FROM solve_runs WHERE id = ?1",
        [rolled_back_run.run_id.to_string()],
        |row| row.get(0),
    )?;
    let interrupted_status: String = connection.query_row(
        "SELECT status FROM solve_runs WHERE id = ?1",
        [interrupted_run.run_id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(quarantine, ("quarantined".to_owned(), 0));
    assert_eq!(accepted_optimal_status, "optimal");
    assert_eq!(interrupted_status, "interrupted");
    drop(connection);

    let late_started = store
        .start_solve_run(solve_request(scenario_id, 21, 21)?)
        .await?;
    let late_run = late_started.input.clone();
    let late_result = accepted_result_for(&late_run, 21)?;
    let late_manifest = RunManifestV1::new(
        late_run.run_id,
        late_run.checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: late_result.solution.solution_id,
            accepted_result_checksum: late_result.checksum.clone(),
            verification_checksum: late_result.verification.checksum.clone(),
        },
        late_started.started_at,
        shift_timestamp(late_started.started_at, 120_001)?,
        Some(DurationMillis::new(120_001)?),
        None,
        Some(DurationMillis::new(500)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    assert!(matches!(
        store
            .finalize_accepted_run(late_result, late_manifest, BTreeMap::new())
            .await,
        Err(StoreError::InvalidPersistedResult(_))
    ));

    let original_document = document(scenario_id)?;
    store
        .execute_command(
            scenario_id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), None, UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .iter()
            .any(|revision| revision.scenario.document == original_document)
    );
    Ok(())
}

#[tokio::test]
async fn accepted_persistence_after_recovery_cutoff_is_rejected_atomically()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(310)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;
    let mut request = solve_request(scenario_id, 70, 70)?;
    request.started_at = Rfc3339Timestamp::from_timestamp(
        jiff::Timestamp::now().checked_sub(std::time::Duration::from_secs(131))?,
    );
    let started = store.start_solve_run(request).await?;
    let input = started.input.clone();
    let accepted = accepted_result_for(&input, 70)?;
    let manifest = terminal_manifest(
        &input,
        started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    assert!(
        manifest.finished_at.as_timestamp()
            <= started
                .started_at
                .as_timestamp()
                .checked_add(std::time::Duration::from_mins(2))?
    );
    assert!(matches!(
        store
            .finalize_accepted_run(accepted, manifest, BTreeMap::new())
            .await,
        Err(StoreError::InvalidPersistedResult(_))
    ));
    let connection = Connection::open(&path)?;
    let solution_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM solutions", [], |row| row.get(0))?;
    let status: String = connection.query_row(
        "SELECT status FROM solve_runs WHERE id = ?1",
        [input.run_id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(solution_count, 0);
    assert_eq!(status, "running");
    Ok(())
}

#[tokio::test]
async fn startup_terminalizes_only_a_valid_v2_running_input_once() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(303)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;
    let mut expired_request = solve_request(scenario_id, 20, 20)?;
    expired_request.started_at = timestamp(CREATED)?;
    let run = store.start_solve_run(expired_request).await?.input;
    drop(store);

    let (reopened, outcome) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(outcome.recovery.interrupted_solve_run_ids, vec![run.run_id]);
    drop(reopened);
    let (reopened, second) = SqliteScenarioStore::open(&path).await?;
    assert!(second.recovery.interrupted_solve_run_ids.is_empty());
    let connection = Connection::open(&path)?;
    let (manifest_json, elapsed_ms): (String, Option<i64>) = connection.query_row(
        "SELECT run_manifest_json, elapsed_ms FROM solve_runs WHERE id = ?1",
        [run.run_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let manifest = RunManifestV1::from_json(manifest_json.as_bytes())?;
    assert!(matches!(
        &manifest.outcome,
        RunTerminalOutcomeV1::Interrupted
    ));
    assert_eq!(manifest.elapsed_milliseconds, None);
    assert_eq!(elapsed_ms, None);
    drop(reopened);
    Ok(())
}

#[tokio::test]
async fn second_store_open_preserves_running_input_before_recovery_deadline()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(307)?;
    let (owner, _) = SqliteScenarioStore::open(&path).await?;
    owner
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;
    let mut request = solve_request(scenario_id, 50, 50)?;
    request.started_at = Rfc3339Timestamp::from_timestamp(
        jiff::Timestamp::now().checked_sub(std::time::Duration::from_secs(121))?,
    );
    let run = owner.start_solve_run(request).await?.input;

    let (observer, outcome) = SqliteScenarioStore::open(&path).await?;
    assert!(outcome.recovery.interrupted_solve_run_ids.is_empty());
    let connection = Connection::open(&path)?;
    let status: String = connection.query_row(
        "SELECT status FROM solve_runs WHERE id = ?1",
        [run.run_id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(status, "running");
    drop(observer);
    drop(owner);
    Ok(())
}

#[tokio::test]
// Keep edit, replacement and restart coupled to the original accepted revision.
#[allow(clippy::too_many_lines)]
async fn accepted_r0_survives_edit_ordinary_replace_and_reopen() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(308)?;
    let original = document(scenario_id)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: original.clone(),
        })
        .await?;
    let started = store
        .start_solve_run(solve_request(scenario_id, 60, 60)?)
        .await?;
    let input = started.input.clone();
    store
        .execute_command(
            scenario_id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), None, UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    assert_eq!(
        store.load_solve_input(input.run_id).await?.document,
        original
    );
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .is_empty()
    );
    let connection = Connection::open(&path)?;
    let retained_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM retained_scenario_revisions",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(retained_count, 1);
    drop(connection);
    let accepted = accepted_result_for(&input, 60)?;
    store
        .finalize_accepted_run(
            accepted.clone(),
            terminal_manifest(
                &input,
                started.started_at,
                RunTerminalOutcomeV1::Accepted {
                    status: SolveStatus::Feasible,
                    solution_id: accepted.solution.solution_id,
                    accepted_result_checksum: accepted.checksum.clone(),
                    verification_checksum: accepted.verification.checksum.clone(),
                },
            )?,
            BTreeMap::new(),
        )
        .await?;
    assert_eq!(store.library_snapshot().await?.scenario_revisions.len(), 1);
    let mut replacement = staged_import(
        Revision::new(3),
        vec![(
            Revision::new(2),
            set_marker(document(scenario_id)?, Some(2), LATER)?,
            StagedDisposition::Replace,
        )],
        timestamp(CREATED)?,
    );
    replacement.mode = RestoreMode::ImportScenario;
    store
        .apply_staged_library(
            StagedLibraryApply::Import(replacement),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;
    let snapshot = store.library_snapshot().await?;
    assert_eq!(snapshot.scenario_revisions[0].scenario.document, original);
    assert!(
        snapshot
            .sections
            .results
            .contains_key(&accepted.solution.solution_id.to_string())
    );
    drop(store);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let snapshot = reopened.library_snapshot().await?;
    assert_eq!(snapshot.scenario_revisions[0].scenario.document, original);
    assert!(
        snapshot
            .sections
            .results
            .contains_key(&accepted.solution.solution_id.to_string())
    );
    Ok(())
}

#[tokio::test]
async fn nonaccepted_terminal_run_prunes_its_retained_source_revision() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(309)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(scenario_id)?,
        })
        .await?;
    let started = store
        .start_solve_run(solve_request(scenario_id, 61, 61)?)
        .await?;
    let input = started.input.clone();
    store
        .execute_command(
            scenario_id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), None, UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .is_empty()
    );
    let connection = Connection::open(&path)?;
    let retained_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM retained_scenario_revisions",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(retained_count, 1);
    drop(connection);
    store
        .finalize_terminal_run(terminal_manifest(
            &input,
            started.started_at,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Infeasible,
            },
        )?)
        .await?;
    assert!(
        store
            .library_snapshot()
            .await?
            .scenario_revisions
            .is_empty()
    );
    let connection = Connection::open(&path)?;
    let retained_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM retained_scenario_revisions",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(retained_count, 0);
    Ok(())
}

#[tokio::test]
// The full import/copy/replace sequence keeps opaque authority and collision rollback auditable.
#[allow(clippy::too_many_lines)]
async fn imported_v2_wrappers_remain_opaque_and_identity_conflicts_roll_back()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let source_path = directory.path().join("source.sqlite3");
    let target_path = directory.path().join("target.sqlite3");
    let imported_scenario_id = scenario_id(304)?;
    let scenario = document(imported_scenario_id)?;
    let (source, _) = SqliteScenarioStore::open(&source_path).await?;
    source
        .create_project(NewProject {
            document: scenario.clone(),
        })
        .await?;
    let started = source
        .start_solve_run(solve_request(imported_scenario_id, 30, 30)?)
        .await?;
    let input = started.input.clone();
    let accepted = accepted_result_for(&input, 30)?;
    let manifest = terminal_manifest(
        &input,
        started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    source
        .finalize_accepted_run(accepted.clone(), manifest, BTreeMap::new())
        .await?;
    let source_snapshot = source.library_snapshot().await?;
    let result_key = accepted.solution.solution_id.to_string();
    let canonical = source_snapshot.sections.results[&result_key].clone();

    let (target, _) = SqliteScenarioStore::open(&target_path).await?;
    let mut initial = staged_import(
        Revision::INITIAL,
        vec![(Revision::INITIAL, scenario, StagedDisposition::Create)],
        timestamp(CREATED)?,
    );
    initial
        .results
        .insert(format!("{result_key}.json"), canonical.clone());
    target
        .apply_staged_library(
            StagedLibraryApply::Import(initial),
            timestamp(UPDATED)?,
            CancellationToken::new(),
        )
        .await?;
    let restored = target.library_snapshot().await?;
    assert_eq!(
        restored.sections.results[&format!("{result_key}.json")],
        canonical
    );
    let connection = Connection::open(&target_path)?;
    let (accepted_rows, opaque_rows): (i64, i64) = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM solutions WHERE accepted = 1), (SELECT COUNT(*) FROM portable_sections WHERE section = 'results')",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(accepted_rows, 0);
    assert_eq!(opaque_rows, 1);
    drop(connection);

    let mut identical = staged_import(restored.revision, Vec::new(), timestamp(UPDATED)?);
    identical
        .results
        .insert(format!("{result_key}.json"), canonical.clone());
    identical
        .supplemental_replacements
        .insert(SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: format!("{result_key}.json"),
        });
    target
        .apply_staged_library(
            StagedLibraryApply::Import(identical),
            timestamp(LATER)?,
            CancellationToken::new(),
        )
        .await?;

    let mut conflicting: Value = serde_json::from_slice(&canonical)?;
    conflicting
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("portable result is not an object"))?
        .insert("opaqueImportMarker".to_owned(), json!("different"));
    let before_conflict = target.library_snapshot().await?.revision;
    let mut staged_conflict = staged_import(before_conflict, Vec::new(), timestamp(LATER)?);
    staged_conflict.results.insert(
        format!("{result_key}.json"),
        serde_json::to_vec(&conflicting)?,
    );
    staged_conflict
        .supplemental_replacements
        .insert(SupplementalIdentity {
            section: SupplementalSectionKind::Results,
            key: format!("{result_key}.json"),
        });
    assert!(matches!(
        target
            .apply_staged_library(
                StagedLibraryApply::Import(staged_conflict),
                timestamp(LATER)?,
                CancellationToken::new(),
            )
            .await,
        Err(StoreError::InvalidStagedApply(_) | StoreError::IdentityCollision(_))
    ));
    assert_eq!(target.library_snapshot().await?.revision, before_conflict);

    let remapped_path = directory.path().join("remapped.sqlite3");
    let (remapped_store, _) = SqliteScenarioStore::open(&remapped_path).await?;
    let mut remapped = staged_import(
        Revision::INITIAL,
        vec![(
            Revision::INITIAL,
            document(imported_scenario_id)?,
            StagedDisposition::CreateCopy,
        )],
        timestamp(CREATED)?,
    );
    let original_id = scenario_id(305)?;
    remapped.scenarios[0].original_id = original_id;
    remapped.scenarios[0]
        .id_remap
        .insert(original_id.as_uuid(), imported_scenario_id.as_uuid());
    remapped
        .results
        .insert(format!("{result_key}.json"), canonical);
    remapped_store
        .apply_staged_library(
            StagedLibraryApply::Import(remapped),
            timestamp(UPDATED)?,
            CancellationToken::new(),
        )
        .await?;
    let connection = Connection::open(&remapped_path)?;
    let canonical_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM solutions WHERE accepted = 1",
        [],
        |row| row.get(0),
    )?;
    let opaque_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM portable_sections WHERE section = 'results'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(canonical_count, 0);
    assert_eq!(opaque_count, 1);
    Ok(())
}
#[cfg(unix)]
#[tokio::test]
async fn unrelated_valid_v1_backup_is_rejected_without_migrating_live_database()
-> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let backup_path =
        std::path::PathBuf::from(format!("{}.pre-v2-backup.sqlite3", path.to_string_lossy()));
    let fixture = include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3");
    std::fs::write(&path, fixture)?;
    std::fs::write(&backup_path, fixture)?;
    for private_path in [&path, &backup_path] {
        let mut permissions = std::fs::metadata(private_path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(private_path, permissions)?;
    }
    let unrelated = Connection::open(&backup_path)?;
    unrelated.execute(
        "UPDATE app_metadata SET value = '1' WHERE key = 'portable_library_revision'",
        [],
    )?;
    drop(unrelated);

    assert!(matches!(
        SqliteScenarioStore::open(&path).await,
        Err(StoreError::Integrity(_))
    ));
    let live = Connection::open(&path)?;
    let version: u32 = live.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 1);
    assert!(
        std::fs::read_dir(directory.path())?
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".compare-"))
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn v1_upgrade_backup_observes_the_commit_before_its_writer_lock() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    std::fs::write(
        &path,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;
    let writer = Connection::open(&path)?;
    writer.pragma_update(None, "journal_mode", "WAL")?;
    writer.execute_batch("BEGIN IMMEDIATE")?;
    writer.execute(
        "INSERT INTO app_metadata (key, value) VALUES ('migration_lock_marker', 'committed-before-backup')",
        [],
    )?;

    let hook = V2MigrationBeginTestHook::new();
    let actor_hook = hook.clone();
    let open_path = path.clone();
    let (done_sender, done_receiver) = std::sync::mpsc::channel();
    let open_task = tokio::spawn(async move {
        let result = SqliteScenarioStore::open_with_options(
            open_path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_v2_migration_begin_test_hook(actor_hook),
        )
        .await;
        let _ignored = done_sender.send(());
        result
    });
    let wait_hook = hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_before_begin()).await?;

    writer.execute_batch("COMMIT")?;
    hook.release();
    tokio::task::spawn_blocking(move || {
        done_receiver.recv_timeout(std::time::Duration::from_secs(5))
    })
    .await??;
    let (store, outcome) = open_task.await??;
    let backup_path = outcome
        .retained_backup_path
        .ok_or_else(|| std::io::Error::other("missing retained V1 backup"))?;
    drop(store);

    for database_path in [&path, &backup_path] {
        let connection = Connection::open(database_path)?;
        let marker: String = connection.query_row(
            "SELECT value FROM app_metadata WHERE key = 'migration_lock_marker'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(marker, "committed-before-backup");
    }
    let live = Connection::open(&path)?;
    let live_version: u32 = live.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let backup = Connection::open(&backup_path)?;
    let backup_version: u32 = backup.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(live_version, 5);
    assert_eq!(backup_version, 1);
    Ok(())
}

#[tokio::test]
async fn released_v1_fixture_is_backed_up_and_upgraded_to_v5() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    std::fs::write(
        &path,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;

    let (store, outcome) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(outcome.applied_migrations, vec![2, 3, 4, 5]);
    let backup_path = outcome
        .retained_backup_path
        .ok_or_else(|| std::io::Error::other("missing retained V1 backup"))?;
    assert_eq!(
        backup_path,
        std::path::PathBuf::from(format!("{}.pre-v2-backup.sqlite3", path.to_string_lossy()))
    );
    assert!(backup_path.is_file());
    drop(store);

    let current = Connection::open(&path)?;
    let current_version: u32 =
        current.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(current_version, 5);
    let backup = Connection::open(&backup_path)?;
    let backup_version: u32 = backup.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let backup_check: String = backup.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    assert_eq!(backup_version, 1);
    assert_eq!(backup_check, "ok");
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn v2_migration_failure_retains_a_valid_v1_backup() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    std::fs::write(
        &path,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;
    let result = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::new(SnapshotPolicy::default()).with_failpoint(Failpoint::AfterV2MigrationSql),
    )
    .await;
    assert!(matches!(result, Err(StoreError::InjectedFailure)));

    let backup_path =
        std::path::PathBuf::from(format!("{}.pre-v2-backup.sqlite3", path.to_string_lossy()));
    let current = Connection::open(&path)?;
    let current_version: u32 =
        current.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let backup = Connection::open(&backup_path)?;
    let backup_version: u32 = backup.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let backup_check: String = backup.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    assert_eq!(current_version, 1);
    assert_eq!(backup_version, 1);
    assert_eq!(backup_check, "ok");
    Ok(())
}

#[tokio::test]
// One migration fixture proves legacy payload preservation and every status classification.
#[allow(clippy::too_many_lines)]
async fn v2_migration_classifies_legacy_rows_without_rewriting_payloads()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    std::fs::write(
        &path,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;
    let scenario_id = scenario_id(306)?;
    let document = document(scenario_id)?;
    let running_id =
        SolveRunId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8001-000000000040")?);
    let terminal_id =
        SolveRunId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8001-000000000041")?);
    let connection = Connection::open(&path)?;
    connection.execute(
        "INSERT INTO scenarios (id, domain_pack_id, domain_schema_version, title, description, revision, document_json, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4, 0, ?5, ?6, ?6)",
        params![
            scenario_id.to_string(),
            document.domain_pack.id.to_string(),
            document.metadata.title,
            document.metadata.description,
            serde_json::to_string(&document)?,
            CREATED,
        ],
    )?;
    connection.execute(
        "INSERT INTO scenario_history_state (scenario_id, cursor_sequence, branch_generation) VALUES (?1, 0, 0)",
        [scenario_id.to_string()],
    )?;
    for (run_id, status, error_json) in [
        (
            running_id,
            "running",
            Some(r#"{"safe":"preserved-running"}"#),
        ),
        (
            terminal_id,
            "completed",
            Some(r#"{"safe":"preserved-terminal"}"#),
        ),
    ] {
        connection.execute(
            "INSERT INTO solve_runs (id, scenario_id, scenario_revision, input_hash, backend_id, backend_version, status, options_json, started_at, error_json) VALUES (?1, ?2, 0, 'legacy-hash', 'legacy.backend', '1', ?3, '{\"legacy\":true}', ?4, ?5)",
            params![
                run_id.to_string(),
                scenario_id.to_string(),
                status,
                CREATED,
                error_json,
            ],
        )?;
    }
    connection.execute(
        "INSERT INTO solutions (id, solve_run_id, scenario_id, scenario_revision, status, accepted, normalized_solution_json, score_json, verification_report_json, created_at) VALUES (?1, ?2, ?3, 0, 'accepted', 1, '{\"legacy\":true}', '{\"legacy\":true}', '{\"legacy\":true}', ?4)",
        params![
            SolutionId::from_uuid(Uuid::parse_str(
                "018f47f2-e880-7000-8003-000000000040"
            )?)
            .to_string(),
            terminal_id.to_string(),
            scenario_id.to_string(),
            CREATED,
        ],
    )?;
    drop(connection);

    let (store, outcome) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(outcome.applied_migrations, vec![2, 3, 4, 5]);
    assert!(outcome.recovery.interrupted_solve_run_ids.is_empty());
    drop(store);
    let connection = Connection::open(&path)?;
    let running: (String, String, Option<String>) = connection.query_row(
        "SELECT status, options_json, error_json FROM solve_runs WHERE id = ?1",
        [running_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let terminal: (String, String, Option<String>) = connection.query_row(
        "SELECT status, options_json, error_json FROM solve_runs WHERE id = ?1",
        [terminal_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let solution: (String, i64, String) = connection.query_row(
        "SELECT status, accepted, normalized_solution_json FROM solutions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(
        running,
        (
            "legacy_interrupted".to_owned(),
            r#"{"legacy":true}"#.to_owned(),
            Some(r#"{"safe":"preserved-running"}"#.to_owned()),
        )
    );
    assert_eq!(
        terminal,
        (
            "legacy_terminal".to_owned(),
            r#"{"legacy":true}"#.to_owned(),
            Some(r#"{"safe":"preserved-terminal"}"#.to_owned()),
        )
    );
    assert_eq!(
        solution,
        (
            "legacy_unverified".to_owned(),
            0,
            r#"{"legacy":true}"#.to_owned(),
        )
    );
    Ok(())
}

#[tokio::test]
async fn startup_applies_required_pragmas_schema_and_indexes() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, outcome) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(outcome.schema_version, 5);
    let diagnostics = store.diagnostics().await?;
    assert!(diagnostics.foreign_keys);
    assert_eq!(diagnostics.journal_mode, "wal");
    assert_eq!(diagnostics.synchronous, 1);
    assert_eq!(diagnostics.busy_timeout_ms, 5_000);
    assert!(!diagnostics.trusted_schema);
    assert_eq!(diagnostics.sqlite_length_limit_bytes, 128 * 1024 * 1024);
    assert_eq!(diagnostics.schema_version, 5);
    for table in [
        "app_metadata",
        "scenarios",
        "scenario_snapshots",
        "retained_scenario_revisions",
        "command_journal",
        "scenario_history_state",
        "solve_runs",
        "solutions",
        "counterfactual_jobs",
        "app_settings",
        "portable_library_metadata",
        "ai_conversations",
        "portable_sections",
        "portable_import_provenance",
        "safety_backup_failure_receipts",
        "scenario_revision_high_water",
        "scenario_identity_owners",
        "ai_messages",
        "schema_migrations",
    ] {
        assert!(
            diagnostics
                .tables
                .iter()
                .any(|candidate| candidate == table)
        );
    }
    for index in [
        "scenarios_by_recency",
        "solve_runs_by_scenario",
        "accepted_solutions_by_scenario",
        "canonical_solution_by_run",
        "counterfactual_jobs_by_scenario",
        "solve_runs_by_request_id",
        "counterfactual_jobs_by_derived_run",
        "counterfactual_jobs_by_derived_request",
        "counterfactual_jobs_running_recovery",
        "ai_conversations_by_scenario",
        "command_journal_by_history",
    ] {
        assert!(
            diagnostics
                .indexes
                .iter()
                .any(|candidate| candidate == index)
        );
    }
    Ok(())
}

#[tokio::test]
async fn released_migration_checksum_mismatch_is_rejected() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    drop(store);

    let connection = Connection::open(&path)?;
    assert_eq!(
        connection.execute(
            "UPDATE schema_migrations SET checksum = ?1 WHERE version = 1",
            ["checksum-for-changed-released-sql"],
        )?,
        1
    );
    drop(connection);

    assert!(matches!(
        SqliteScenarioStore::open(&path).await,
        Err(StoreError::MigrationChanged { version: 1 })
    ));
    let connection = Connection::open(&path)?;
    let retained_checksum: String = connection.query_row(
        "SELECT checksum FROM schema_migrations WHERE version = 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(retained_checksum, "checksum-for-changed-released-sql");
    Ok(())
}

#[tokio::test]
async fn released_migration_checksums_and_registry_pragma_disagreement_are_rejected()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let checksum_path = directory.path().join("checksum.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&checksum_path).await?;
    drop(store);
    let connection = Connection::open(&checksum_path)?;
    connection.execute(
        "UPDATE schema_migrations SET checksum = 'changed-v2' WHERE version = 2",
        [],
    )?;
    drop(connection);
    assert!(matches!(
        SqliteScenarioStore::open(&checksum_path).await,
        Err(StoreError::MigrationChanged { version: 2 })
    ));

    let v4_checksum_path = directory.path().join("v4-checksum.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&v4_checksum_path).await?;
    drop(store);
    let connection = Connection::open(&v4_checksum_path)?;
    connection.execute(
        "UPDATE schema_migrations SET checksum = 'changed-v4' WHERE version = 4",
        [],
    )?;
    drop(connection);
    assert!(matches!(
        SqliteScenarioStore::open(&v4_checksum_path).await,
        Err(StoreError::MigrationChanged { version: 4 })
    ));

    let registry_path = directory.path().join("registry.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&registry_path).await?;
    drop(store);
    let connection = Connection::open(&registry_path)?;
    connection.pragma_update(None, "user_version", 1)?;
    drop(connection);
    assert!(matches!(
        SqliteScenarioStore::open(&registry_path).await,
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[tokio::test]
async fn newer_database_is_rejected_without_schema_mutation() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let connection = Connection::open(&path)?;
    connection
        .execute_batch("CREATE TABLE future_marker (value TEXT); PRAGMA user_version = 6;")?;
    drop(connection);
    let bytes_before = std::fs::read(&path)?;
    let wal_path = directory.path().join("library.sqlite3-wal");
    let shm_path = directory.path().join("library.sqlite3-shm");
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());

    let result = SqliteScenarioStore::open(&path).await;
    assert!(matches!(
        result,
        Err(StoreError::NewerSchema {
            found: 6,
            supported: 5
        })
    ));
    assert_eq!(std::fs::read(&path)?, bytes_before);
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());
    let connection = Connection::open(&path)?;
    let marker_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'future_marker')",
        [],
        |row| row.get(0),
    )?;
    let migration_registry_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    assert!(marker_exists);
    assert!(!migration_registry_exists);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn migration_failpoint_rolls_back_schema_and_registry() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let result = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::new(SnapshotPolicy::default()).with_failpoint(Failpoint::AfterMigrationSql),
    )
    .await;
    assert!(matches!(result, Err(StoreError::InjectedFailure)));

    let connection = Connection::open(&path)?;
    let application_tables: u32 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    let schema_version: u32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(application_tables, 0);
    assert_eq!(schema_version, 0);
    Ok(())
}

#[tokio::test]
async fn startup_does_not_fabricate_a_manifest_for_an_invalid_or_legacy_run()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let scenario_id = scenario_id(6)?;
    let expected = document(scenario_id)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: expected.clone(),
        })
        .await?;
    drop(store);

    let run_id = SolveRunId::from_uuid(Uuid::now_v7());
    let connection = Connection::open(&path)?;
    connection.execute(
        "INSERT INTO solve_runs (id, scenario_id, scenario_revision, input_hash, backend_id, backend_version, status, options_json, started_at) VALUES (?1, ?2, 1, 'hash', 'backend', '1', 'running', '{}', ?3)",
        params![run_id.to_string(), scenario_id.to_string(), CREATED],
    )?;
    drop(connection);

    let (reopened, outcome) = SqliteScenarioStore::open(&path).await?;
    assert!(outcome.recovery.interrupted_solve_run_ids.is_empty());
    let persisted = reopened.get_project(scenario_id).await?;
    assert_eq!(persisted.summary.revision, Revision::INITIAL);
    assert_eq!(persisted.document, expected);

    let connection = Connection::open(&path)?;
    let (status, finished_at, error_json): (String, Option<String>, Option<String>) = connection
        .query_row(
            "SELECT status, finished_at, error_json FROM solve_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    assert_eq!(status, "running");
    assert!(finished_at.is_none());
    assert!(error_json.is_none());
    drop(reopened);
    Ok(())
}

#[tokio::test]
async fn v5_registers_private_counterfactual_run_ownership() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-v5.sqlite3");
    let (store, outcome) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(outcome.applied_migrations, vec![1, 2, 3, 4, 5]);
    drop(store);
    let connection = Connection::open(&path)?;
    let migration_name: String = connection.query_row(
        "SELECT name FROM schema_migrations WHERE version = 5",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(migration_name, "V5_counterfactual_run_ownership.sql");
    let columns = connection
        .prepare("PRAGMA table_info(counterfactual_jobs)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(columns.iter().any(|column| column == "derived_run_id"));
    assert!(columns.iter().any(|column| column == "derived_request_id"));
    Ok(())
}

#[tokio::test]
async fn queued_counterfactual_failure_accepts_only_pre_run_categories()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory
        .path()
        .join("counterfactual-pre-run-failure.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 91).await?;
    let budget_request = retry_counterfactual_request(&fixture, 92)?;
    store
        .start_counterfactual_job(budget_request.clone())
        .await?;
    let failed = store
        .transition_counterfactual_job(
            budget_request.job_id,
            CounterfactualJobTransitionV1::Failed {
                finished_at: budget_request.created_at,
                error: CounterfactualJobErrorV1 {
                    kind: CounterfactualFailureKind::BudgetExhausted,
                },
            },
        )
        .await?;
    assert_eq!(failed.state, CounterfactualJobState::Failed);
    assert_eq!(
        failed.error,
        Some(CounterfactualJobErrorV1 {
            kind: CounterfactualFailureKind::BudgetExhausted,
        })
    );

    let candidate_request = retry_counterfactual_request(&fixture, 93)?;
    store
        .start_counterfactual_job(candidate_request.clone())
        .await?;
    assert!(matches!(
        store
            .transition_counterfactual_job(
                candidate_request.job_id,
                CounterfactualJobTransitionV1::Failed {
                    finished_at: candidate_request.created_at,
                    error: CounterfactualJobErrorV1 {
                        kind: CounterfactualFailureKind::InvalidCandidate,
                    },
                },
            )
            .await,
        Err(StoreError::CounterfactualTransitionConflict(job_id))
            if job_id == candidate_request.job_id
    ));
    Ok(())
}

#[tokio::test]
async fn stale_base_job_is_durably_queued_then_failed_without_a_derived_run()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-stale-base.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 95).await?;
    store
        .execute_command(
            fixture.request.semantics.scenario_id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            |current| {
                Ok(CommandWrite {
                    document: set_marker(current.clone(), Some(1), UPDATED)?,
                    journal: journal(json!({"marker": 1}), Some(json!({"marker": null})), UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;

    let queued = store
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    assert_eq!(queued.record.state, CounterfactualJobState::Queued);
    assert!(!queued.reused);
    let failed = store
        .transition_counterfactual_job(
            fixture.request.job_id,
            CounterfactualJobTransitionV1::Failed {
                finished_at: shift_timestamp(fixture.request.created_at, 1_000)?,
                error: CounterfactualJobErrorV1 {
                    kind: CounterfactualFailureKind::StaleRevision,
                },
            },
        )
        .await?;
    assert_eq!(failed.state, CounterfactualJobState::Failed);
    assert_eq!(
        failed.error,
        Some(CounterfactualJobErrorV1 {
            kind: CounterfactualFailureKind::StaleRevision,
        })
    );
    drop(store);
    let connection = Connection::open(path)?;
    let ownership: (Option<String>, Option<String>) = connection.query_row(
        "SELECT derived_run_id, derived_request_id FROM counterfactual_jobs WHERE id = ?1",
        [fixture.request.job_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(ownership, (None, None));
    let solve_run_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM solve_runs", [], |row| row.get(0))?;
    assert_eq!(solve_run_count, 1);
    Ok(())
}

#[tokio::test]
async fn counterfactual_owned_run_start_is_atomic_exact_and_cross_wire_safe()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-owned-start.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 80).await?;
    let run_request = owned_counterfactual_solve_request(&fixture, 80)?;
    store
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    store
        .transition_counterfactual_job(
            fixture.request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: fixture.request.created_at,
            },
        )
        .await?;
    let mut automatic_backend = run_request.clone();
    automatic_backend.solve_options.backend = BackendSelection::Auto;
    assert!(matches!(
        store
            .start_counterfactual_run(fixture.request.job_id, automatic_backend)
            .await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));
    let mut changed_seed = run_request.clone();
    changed_seed.solve_options.random_seed = changed_seed.solve_options.random_seed.wrapping_add(1);
    assert!(matches!(
        store
            .start_counterfactual_run(fixture.request.job_id, changed_seed)
            .await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));
    let started = store
        .start_counterfactual_run(fixture.request.job_id, run_request.clone())
        .await?;
    assert!(!started.reused);
    let replay = store
        .start_counterfactual_run(fixture.request.job_id, run_request.clone())
        .await?;
    assert!(replay.reused);
    assert_eq!(replay.input, started.input);
    assert_eq!(replay.started_at, started.started_at);

    let other_request = retry_counterfactual_request(&fixture, 81)?;
    store
        .start_counterfactual_job(other_request.clone())
        .await?;
    store
        .transition_counterfactual_job(
            other_request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: other_request.created_at,
            },
        )
        .await?;
    assert!(matches!(
        store
            .start_counterfactual_run(other_request.job_id, run_request)
            .await,
        Err(StoreError::CounterfactualTransitionConflict(job_id))
            if job_id == other_request.job_id
    ));
    drop(store);
    let connection = Connection::open(&path)?;
    let owner: (Option<String>, Option<String>) = connection.query_row(
        "SELECT derived_run_id, derived_request_id FROM counterfactual_jobs WHERE id = ?1",
        [fixture.request.job_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(owner.0, Some(started.input.run_id.to_string()));
    assert_eq!(owner.1, Some(started.input.request_id.to_string()));
    let other_owner: (Option<String>, Option<String>) = connection.query_row(
        "SELECT derived_run_id, derived_request_id FROM counterfactual_jobs WHERE id = ?1",
        [other_request.job_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(other_owner, (None, None));
    Ok(())
}

#[tokio::test]
async fn counterfactual_atomic_completion_uses_transaction_visible_authority()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory
        .path()
        .join("counterfactual-owned-complete.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 82).await?;
    let started = start_owned_counterfactual_run(&store, &fixture, 82).await?;
    let finalization = completed_accepted_finalization(&fixture, &started, 82)?;
    let solution_id = match &finalization {
        CounterfactualRunFinalizationV1::CompletedAccepted {
            accepted_result, ..
        } => accepted_result.solution.solution_id,
        _ => unreachable!(),
    };
    let completed = store
        .finalize_counterfactual_run(fixture.request.job_id, finalization)
        .await?;
    assert_eq!(completed.state, CounterfactualJobState::Completed);
    assert!(completed.result.is_some());
    let accepted = store.load_accepted_result(solution_id).await?;
    assert_eq!(accepted.portable.run_input, started.input);
    assert!(matches!(
        store
            .request_counterfactual_cancel(
                fixture.request.job_id,
                counterfactual_request_id(83)?,
                completed
                    .finished_at
                    .ok_or_else(|| std::io::Error::other("missing completion time"))?,
            )
            .await?,
        CounterfactualCancelOutcomeV1::AlreadyTerminal { record }
            if record == completed
    ));

    let proof_request = retry_counterfactual_request(&fixture, 84)?;
    let proof_fixture = CounterfactualStoreFixture {
        request: proof_request,
        base_input: fixture.base_input.clone(),
        base_manifest: fixture.base_manifest.clone(),
        base_accepted: fixture.base_accepted.clone(),
    };
    let proof_run = start_owned_counterfactual_run(&store, &proof_fixture, 84).await?;
    let proof = store
        .finalize_counterfactual_run(
            proof_fixture.request.job_id,
            completed_no_result_finalization(&proof_fixture, &proof_run)?,
        )
        .await?;
    assert_eq!(proof.state, CounterfactualJobState::Completed);
    assert!(matches!(
        proof.result.as_ref().map(|result| &result.conclusion),
        Some(CounterfactualConclusionV1::NotDistinguishedWithinBudget)
    ));
    Ok(())
}

#[tokio::test]
async fn counterfactual_no_result_rejects_completion_past_immutable_budget()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-over-budget.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 94).await?;
    let started = start_owned_counterfactual_run(&store, &fixture, 94).await?;
    let over_budget = fixture
        .request
        .semantics
        .total_budget_milliseconds
        .value()
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("test budget cannot be incremented"))?;
    let manifest = RunManifestV1::new(
        started.input.run_id,
        started.input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::NoSolutionWithinLimit,
        },
        started.started_at,
        shift_timestamp(started.started_at, over_budget)?,
        Some(DurationMillis::new(over_budget)?),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let compilation = CounterfactualCompilationBindingV1::new(
        fixture.request.semantics.base_model_hash.clone(),
        fixture.request.condition.checksum.clone(),
        started.input.model_hash.clone(),
        fixture.request.semantics.objective_policy_hash.clone(),
    )?;
    let result = CounterfactualResultV1::new(
        fixture.request.clone(),
        fixture.base_input.clone(),
        fixture.base_manifest.clone(),
        compilation,
        started.input.clone(),
        manifest.clone(),
        CounterfactualConclusionV1::NotDistinguishedWithinBudget,
    )?;
    assert!(matches!(
        store
            .finalize_counterfactual_run(
                fixture.request.job_id,
                CounterfactualRunFinalizationV1::CompletedNoResult {
                    manifest,
                    result: Box::new(result),
                },
            )
            .await,
        Err(StoreError::InvalidPersistedResult(message))
            if message.contains("immutable solve deadline")
    ));
    assert_eq!(
        store
            .load_counterfactual_job(fixture.request.job_id)
            .await?
            .state,
        CounterfactualJobState::Running
    );
    drop(store);
    let connection = Connection::open(path)?;
    let (status, manifest_json): (String, Option<String>) = connection.query_row(
        "SELECT status, run_manifest_json FROM solve_runs WHERE id = ?1",
        [started.input.run_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(status, "running");
    assert!(manifest_json.is_none());
    Ok(())
}

#[tokio::test]
async fn cancellation_winner_rolls_back_accepted_authority_then_finalizes_cancelled()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-cancel-race.sqlite3");
    let (first, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&first, 85).await?;
    let started = start_owned_counterfactual_run(&first, &fixture, 85).await?;
    let accepted_finalization = completed_accepted_finalization(&fixture, &started, 85)?;
    let (second, _) = SqliteScenarioStore::open(&path).await?;
    second
        .request_counterfactual_cancel(
            fixture.request.job_id,
            counterfactual_request_id(86)?,
            fixture.request.created_at,
        )
        .await?;
    assert!(matches!(
        first
            .finalize_counterfactual_run(fixture.request.job_id, accepted_finalization)
            .await,
        Err(StoreError::CounterfactualTransitionConflict(job_id))
            if job_id == fixture.request.job_id
    ));
    let connection = Connection::open(&path)?;
    let (status, manifest, accepted_count): (String, Option<String>, i64) = connection.query_row(
        "SELECT r.status, r.run_manifest_json, (SELECT COUNT(*) FROM solutions WHERE solve_run_id = r.id AND accepted = 1) FROM solve_runs r WHERE r.id = ?1",
        [started.input.run_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(status, "running");
    assert!(manifest.is_none());
    assert_eq!(accepted_count, 0);
    drop(connection);

    let cancelled_manifest = terminal_manifest(
        &started.input,
        started.started_at,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Cancelled,
        },
    )?;
    let cancelled = first
        .finalize_counterfactual_run(
            fixture.request.job_id,
            CounterfactualRunFinalizationV1::Cancelled {
                manifest: cancelled_manifest,
            },
        )
        .await?;
    assert_eq!(cancelled.state, CounterfactualJobState::Cancelled);
    let connection = Connection::open(&path)?;
    let status: String = connection.query_row(
        "SELECT status FROM solve_runs WHERE id = ?1",
        [started.input.run_id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(status, "cancelled");
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn startup_recovers_expired_running_counterfactual_jobs_without_results()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-recovery.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 87).await?;
    let started_at = fixture.request.created_at;
    let interrupted_request = CounterfactualJobRequestV1::new(
        counterfactual_job_id(88)?,
        counterfactual_request_id(88)?,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        started_at,
    )?;
    let cancelled_request = CounterfactualJobRequestV1::new(
        counterfactual_job_id(89)?,
        counterfactual_request_id(89)?,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        started_at,
    )?;
    for request in [&interrupted_request, &cancelled_request] {
        store.start_counterfactual_job(request.clone()).await?;
        store
            .transition_counterfactual_job(
                request.job_id,
                CounterfactualJobTransitionV1::Running {
                    started_at: request.created_at,
                },
            )
            .await?;
    }
    let mut interrupted_run_request = owned_counterfactual_solve_request(&fixture, 88)?;
    interrupted_run_request.started_at = started_at;
    let interrupted_run = store
        .start_counterfactual_run(interrupted_request.job_id, interrupted_run_request)
        .await?;
    let mut cancelled_run_request = owned_counterfactual_solve_request(&fixture, 89)?;
    cancelled_run_request.started_at = started_at;
    let cancelled_run = store
        .start_counterfactual_run(cancelled_request.job_id, cancelled_run_request)
        .await?;
    store
        .request_counterfactual_cancel(
            cancelled_request.job_id,
            counterfactual_request_id(90)?,
            cancelled_request.created_at,
        )
        .await?;
    let mut maximum_budget_semantics = fixture.request.semantics.clone();
    maximum_budget_semantics.total_budget_milliseconds = DurationMillis::new(30_000)?;
    let maximum_budget_request = CounterfactualJobRequestV1::new(
        counterfactual_job_id(91)?,
        counterfactual_request_id(91)?,
        maximum_budget_semantics,
        fixture.request.condition.clone(),
        started_at,
    )?;
    store
        .start_counterfactual_job(maximum_budget_request.clone())
        .await?;
    store
        .transition_counterfactual_job(
            maximum_budget_request.job_id,
            CounterfactualJobTransitionV1::Running { started_at },
        )
        .await?;
    drop(store);

    let recovery_now = shift_timestamp(started_at, 20_000)?;
    let (store, outcome) = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::new(SnapshotPolicy::default()).with_recovery_now(recovery_now.as_timestamp()),
    )
    .await?;
    assert_eq!(
        outcome.recovery.recovered_counterfactual_job_ids,
        vec![interrupted_request.job_id, cancelled_request.job_id]
    );
    assert_eq!(
        outcome.recovery.interrupted_solve_run_ids,
        vec![interrupted_run.input.run_id, cancelled_run.input.run_id]
    );
    let interrupted = store
        .load_counterfactual_job(interrupted_request.job_id)
        .await?;
    assert_eq!(interrupted.state, CounterfactualJobState::Interrupted);
    assert!(interrupted.result.is_none());
    assert!(interrupted.error.is_none());
    let cancelled = store
        .load_counterfactual_job(cancelled_request.job_id)
        .await?;
    assert_eq!(cancelled.state, CounterfactualJobState::Cancelled);
    assert!(cancelled.result.is_none());
    assert!(cancelled.error.is_none());
    let maximum_budget = store
        .load_counterfactual_job(maximum_budget_request.job_id)
        .await?;
    assert_eq!(maximum_budget.state, CounterfactualJobState::Running);
    assert!(maximum_budget.finished_at.is_none());
    drop(store);
    let connection = Connection::open(&path)?;
    for run_id in [interrupted_run.input.run_id, cancelled_run.input.run_id] {
        let (status, manifest_json): (String, String) = connection.query_row(
            "SELECT status, run_manifest_json FROM solve_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(status, "interrupted");
        let manifest: RunManifestV1 = serde_json::from_str(&manifest_json)?;
        assert!(matches!(
            &manifest.outcome,
            RunTerminalOutcomeV1::Interrupted
        ));
        assert_eq!(manifest.elapsed_milliseconds, None);
    }
    drop(connection);
    let (_, second) = SqliteScenarioStore::open(&path).await?;
    assert!(second.recovery.recovered_counterfactual_job_ids.is_empty());
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn counterfactual_start_load_authority_idempotency_and_identity_are_exact()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-start.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 1).await?;
    let revision_before = store.library_metadata_snapshot().await?.revision;
    let started = store
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    assert!(!started.reused);
    assert_eq!(started.record.state, CounterfactualJobState::Queued);
    assert_eq!(
        store
            .load_counterfactual_job(fixture.request.job_id)
            .await?,
        started.record
    );
    let retry = CounterfactualJobRequestV1::new(
        counterfactual_job_id(2)?,
        fixture.request.request_id,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        shift_timestamp(fixture.request.created_at, 1)?,
    )?;
    let reused = store.start_counterfactual_job(retry).await?;
    assert!(reused.reused);
    assert_eq!(reused.record, started.record);
    assert_eq!(
        store.library_metadata_snapshot().await?.revision,
        revision_before
    );
    drop(store);

    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let reused_after_restart = store
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    assert!(reused_after_restart.reused);
    assert_eq!(reused_after_restart.record, started.record);
    let mut conflicting_semantics = fixture.request.semantics.clone();
    conflicting_semantics.total_budget_milliseconds = DurationMillis::new(
        conflicting_semantics
            .total_budget_milliseconds
            .value()
            .saturating_add(1),
    )?;
    let conflicting_retry = CounterfactualJobRequestV1::new(
        counterfactual_job_id(5)?,
        fixture.request.request_id,
        conflicting_semantics,
        fixture.request.condition.clone(),
        fixture.request.created_at,
    )?;
    assert!(matches!(
        store.start_counterfactual_job(conflicting_retry).await,
        Err(StoreError::CounterfactualRequestIdConflict { request_id })
            if request_id == fixture.request.request_id
    ));

    let mut bad_semantics = fixture.request.semantics.clone();
    bad_semantics.base.result_checksum = blake3_hex(b"not-the-local-base");
    let invalid_base = CounterfactualJobRequestV1::new(
        counterfactual_job_id(3)?,
        counterfactual_request_id(3)?,
        bad_semantics,
        fixture.request.condition.clone(),
        fixture.request.created_at,
    )?;
    assert!(matches!(
        store.start_counterfactual_job(invalid_base).await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));

    let colliding_job = CounterfactualJobRequestV1::new(
        CounterfactualJobId::from_uuid(fixture.request.semantics.snapshot_id.as_uuid()),
        counterfactual_request_id(4)?,
        fixture.request.semantics.clone(),
        fixture.request.condition.clone(),
        fixture.request.created_at,
    )?;
    assert!(matches!(
        store.start_counterfactual_job(colliding_job).await,
        Err(StoreError::IdentityCollision(identity))
            if identity == fixture.request.semantics.snapshot_id.as_uuid()
    ));
    drop(store);
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE counterfactual_jobs SET request_json = '{}' WHERE id = ?1",
        [fixture.request.job_id.to_string()],
    )?;
    drop(connection);
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    assert!(matches!(
        store.load_counterfactual_job(fixture.request.job_id).await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));
    drop(store);
    Ok(())
}

#[tokio::test]
async fn counterfactual_capacity_is_transactional_idempotent_and_released_by_terminal_state()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-capacity.sqlite3");
    let (first, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&first, 200).await?;
    let (second, _) = SqliteScenarioStore::open(&path).await?;

    let mut active = Vec::new();
    for suffix in 201..(201 + MAX_ACTIVE_COUNTERFACTUAL_JOBS - 1) {
        let request = retry_counterfactual_request(&fixture, u16::try_from(suffix)?)?;
        first.start_counterfactual_job(request.clone()).await?;
        active.push(request);
    }

    let race_a = retry_counterfactual_request(&fixture, 210)?;
    let race_b = retry_counterfactual_request(&fixture, 211)?;
    let (left, right) = tokio::join!(
        first.start_counterfactual_job(race_a.clone()),
        second.start_counterfactual_job(race_b.clone()),
    );
    let (winner, rejected) = match (left, right) {
        (Ok(started), Err(StoreError::CounterfactualCapacityExceeded { maximum }))
            if maximum == MAX_ACTIVE_COUNTERFACTUAL_JOBS =>
        {
            (started.record.request, race_b)
        }
        (Err(StoreError::CounterfactualCapacityExceeded { maximum }), Ok(started))
            if maximum == MAX_ACTIVE_COUNTERFACTUAL_JOBS =>
        {
            (started.record.request, race_a)
        }
        other => return Err(format!("unexpected capacity race result: {other:?}").into()),
    };
    active.push(winner.clone());

    let replay = CounterfactualJobRequestV1::new(
        counterfactual_job_id(212)?,
        winner.request_id,
        winner.semantics.clone(),
        winner.condition.clone(),
        shift_timestamp(winner.created_at, 1)?,
    )?;
    let replayed = first.start_counterfactual_job(replay).await?;
    assert!(replayed.reused);
    assert_eq!(replayed.record.request, winner);

    assert!(matches!(
        first.start_counterfactual_job(rejected.clone()).await,
        Err(StoreError::CounterfactualCapacityExceeded { maximum })
            if maximum == MAX_ACTIVE_COUNTERFACTUAL_JOBS
    ));
    assert!(matches!(
        first.load_counterfactual_job(rejected.job_id).await,
        Err(StoreError::CounterfactualJobNotFound(job_id)) if job_id == rejected.job_id
    ));

    let released = &active[0];
    first
        .transition_counterfactual_job(
            released.job_id,
            CounterfactualJobTransitionV1::Failed {
                finished_at: released.created_at,
                error: CounterfactualJobErrorV1 {
                    kind: CounterfactualFailureKind::BudgetExhausted,
                },
            },
        )
        .await?;
    let admitted = first.start_counterfactual_job(rejected).await?;
    assert!(!admitted.reused);
    assert_eq!(admitted.record.state, CounterfactualJobState::Queued);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn counterfactual_transitions_and_cancellation_enforce_the_complete_state_matrix()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-transitions.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 10).await?;
    let running_request = retry_counterfactual_request(&fixture, 11)?;
    store
        .start_counterfactual_job(running_request.clone())
        .await?;
    let started_at = shift_timestamp(running_request.created_at, 1_000)?;
    let running = CounterfactualJobTransitionV1::Running { started_at };
    assert_eq!(
        store
            .transition_counterfactual_job(running_request.job_id, running.clone())
            .await?
            .state,
        CounterfactualJobState::Running
    );
    assert_eq!(
        store
            .transition_counterfactual_job(running_request.job_id, running)
            .await?
            .state,
        CounterfactualJobState::Running
    );
    assert!(matches!(
        store
            .transition_counterfactual_job(
                running_request.job_id,
                CounterfactualJobTransitionV1::Running {
                    started_at: shift_timestamp(started_at, 1)?
                }
            )
            .await,
        Err(StoreError::CounterfactualTransitionConflict(_))
    ));
    assert!(matches!(
        store
            .request_counterfactual_cancel(
                running_request.job_id,
                counterfactual_request_id(12)?,
                running_request.created_at,
            )
            .await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));
    let cancel_id = counterfactual_request_id(12)?;
    let requested = store
        .request_counterfactual_cancel(running_request.job_id, cancel_id, started_at)
        .await?;
    assert!(matches!(
        requested,
        CounterfactualCancelOutcomeV1::Requested {
            ref record,
            reused: false
        } if record.state == CounterfactualJobState::Running
    ));
    assert!(matches!(
        store
            .request_counterfactual_cancel(
                running_request.job_id,
                cancel_id,
                shift_timestamp(started_at, 1)?,
            )
            .await?,
        CounterfactualCancelOutcomeV1::Requested { reused: true, .. }
    ));
    let cancelled = store
        .transition_counterfactual_job(
            running_request.job_id,
            CounterfactualJobTransitionV1::Cancelled {
                finished_at: shift_timestamp(started_at, 1)?,
            },
        )
        .await?;
    assert_eq!(cancelled.state, CounterfactualJobState::Cancelled);

    let queued_request = retry_counterfactual_request(&fixture, 13)?;
    store
        .start_counterfactual_job(queued_request.clone())
        .await?;
    let queued_cancel_id = counterfactual_request_id(14)?;
    let queued_cancelled = store
        .request_counterfactual_cancel(
            queued_request.job_id,
            queued_cancel_id,
            queued_request.created_at,
        )
        .await?;
    assert!(matches!(
        queued_cancelled,
        CounterfactualCancelOutcomeV1::Requested {
            ref record,
            reused: false
        } if record.state == CounterfactualJobState::Cancelled
            && record.finished_at == Some(queued_request.created_at)
    ));
    assert!(matches!(
        store
            .transition_counterfactual_job(
                queued_request.job_id,
                CounterfactualJobTransitionV1::Interrupted {
                    finished_at: shift_timestamp(queued_request.created_at, 1)?
                }
            )
            .await,
        Err(StoreError::CounterfactualTransitionConflict(_))
    ));
    let cross_job_request = retry_counterfactual_request(&fixture, 17)?;
    store
        .start_counterfactual_job(cross_job_request.clone())
        .await?;
    assert!(matches!(
        store
            .request_counterfactual_cancel(
                cross_job_request.job_id,
                queued_cancel_id,
                cross_job_request.created_at,
            )
            .await,
        Err(StoreError::CounterfactualCancelRequestIdConflict { .. })
    ));

    let interrupted_request = retry_counterfactual_request(&fixture, 18)?;
    store
        .start_counterfactual_job(interrupted_request.clone())
        .await?;
    store
        .transition_counterfactual_job(
            interrupted_request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: interrupted_request.created_at,
            },
        )
        .await?;
    let interrupted = store
        .transition_counterfactual_job(
            interrupted_request.job_id,
            CounterfactualJobTransitionV1::Interrupted {
                finished_at: shift_timestamp(interrupted_request.created_at, 1)?,
            },
        )
        .await?;
    assert_eq!(interrupted.state, CounterfactualJobState::Interrupted);

    let failed_request = retry_counterfactual_request(&fixture, 15)?;
    store
        .start_counterfactual_job(failed_request.clone())
        .await?;
    let failed_transition = CounterfactualJobTransitionV1::Failed {
        finished_at: failed_request.created_at,
        error: CounterfactualJobErrorV1 {
            kind: CounterfactualFailureKind::CompilationFailed,
        },
    };
    let failed = store
        .transition_counterfactual_job(failed_request.job_id, failed_transition.clone())
        .await?;
    assert_eq!(failed.state, CounterfactualJobState::Failed);
    assert_eq!(
        store
            .transition_counterfactual_job(failed_request.job_id, failed_transition)
            .await?,
        failed
    );
    assert!(matches!(
        store
            .request_counterfactual_cancel(
                failed_request.job_id,
                counterfactual_request_id(16)?,
                failed_request.created_at,
            )
            .await?,
        CounterfactualCancelOutcomeV1::AlreadyTerminal { record }
            if record == failed
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn counterfactual_completion_uses_only_local_authority_and_linearizes_with_cancel()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory
        .path()
        .join("counterfactual-linearization.sqlite3");
    let (first, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&first, 20).await?;
    first
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    first
        .transition_counterfactual_job(
            fixture.request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: fixture.request.created_at,
            },
        )
        .await?;
    let unpersisted = counterfactual_result(&first, &fixture, 20, false).await?;
    assert!(matches!(
        first
            .transition_counterfactual_job(
                fixture.request.job_id,
                CounterfactualJobTransitionV1::Completed {
                    result: Box::new(unpersisted)
                }
            )
            .await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));

    let terminal_first_request = retry_counterfactual_request(&fixture, 21)?;
    let terminal_first_fixture = CounterfactualStoreFixture {
        request: terminal_first_request.clone(),
        base_input: fixture.base_input.clone(),
        base_manifest: fixture.base_manifest.clone(),
        base_accepted: fixture.base_accepted.clone(),
    };
    first
        .start_counterfactual_job(terminal_first_request.clone())
        .await?;
    first
        .transition_counterfactual_job(
            terminal_first_request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: terminal_first_request.created_at,
            },
        )
        .await?;
    let terminal_result = counterfactual_result(&first, &terminal_first_fixture, 21, true).await?;
    let completed_transition = CounterfactualJobTransitionV1::Completed {
        result: Box::new(terminal_result),
    };
    let completed = first
        .transition_counterfactual_job(terminal_first_request.job_id, completed_transition.clone())
        .await?;
    assert_eq!(completed.state, CounterfactualJobState::Completed);
    assert_eq!(
        first
            .transition_counterfactual_job(terminal_first_request.job_id, completed_transition,)
            .await?,
        completed
    );
    let (second, _) = SqliteScenarioStore::open(&path).await?;
    assert!(matches!(
        second
            .request_counterfactual_cancel(
                terminal_first_request.job_id,
                counterfactual_request_id(22)?,
                completed
                    .finished_at
                    .ok_or_else(|| std::io::Error::other("missing finish"))?,
            )
            .await?,
        CounterfactualCancelOutcomeV1::AlreadyTerminal { record }
            if record == completed
    ));

    let cancel_first_request = retry_counterfactual_request(&fixture, 23)?;
    let cancel_first_fixture = CounterfactualStoreFixture {
        request: cancel_first_request.clone(),
        base_input: fixture.base_input.clone(),
        base_manifest: fixture.base_manifest.clone(),
        base_accepted: fixture.base_accepted.clone(),
    };
    first
        .start_counterfactual_job(cancel_first_request.clone())
        .await?;
    first
        .transition_counterfactual_job(
            cancel_first_request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: cancel_first_request.created_at,
            },
        )
        .await?;
    let cancel_first_result =
        counterfactual_result(&first, &cancel_first_fixture, 23, true).await?;
    second
        .request_counterfactual_cancel(
            cancel_first_request.job_id,
            counterfactual_request_id(24)?,
            cancel_first_request.created_at,
        )
        .await?;
    assert!(matches!(
        first
            .transition_counterfactual_job(
                cancel_first_request.job_id,
                CounterfactualJobTransitionV1::Completed {
                    result: Box::new(cancel_first_result)
                }
            )
            .await,
        Err(StoreError::CounterfactualTransitionConflict(_))
    ));

    let alternative_request = retry_counterfactual_request(&fixture, 25)?;
    let alternative_fixture = CounterfactualStoreFixture {
        request: alternative_request.clone(),
        base_input: fixture.base_input.clone(),
        base_manifest: fixture.base_manifest.clone(),
        base_accepted: fixture.base_accepted.clone(),
    };
    first
        .start_counterfactual_job(alternative_request.clone())
        .await?;
    first
        .transition_counterfactual_job(
            alternative_request.job_id,
            CounterfactualJobTransitionV1::Running {
                started_at: alternative_request.created_at,
            },
        )
        .await?;
    let (forged_alternative, exact_alternative) =
        accepted_counterfactual_results(&first, &alternative_fixture, 25).await?;
    assert!(matches!(
        first
            .transition_counterfactual_job(
                alternative_request.job_id,
                CounterfactualJobTransitionV1::Completed {
                    result: Box::new(forged_alternative)
                }
            )
            .await,
        Err(StoreError::InvalidPersistedCounterfactual(_))
    ));
    assert_eq!(
        first
            .load_counterfactual_job(alternative_request.job_id)
            .await?
            .state,
        CounterfactualJobState::Running
    );
    let exact_completed = first
        .transition_counterfactual_job(
            alternative_request.job_id,
            CounterfactualJobTransitionV1::Completed {
                result: Box::new(exact_alternative),
            },
        )
        .await?;
    assert_eq!(exact_completed.state, CounterfactualJobState::Completed);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn counterfactual_insert_failpoint_rolls_back_and_survives_reopen()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("counterfactual-failpoint.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    let fixture = counterfactual_store_fixture(&store, 30).await?;
    store.set_failpoint(Failpoint::AfterCounterfactualJobInsert)?;
    assert!(matches!(
        store
            .start_counterfactual_job(fixture.request.clone())
            .await,
        Err(StoreError::InjectedFailure)
    ));
    assert!(matches!(
        store.load_counterfactual_job(fixture.request.job_id).await,
        Err(StoreError::CounterfactualJobNotFound(_))
    ));
    drop(store);
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    assert!(matches!(
        store.load_counterfactual_job(fixture.request.job_id).await,
        Err(StoreError::CounterfactualJobNotFound(_))
    ));
    let started = store
        .start_counterfactual_job(fixture.request.clone())
        .await?;
    assert_eq!(started.record.state, CounterfactualJobState::Queued);
    store.set_failpoint(Failpoint::AfterCounterfactualTransition)?;
    assert!(matches!(
        store
            .transition_counterfactual_job(
                fixture.request.job_id,
                CounterfactualJobTransitionV1::Running {
                    started_at: fixture.request.created_at,
                },
            )
            .await,
        Err(StoreError::InjectedFailure)
    ));
    assert_eq!(
        store
            .load_counterfactual_job(fixture.request.job_id)
            .await?
            .state,
        CounterfactualJobState::Queued
    );
    store.set_failpoint(Failpoint::AfterCounterfactualCancelWrite)?;
    assert!(matches!(
        store
            .request_counterfactual_cancel(
                fixture.request.job_id,
                counterfactual_request_id(31)?,
                fixture.request.created_at,
            )
            .await,
        Err(StoreError::InjectedFailure)
    ));
    assert_eq!(
        store
            .load_counterfactual_job(fixture.request.job_id)
            .await?
            .state,
        CounterfactualJobState::Queued
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn concurrent_v1_v2_and_v3_openers_verify_peer_completed_migrations_under_lock()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let v1_path = directory.path().join("concurrent-v1.sqlite3");
    std::fs::write(
        &v1_path,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;
    let v2_hook = V2MigrationBeginTestHook::new();
    let actor_hook = v2_hook.clone();
    let actor_path = v1_path.clone();
    let first_open = tokio::spawn(async move {
        SqliteScenarioStore::open_with_options(
            actor_path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_v2_migration_begin_test_hook(actor_hook),
        )
        .await
    });
    let wait_hook = v2_hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_before_begin()).await?;
    let (peer, peer_outcome) = SqliteScenarioStore::open(&v1_path).await?;
    assert_eq!(peer_outcome.applied_migrations, vec![2, 3, 4, 5]);
    drop(peer);
    v2_hook.release();
    let (first, first_outcome) = first_open.await??;
    assert!(first_outcome.applied_migrations.is_empty());
    drop(first);

    let v2_path = directory.path().join("concurrent-v2.sqlite3");
    std::fs::write(
        &v2_path,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;
    assert!(matches!(
        SqliteScenarioStore::open_with_options(
            &v2_path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_failpoint(Failpoint::AfterV3MigrationSql),
        )
        .await,
        Err(StoreError::InjectedFailure)
    ));
    let connection = Connection::open(&v2_path)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 2);
    drop(connection);

    let v3_hook = V3MigrationBeginTestHook::new();
    let actor_hook = v3_hook.clone();
    let actor_path = v2_path.clone();
    let first_open = tokio::spawn(async move {
        SqliteScenarioStore::open_with_options(
            actor_path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_v3_migration_begin_test_hook(actor_hook),
        )
        .await
    });
    let wait_hook = v3_hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_before_begin()).await?;
    let (peer, peer_outcome) = SqliteScenarioStore::open(&v2_path).await?;
    assert_eq!(peer_outcome.applied_migrations, vec![3, 4, 5]);
    drop(peer);
    v3_hook.release();
    let (first, first_outcome) = first_open.await??;
    assert!(first_outcome.applied_migrations.is_empty());
    drop(first);
    let connection = Connection::open(&v2_path)?;
    let versions = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")?
        .query_map([], |row| row.get::<_, u32>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(versions, vec![1, 2, 3, 4, 5]);

    let v3_path = directory.path().join("concurrent-v3.sqlite3");
    assert!(matches!(
        SqliteScenarioStore::open_with_options(
            &v3_path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_failpoint(Failpoint::AfterV4MigrationSql),
        )
        .await,
        Err(StoreError::InjectedFailure)
    ));
    let connection = Connection::open(&v3_path)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 3);
    drop(connection);

    let v4_hook = V4MigrationBeginTestHook::new();
    let actor_hook = v4_hook.clone();
    let actor_path = v3_path.clone();
    let first_open = tokio::spawn(async move {
        SqliteScenarioStore::open_with_options(
            actor_path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_v4_migration_begin_test_hook(actor_hook),
        )
        .await
    });
    let wait_hook = v4_hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_before_begin()).await?;
    let (peer, peer_outcome) = SqliteScenarioStore::open(&v3_path).await?;
    assert_eq!(peer_outcome.applied_migrations, vec![4, 5]);
    drop(peer);
    v4_hook.release();
    let (first, first_outcome) = first_open.await??;
    assert!(first_outcome.applied_migrations.is_empty());
    drop(first);
    let connection = Connection::open(&v3_path)?;
    let versions = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")?
        .query_map([], |row| row.get::<_, u32>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(versions, vec![1, 2, 3, 4, 5]);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn v4_migration_failure_rolls_back_then_v3_upgrades_cleanly() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("v3-to-v4.sqlite3");
    assert!(matches!(
        SqliteScenarioStore::open_with_options(
            &path,
            OpenOptions::new(SnapshotPolicy::default())
                .with_failpoint(Failpoint::AfterV4MigrationSql),
        )
        .await,
        Err(StoreError::InjectedFailure)
    ));
    let connection = Connection::open(&path)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let versions = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")?
        .query_map([], |row| row.get::<_, u32>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let selected_column_count: u32 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('scenarios') WHERE name = 'selected_solution_id'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, 3);
    assert_eq!(versions, vec![1, 2, 3]);
    assert_eq!(selected_column_count, 0);
    drop(connection);

    let (store, outcome) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(outcome.applied_migrations, vec![4, 5]);
    drop(store);
    let connection = Connection::open(&path)?;
    let selected_column_count: u32 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('scenarios') WHERE name = 'selected_solution_id'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(selected_column_count, 1);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn accepted_result_reads_and_selection_preserve_validated_viewing_state_after_reopen()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("accepted-result-queries.sqlite3");
    let id = scenario_id(410)?;
    let original_document = document(id)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: original_document.clone(),
        })
        .await?;

    let started_at = Rfc3339Timestamp::from_timestamp(jiff::Timestamp::now());
    let mut older_request = solve_request(id, 90, 90)?;
    older_request.started_at = started_at;
    let older_started = store.start_solve_run(older_request).await?;
    let older_result = accepted_result_for(&older_started.input, 92)?;
    let older_manifest = terminal_manifest(
        &older_started.input,
        older_started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: older_result.solution.solution_id,
            accepted_result_checksum: older_result.checksum.clone(),
            verification_checksum: older_result.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(
            older_result.clone(),
            older_manifest.clone(),
            BTreeMap::new(),
        )
        .await?;

    let current_document = set_marker(original_document.clone(), Some(1), UPDATED)?;
    let written_document = current_document.clone();
    store
        .execute_command(
            id,
            Revision::INITIAL,
            RedoBranchPolicy::Reject,
            CancellationToken::new(),
            move |_| {
                Ok(CommandWrite {
                    document: written_document,
                    journal: journal(json!({"marker": 1}), None, UPDATED)?,
                    output: (),
                })
            },
        )
        .await?;

    let current_started_at = shift_timestamp(started_at, 1)?;
    let mut first_current_request = solve_request(id, 91, 91)?;
    first_current_request.expected_revision = Revision::new(1);
    first_current_request.started_at = current_started_at;
    let first_current_started = store.start_solve_run(first_current_request).await?;
    let first_current_result = accepted_result_for(&first_current_started.input, 91)?;
    let first_current_manifest = terminal_manifest(
        &first_current_started.input,
        first_current_started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            solution_id: first_current_result.solution.solution_id,
            accepted_result_checksum: first_current_result.checksum.clone(),
            verification_checksum: first_current_result.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(
            first_current_result.clone(),
            first_current_manifest.clone(),
            BTreeMap::new(),
        )
        .await?;

    let mut second_current_request = solve_request(id, 92, 92)?;
    second_current_request.expected_revision = Revision::new(1);
    second_current_request.started_at = current_started_at;
    let second_current_started = store.start_solve_run(second_current_request).await?;
    let second_current_result = accepted_result_for(&second_current_started.input, 90)?;
    let second_current_manifest = terminal_manifest(
        &second_current_started.input,
        second_current_started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: second_current_result.solution.solution_id,
            accepted_result_checksum: second_current_result.checksum.clone(),
            verification_checksum: second_current_result.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(
            second_current_result.clone(),
            second_current_manifest.clone(),
            BTreeMap::new(),
        )
        .await?;
    let other_id = scenario_id(412)?;
    store
        .create_project(NewProject {
            document: document(other_id)?,
        })
        .await?;
    let mut other_request = solve_request(other_id, 93, 93)?;
    other_request.started_at = shift_timestamp(started_at, 2)?;
    let other_started = store.start_solve_run(other_request).await?;
    let other_result = accepted_result_for(&other_started.input, 93)?;
    let other_manifest = terminal_manifest(
        &other_started.input,
        other_started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: other_result.solution.solution_id,
            accepted_result_checksum: other_result.checksum.clone(),
            verification_checksum: other_result.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(other_result.clone(), other_manifest, BTreeMap::new())
        .await?;
    drop(store);

    let unaccepted_id =
        SolutionId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8003-000000000093")?);
    let legacy_id = SolutionId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8003-000000000094")?);
    let connection = Connection::open(&path)?;
    for (solution_id, status, accepted) in [
        (unaccepted_id, "verified", 0_i64),
        (legacy_id, "legacy_unverified", 1_i64),
    ] {
        connection.execute(
            "INSERT INTO solutions (id, solve_run_id, scenario_id, scenario_revision, status, accepted, normalized_solution_json, score_json, verification_report_json, evidence_json, created_at) VALUES (?1, ?2, ?3, 1, ?4, ?5, '{}', '{}', '{}', NULL, ?6)",
            params![
                solution_id.to_string(),
                first_current_started.input.run_id.to_string(),
                id.to_string(),
                status,
                accepted,
                first_current_manifest.finished_at.to_string(),
            ],
        )?;
    }
    drop(connection);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let summaries = reopened.list_accepted_results(id).await?;
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.result.solution_id)
            .collect::<Vec<_>>(),
        vec![
            second_current_result.solution.solution_id,
            first_current_result.solution.solution_id,
            older_result.solution.solution_id,
        ]
    );
    assert_eq!(summaries[0].current_revision, Revision::new(1));
    assert_eq!(summaries[0].scenario_revision, Revision::new(1));
    assert!(!summaries[0].stale);
    assert_eq!(summaries[0].status, SolveStatus::Feasible);
    assert_eq!(summaries[0].score, second_current_result.verification.score);
    assert_eq!(
        summaries[0].verification_checksum,
        second_current_result.verification.checksum
    );
    assert_eq!(summaries[2].scenario_revision, Revision::INITIAL);
    assert!(summaries[2].stale);

    let expected_portable = PortableAcceptedResultV2::new(
        first_current_started.input.clone(),
        first_current_manifest,
        first_current_result.clone(),
        BTreeMap::new(),
    )?;
    let loaded = reopened
        .load_accepted_result(first_current_result.solution.solution_id)
        .await?;
    assert_eq!(loaded.document, current_document);
    assert_eq!(loaded.portable, expected_portable);
    for unavailable in [
        unaccepted_id,
        legacy_id,
        SolutionId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8003-000000000095")?),
    ] {
        assert!(matches!(
            reopened.load_accepted_result(unavailable).await,
            Err(StoreError::AcceptedResultNotFound(solution_id)) if solution_id == unavailable
        ));
    }

    assert_eq!(reopened.selected_accepted_result(id).await?, None);
    let before_project = reopened.get_project(id).await?;
    let before_library_revision = reopened.library_metadata_snapshot().await?.revision;
    let selected_current = reopened
        .select_accepted_result(
            id,
            Revision::new(1),
            first_current_result.solution.solution_id,
        )
        .await?;
    assert!(selected_current.selected);
    assert!(!selected_current.stale);
    assert_eq!(
        reopened
            .select_accepted_result(
                id,
                Revision::new(1),
                first_current_result.solution.solution_id,
            )
            .await?,
        selected_current
    );
    let selected_stale = reopened
        .select_accepted_result(id, Revision::new(1), older_result.solution.solution_id)
        .await?;
    assert!(selected_stale.selected);
    assert!(selected_stale.stale);
    assert_eq!(selected_stale.current_revision, Revision::new(1));
    let summaries = reopened.list_accepted_results(id).await?;
    assert_eq!(
        summaries.iter().filter(|summary| summary.selected).count(),
        1
    );
    assert!(summaries.iter().any(|summary| summary.result.solution_id
        == older_result.solution.solution_id
        && summary.selected
        && summary.stale));
    assert_eq!(
        reopened.get_project(id).await?.summary.revision,
        before_project.summary.revision
    );
    assert_eq!(
        reopened.get_project(id).await?.document,
        before_project.document
    );
    assert_eq!(
        reopened.library_metadata_snapshot().await?.revision,
        before_library_revision
    );

    assert!(matches!(
        reopened
            .select_accepted_result(
                id,
                Revision::INITIAL,
                second_current_result.solution.solution_id,
            )
            .await,
        Err(StoreError::Conflict { expected, actual })
            if expected == Revision::INITIAL && actual == Revision::new(1)
    ));
    for unavailable in [
        unaccepted_id,
        legacy_id,
        SolutionId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8003-000000000095")?),
        other_result.solution.solution_id,
    ] {
        assert!(matches!(
            reopened
                .select_accepted_result(id, Revision::new(1), unavailable)
                .await,
            Err(StoreError::AcceptedResultNotFound(solution_id)) if solution_id == unavailable
        ));
    }
    assert_eq!(
        reopened
            .selected_accepted_result(id)
            .await?
            .ok_or("selected result disappeared after rejected selection")?
            .result
            .solution_id,
        older_result.solution.solution_id
    );
    drop(reopened);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(
        reopened
            .selected_accepted_result(id)
            .await?
            .ok_or("selected result did not survive restart")?
            .result
            .solution_id,
        older_result.solution.solution_id
    );
    drop(reopened);
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    assert_eq!(
        connection.execute(
            "DELETE FROM solutions WHERE id = ?1",
            [older_result.solution.solution_id.to_string()],
        )?,
        1
    );
    drop(connection);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(reopened.selected_accepted_result(id).await?, None);
    assert_eq!(
        reopened.library_metadata_snapshot().await?.revision,
        before_library_revision
    );
    Ok(())
}

#[tokio::test]
async fn persisted_selection_rejects_unaccepted_and_legacy_solution_rows()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("invalid-selected-authority.sqlite3");
    let id = scenario_id(413)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let started = store.start_solve_run(solve_request(id, 97, 97)?).await?;
    drop(store);

    let unaccepted_id =
        SolutionId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8003-000000000097")?);
    let legacy_id = SolutionId::from_uuid(Uuid::parse_str("018f47f2-e880-7000-8003-000000000098")?);
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    for (solution_id, status, accepted) in [
        (unaccepted_id, "verified", 0_i64),
        (legacy_id, "legacy_unverified", 1_i64),
    ] {
        connection.execute(
            "INSERT INTO solutions (id, solve_run_id, scenario_id, scenario_revision, status, accepted, normalized_solution_json, score_json, verification_report_json, evidence_json, created_at) VALUES (?1, ?2, ?3, 0, ?4, ?5, '{}', '{}', '{}', NULL, ?6)",
            params![
                solution_id.to_string(),
                started.input.run_id.to_string(),
                id.to_string(),
                status,
                accepted,
                CREATED,
            ],
        )?;
    }
    drop(connection);

    for selected_id in [unaccepted_id, legacy_id] {
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute(
            "UPDATE scenarios SET selected_solution_id = ?2 WHERE id = ?1",
            params![id.to_string(), selected_id.to_string()],
        )?;
        drop(connection);

        let (store, _) = SqliteScenarioStore::open(&path).await?;
        assert!(matches!(
            store.selected_accepted_result(id).await,
            Err(StoreError::InvalidPersistedResult(_))
        ));
        assert!(matches!(
            store.list_accepted_results(id).await,
            Err(StoreError::InvalidPersistedResult(_))
        ));
        drop(store);
    }
    Ok(())
}

#[tokio::test]
async fn accepted_result_queries_reject_tampered_local_authority() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("tampered-accepted-result.sqlite3");
    let id = scenario_id(411)?;
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    store
        .create_project(NewProject {
            document: document(id)?,
        })
        .await?;
    let started = store.start_solve_run(solve_request(id, 96, 96)?).await?;
    let accepted = accepted_result_for(&started.input, 96)?;
    let manifest = terminal_manifest(
        &started.input,
        started.started_at,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
    )?;
    store
        .finalize_accepted_run(accepted.clone(), manifest, BTreeMap::new())
        .await?;
    drop(store);

    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE solve_runs SET elapsed_ms = elapsed_ms + 1 WHERE id = ?1",
        [started.input.run_id.to_string()],
    )?;
    drop(connection);

    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert!(matches!(
        reopened
            .load_accepted_result(accepted.solution.solution_id)
            .await,
        Err(StoreError::InvalidPersistedResult(_))
    ));
    assert!(matches!(
        reopened.list_accepted_results(id).await,
        Err(StoreError::InvalidPersistedResult(_))
    ));
    Ok(())
}
