#[path = "../../../tests/support/portable_decode.rs"]
mod portable_decode;
#[path = "../../../tests/support/portable_encode.rs"]
mod portable_encode;
#[path = "support/setup_control.rs"]
mod setup_control;

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_command::official_registry;
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    BackupAssetSelection, BackupSelection, COUNTERFACTUAL_API_SCHEMA_VERSION, DeferredCapability,
    EuthetoApp, EventSubscription, ManualSelectionAdmissionRequestV1, ProjectScope,
    SOLUTION_ADOPT_BASE_SCHEMA_VERSION, SOLUTION_API_SCHEMA_VERSION,
    SOLUTION_COMPARE_SCHEMA_VERSION, SafetyBackupOutcome, SolutionAdoptBaseRequestV1,
    SolutionCancelCounterfactualDtoV1, SolutionCancelCounterfactualRequestV1,
    SolutionCompareRequestV3, SolutionExplainRequestV1, SolutionExplanationDtoV2,
    SolutionGetCounterfactualRequestV1, SolutionListRequestV1, SolutionSelectRequestV1,
    SolutionStartCounterfactualDtoV1, SolutionStartCounterfactualRequestV1,
    SolutionSummaryRequestV1, SolutionVerifyRequestV1, SolutionViewRequestV1,
};
use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, AssignmentValue,
    COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION, CounterfactualConditionPayloadV1,
    CounterfactualConditionV1, CounterfactualFailureKind, CounterfactualJobRecordV1,
    CounterfactualJobRequestV1, CounterfactualJobState, CounterfactualRequestSemanticsV1,
    CounterfactualResult, DomainAssignmentId, ExplanationEvidencePayloadV2, ExplanationKind,
    ExplanationRequestSubjectV1, ExplanationRequestV1, PortableAcceptedResult, RunManifest,
    RunPhaseTimingsV1, RunTerminalOutcomeV1, ScoreComparisonUnavailableReasonV1, ScoreComparisonV2,
    VerificationContextV1, VerificationIssueId, VerificationReport, VerificationValue, blake3_hex,
};
use eutheto_export::{
    BackupSections, CHECKSUMS_PATH, CURRENT_PORTABLE_SCHEMA_VERSION, Checksums, FullBackupSnapshot,
    MANIFEST_PATH, ScenarioExportSnapshot, assemble_full_backup, assemble_scenario_export,
    backup_selection_from_manifest, parse_omitted_asset_placeholder,
};
use eutheto_import::{
    CollisionAction, CollisionPlan, ImportOptions, ImportProvenance, InspectedBundle,
    InspectionPolicy, MigrationRegistries, PreviewBinding, RestoreAuthorization, RestoreMode,
    SafetyBackupEvidence, StagedDisposition, StagedImport, StagedScenario, inspect_bundle,
};
use eutheto_planning_ir::{
    CandidateValues, PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningProblemSummary,
    ProjectionExpression, Variable, canonical_ir_hash,
};
use eutheto_solver_api::*;
#[cfg(debug_assertions)]
use eutheto_store::{
    CommandCommitTestHook, CommandCommitTestPhase, Failpoint, OpenOptions, SnapshotPolicy,
};
use eutheto_store::{
    NewProject, NewSolveRunV1, SqliteScenarioStore, StagedLibraryApply, StoredAcceptedResultV2,
};
use eutheto_types::{
    ActorRef, AddEntity, AppError, BackendId, BackendSelection, Clock, CommandBatch,
    CommandEnvelope, CommandId, CommandSource, CounterfactualJobId, DirectoryAvailabilityLabel,
    DomainCommandEnvelope, DomainPackRef, DurationMillis, EntityId, EventPayload, EventTopic,
    ExplanationMode, FixedClock, FixedIdGenerator, GapPolicy, Horizon, IanaTimeZone,
    IdGenerationError, IdGenerator, LocaleTag, OperationId, OverlapPolicy,
    PORTABLE_LARGE_ASSET_BYTES_V1, PortableAsset, PortableProjectMetadata, PreservationPolicy,
    ReproducibilityMode, RequestId, ResourceLimits, Revision, Rfc3339Timestamp,
    SCENARIO_FORMAT_VERSION, SUPPORT_PREVIEW_SCHEMA_VERSION, ScenarioCommand, ScenarioDocument,
    ScenarioId, ScenarioSettings, SemanticCapability, SolutionId, SolveMode, SolveOptions,
    SolveRunId, SolveStatus, SupportPreviewDto, SystemClock, SystemIdGenerator, UnitSystem,
    WorkerThreadPolicy, collect_scenario_owned_uuids,
};
#[cfg(debug_assertions)]
use rusqlite::Connection;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::future::{Future, poll_fn};
use std::io::{Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::Poll;
use tempfile::TempDir;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};
trait BoxedResult<T> {
    fn boxed(self) -> Result<T, Box<dyn Error>>;
}

impl<T> BoxedResult<T> for Result<T, AppError> {
    fn boxed(self) -> Result<T, Box<dyn Error>> {
        self.map_err(|error| std::io::Error::other(format!("{error:?}")).into())
    }
}

impl<T> BoxedResult<T> for Result<T, Box<dyn Error>> {
    fn boxed(self) -> Result<T, Box<dyn Error>> {
        self
    }
}
fn private_tempdir() -> Result<TempDir, Box<dyn Error>> {
    let base = dirs::home_dir().ok_or("platform home directory is unavailable")?;
    std::fs::create_dir_all(&base)?;
    Ok(tempfile::Builder::new()
        .prefix("eutheto-core-test-")
        .tempdir_in(base)?)
}

struct CancellingIdGenerator {
    cancellation: eutheto_types::CancellationToken,
    id: Uuid,
}

impl IdGenerator for CancellingIdGenerator {
    fn next_uuid(&self) -> Result<Uuid, IdGenerationError> {
        self.cancellation.cancel();
        Ok(self.id)
    }
}

struct BlockingFirstClock {
    now: Rfc3339Timestamp,
    armed: AtomicBool,
    calls_after_arm: AtomicUsize,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl BlockingFirstClock {
    fn new(now: Rfc3339Timestamp) -> Self {
        Self {
            now,
            armed: AtomicBool::new(false),
            calls_after_arm: AtomicUsize::new(0),
            entered: Arc::new(Barrier::new(2)),
            release: Arc::new(Barrier::new(2)),
        }
    }

    fn arm(&self) {
        self.calls_after_arm.store(0, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    fn calls_after_arm(&self) -> usize {
        self.calls_after_arm.load(Ordering::SeqCst)
    }
}

impl Clock for BlockingFirstClock {
    fn now(&self) -> Rfc3339Timestamp {
        if self.armed.load(Ordering::SeqCst)
            && self.calls_after_arm.fetch_add(1, Ordering::SeqCst) == 0
        {
            self.entered.wait();
            self.release.wait();
        }
        self.now
    }
}

fn timestamp(value: &str) -> Result<Rfc3339Timestamp, Box<dyn Error>> {
    Ok(value.parse()?)
}

fn request_id() -> Result<RequestId, Box<dyn Error>> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}

fn settings() -> Result<ScenarioSettings, Box<dyn Error>> {
    Ok(ScenarioSettings {
        time_zone: "UTC".parse::<IanaTimeZone>()?,
        locale: "en-US".parse::<LocaleTag>()?,
        units: UnitSystem::Metric,
        horizon: Horizon::new(
            timestamp("2026-01-01T00:00:00Z")?,
            timestamp("2026-02-01T00:00:00Z")?,
        )?,
        gap_policy: GapPolicy::Reject,
        overlap_policy: OverlapPolicy::Earlier,
    })
}

fn all_fixed_exclusions() -> BTreeSet<eutheto_export::FixedExclusion> {
    eutheto_export::FixedExclusion::ALL.into_iter().collect()
}

fn complete_full_backup_extensions() -> Result<BTreeMap<String, serde_json::Value>, Box<dyn Error>>
{
    let selection = eutheto_export::BackupSelection {
        include_results: true,
        fixed_exclusions: all_fixed_exclusions(),
        asset_selection: eutheto_export::PortableBackupAssetSelection::All,
        threshold_version: None,
        threshold_bytes: None,
        excluded_asset_count: 0,
        excluded_asset_ids: BTreeSet::new(),
        scope: eutheto_export::BackupSelectionScope::Library,
    };
    Ok(BTreeMap::from([(
        eutheto_export::BACKUP_SELECTION_EXTENSION.to_owned(),
        eutheto_export::backup_selection_extension_value(&selection)?,
    )]))
}
fn dependencies(directory: &TempDir) -> Result<AppDependencies, Box<dyn Error>> {
    dependencies_at(directory, "2026-01-10T12:00:00Z")
}

fn dependencies_at(directory: &TempDir, now: &str) -> Result<AppDependencies, Box<dyn Error>> {
    Ok(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("eutheto.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new(timestamp(now)?)),
        monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: eutheto_types::CancellationToken::default(),
    })
}

fn dependencies_with_fixed_ids(
    directory: &TempDir,
    ids: impl IntoIterator<Item = Uuid>,
) -> Result<AppDependencies, Box<dyn Error>> {
    let mut dependencies = dependencies(directory)?;
    dependencies.ids = Arc::new(FixedIdGenerator::new(ids));
    Ok(dependencies)
}
fn with_newer_unknown_semantics(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        entries.insert(entry.name()?.into_owned(), content);
    }
    let mut manifest: serde_json::Value = serde_json::from_slice(
        entries
            .get(MANIFEST_PATH)
            .ok_or("fixture manifest is missing")?,
    )?;
    let object = manifest
        .as_object_mut()
        .ok_or("fixture manifest is not an object")?;
    object.insert(
        "schemaVersion".to_owned(),
        serde_json::Value::from(CURRENT_PORTABLE_SCHEMA_VERSION.saturating_add(1)),
    );
    object.insert(
        "requiredCapabilities".to_owned(),
        json!([{ "id": "future.semantic", "version": 1 }]),
    );
    let manifest_bytes = eutheto_export::canonical_json(&manifest)?;
    entries.insert(MANIFEST_PATH.to_owned(), manifest_bytes.clone());
    let mut checksums: Checksums = serde_json::from_slice(
        entries
            .get(CHECKSUMS_PATH)
            .ok_or("fixture checksums are missing")?,
    )?;
    checksums.files.insert(
        MANIFEST_PATH.to_owned(),
        eutheto_export::sha256_hex(&manifest_bytes),
    );
    entries.insert(
        CHECKSUMS_PATH.to_owned(),
        eutheto_export::canonical_json(&checksums)?,
    );

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    for (path, content) in entries {
        writer.start_file(path, options)?;
        writer.write_all(&content)?;
    }
    Ok(writer.finish()?.into_inner())
}

fn with_unknown_pack_and_invalid_domain(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        entries.insert(entry.name()?.into_owned(), content);
    }
    let scenario_path = entries
        .keys()
        .find(|path| path.starts_with("scenarios/"))
        .cloned()
        .ok_or("fixture scenario is missing")?;
    let mut scenario: serde_json::Value = serde_json::from_slice(
        entries
            .get(&scenario_path)
            .ok_or("fixture scenario is missing")?,
    )?;
    scenario["domain"]["packId"] = json!("vendor.unknown");
    scenario["domain"]["payload"] = json!("__INVALID_DOMAIN__");
    let encoded = String::from_utf8(eutheto_export::canonical_json(&scenario)?)?
        .replace(
            "\"payload\":\"__INVALID_DOMAIN__\"",
            "\"payload\":{\"entities\":[],\"entities\":{}}",
        )
        .into_bytes();
    assert!(String::from_utf8_lossy(&encoded).contains("\"entities\":[],\"entities\":{}"));
    entries.insert(scenario_path, encoded);
    let files = entries
        .iter()
        .filter(|(path, _)| path.as_str() != CHECKSUMS_PATH)
        .map(|(path, content)| (path.clone(), eutheto_export::sha256_hex(content)))
        .collect();
    entries.insert(
        CHECKSUMS_PATH.to_owned(),
        eutheto_export::canonical_json(&Checksums {
            algorithm: eutheto_export::CHECKSUM_ALGORITHM.to_owned(),
            files,
        })?,
    );
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    for (path, content) in entries {
        writer.start_file(path, options)?;
        writer.write_all(&content)?;
    }
    Ok(writer.finish()?.into_inner())
}

async fn create_project(app: &EuthetoApp, title: &str) -> Result<ScenarioId, Box<dyn Error>> {
    let result = app
        .execute(AppCommand::CreateProject {
            request_id: request_id()?,
            title: title.to_owned(),
            description: "integration project".to_owned(),
            domain_pack: DomainPackRef {
                id: "official.test".parse()?,
                schema_version: 1,
            },
            settings: settings()?,
        })
        .await
        .boxed()?;
    match result {
        AppCommandResult::Project(project) => Ok(project.scenario_id),
        other => Err(format!("unexpected create result: {other:?}").into()),
    }
}

fn add_entity_envelope(
    scenario_id: ScenarioId,
    expected_revision: Revision,
    _name: &str,
) -> Result<CommandEnvelope, Box<dyn Error>> {
    let ids = SystemIdGenerator;
    let person_id = EntityId::new(&ids)?;
    Ok(CommandEnvelope {
        command_id: CommandId::new(&ids)?,
        scenario_id,
        expected_revision,
        actor: ActorRef {
            actor_id: Some("test.actor".to_owned()),
            display_name: "Integration Test".to_owned(),
        },
        source: CommandSource::System,
        command: ScenarioCommand::AddEntity(AddEntity {
            entity_id: person_id,
            value: json!({
                "id": person_id.to_string(),
                "enabled": false,
                "target": 0
            }),
        }),
    })
}

fn invalid_add_entity_envelope(
    scenario_id: ScenarioId,
    expected_revision: Revision,
) -> Result<CommandEnvelope, Box<dyn Error>> {
    let ids = SystemIdGenerator;
    let entity_id = EntityId::new(&ids)?;
    Ok(CommandEnvelope {
        command_id: CommandId::new(&ids)?,
        scenario_id,
        expected_revision,
        actor: ActorRef {
            actor_id: Some("test.actor".to_owned()),
            display_name: "Integration Test".to_owned(),
        },
        source: CommandSource::System,
        command: ScenarioCommand::AddEntity(AddEntity {
            entity_id,
            value: json!({
                "id": entity_id.to_string(),
                "enabled": false,
                "target": 1
            }),
        }),
    })
}

async fn scenario_view(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
) -> Result<eutheto_types::ScenarioViewDto, Box<dyn Error>> {
    match app
        .query(AppQuery::ScenarioView(scenario_id))
        .await
        .boxed()?
    {
        AppQueryResult::Scenario(view) => Ok(*view),
        other => Err(format!("unexpected scenario result: {other:?}").into()),
    }
}

fn solution_test_id(prefix: u16, suffix: u16) -> Result<Uuid, Box<dyn Error>> {
    Ok(Uuid::parse_str(&format!(
        "018f47f2-e880-7000-{prefix:04x}-{suffix:012x}"
    ))?)
}
fn workforce_adoption_document() -> Result<ScenarioDocument, Box<dyn Error>> {
    let mut value = serde_json::to_value(workforce_fixture::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["lockedAssignments"] = json!({});
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("missing Workforce fixture entities")?
        .remove(&workforce_fixture::id(6));
    let mut alternate_person = value["domain"]["entities"][workforce_fixture::id(1)].clone();
    alternate_person["id"] = json!(workforce_fixture::id(12));
    alternate_person["name"] = json!("Another clinician");
    alternate_person["externalId"] = json!("staff-02");
    value["domain"]["entities"][workforce_fixture::id(12)] = alternate_person;
    let shift = &mut value["domain"]["entities"][workforce_fixture::id(8)];
    shift["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z","local":"2026-11-01T08:00:00","offsetSeconds":0
    });
    shift["endsAt"] = json!({
        "instant":"2026-11-01T10:00:00Z","local":"2026-11-01T10:00:00","offsetSeconds":0
    });
    value["domain"]["rules"] = json!({});
    for (index, kind) in [
        (30, "eligibility"),
        (31, "availability"),
        (32, "coverage"),
        (33, "noOverlap"),
    ] {
        let mut rule = json!({
            "id":workforce_fixture::id(index),"kind":kind,"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}}
        });
        if kind == "noOverlap" {
            rule["compatibleCategoryPairs"] = json!([]);
        }
        value["domain"]["rules"][workforce_fixture::id(index)] = rule;
    }
    Ok(serde_json::from_value(value)?)
}

fn solution_test_options() -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend: BackendSelection::Specific(BackendId::new("synthetic.test")?),
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(5_000)?,
        memory_limit_bytes: None,
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
struct CounterfactualTestBackend {
    descriptor: SolverDescriptor,
    runtime_identity: BackendRuntimeIdentity,
    matrix: CapabilityMatrix,
    wait_for_cancel: bool,
    invocations: AtomicUsize,
    entered: AtomicBool,
    observed_cancel: AtomicBool,
}

impl SolverBackend for CounterfactualTestBackend {
    fn completion_reserve(&self) -> DurationMillis {
        DurationMillis::ZERO
    }

    fn descriptor(&self) -> &SolverDescriptor {
        &self.descriptor
    }

    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        &self.runtime_identity
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
        output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(async move {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            self.entered.store(true, Ordering::SeqCst);
            if self.wait_for_cancel {
                while !request.dispatch_budget().child_view().is_cancelled() {
                    tokio::task::yield_now().await;
                }
                self.observed_cancel.store(true, Ordering::SeqCst);
                return counterfactual_backend_outcome(
                    request,
                    &self.runtime_identity,
                    BackendTerminationReason::Cancelled,
                );
            }

            let mut values = CandidateValues::default();
            for variable in &request.problem().variables {
                match variable {
                    Variable::Boolean(variable) => {
                        values.booleans.insert(variable.id.clone(), false);
                    }
                    Variable::Integer(variable) => {
                        values.integers.insert(variable.id.clone(), 0);
                    }
                    Variable::Interval(_) => {
                        return Err(BackendError::new(
                            "tests.counterfactual.unexpected_interval",
                            "The official counterfactual fixture unexpectedly contained an interval.",
                        )?);
                    }
                }
            }
            output.submit_candidate(CandidateSubmission {
                values,
                observed_after_milliseconds: DurationMillis::ZERO,
                objective: None,
                evidence_refs: Vec::new(),
            })?;
            counterfactual_backend_outcome(
                request,
                &self.runtime_identity,
                BackendTerminationReason::CandidateFound,
            )
        })
    }
}

fn counterfactual_backend_outcome(
    request: &SolveRequest,
    identity: &BackendRuntimeIdentity,
    termination: BackendTerminationReason,
) -> Result<BackendSolveOutcome, BackendError> {
    let summary = request.summary();
    let candidate_found = termination == BackendTerminationReason::CandidateFound;
    let random_seed = i32::try_from(request.options().random_seed)
        .map_err(|_| BackendError::from(OutputError::UnsafeDiagnosticLine))?;
    Ok(BackendSolveOutcome {
        backend_id: request.backend_id().clone(),
        model_hash: request.model_hash().to_owned(),
        solve_fingerprint: request.solve_fingerprint().to_owned(),
        termination,
        evidence: BackendTerminationEvidence {
            remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
            backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
            elapsed_milliseconds: DurationMillis::ZERO,
            first_incumbent_milliseconds: candidate_found.then_some(DurationMillis::ZERO),
            objective: None,
            best_bound_values: None,
            assumption_core: None,
            evidence_refs: Vec::new(),
            execution: Some(BackendExecutionEvidence {
                timings: BackendTimingEvidence {
                    translation_serialization_milliseconds: DurationMillis::ZERO,
                    worker_startup_milliseconds: None,
                    handshake_milliseconds: None,
                    solver_milliseconds: Some(DurationMillis::ZERO),
                    protocol_decode_milliseconds: None,
                },
                model_counts: BackendModelCountEvidence {
                    planning_variable_count: summary.variable_count,
                    planning_constraint_count: summary.constraint_count,
                    translated_variable_count: summary.variable_count,
                    translated_constraint_count: summary.constraint_count,
                },
                worker_statistics: None,
                reproducibility: BackendReproducibilityEvidence {
                    backend_version: identity.backend_version().to_owned(),
                    adapter_version: identity.adapter_version().to_owned(),
                    worker_version: identity.worker_version().to_owned(),
                    engine_version: identity.solver_version().to_owned(),
                    protocol_major: identity.protocol_major(),
                    protocol_minor: identity.protocol_minor(),
                    applied_options: request.options().clone(),
                    applied_parameters: BackendAppliedParameterEvidence {
                        wall_time_milliseconds: Some(request.dispatch_budget().backend_limit()),
                        memory_limit_bytes: request.options().memory_limit_bytes,
                        worker_threads: 1,
                        random_seed,
                        stop_after_first_feasible: request.options().stop_after_first_feasible,
                        emit_intermediate_solutions: request
                            .options()
                            .collect_intermediate_solutions,
                        log_search_progress: false,
                        deterministic_test_profile: true,
                    },
                    model_fingerprint_sha256: "a".repeat(64),
                    applied_parameters_sha256: None,
                },
            }),
        },
    })
}

fn counterfactual_solver_registry(
    wait_for_cancel: bool,
) -> Result<(SolverRegistry, Arc<CounterfactualTestBackend>), Box<dyn Error>> {
    let backend_id = BackendId::new("synthetic.test")?;
    let features = SUPPORT_FEATURES
        .iter()
        .map(|(id, category, gate)| {
            Ok(SupportFeature {
                id: SupportFeatureId::new(*id)?,
                category: match *category {
                    "primitive" => SupportFeatureCategory::Primitive,
                    "objective" => SupportFeatureCategory::Objective,
                    "projection" => SupportFeatureCategory::Projection,
                    "solve" => SupportFeatureCategory::Solve,
                    value => {
                        return Err(SupportMatrixError::UnknownGeneratedCategory(
                            value.to_owned(),
                        ));
                    }
                },
                gate: if *gate == "unconditional" {
                    SupportFeatureGate::Unconditional
                } else {
                    SupportFeatureGate::Enabled((*gate).to_owned())
                },
            })
        })
        .collect::<Result<Vec<_>, SupportMatrixError>>()?;
    let supported = features.iter().map(|feature| feature.id.clone()).collect();
    let cells = features
        .iter()
        .map(|feature| {
            (
                feature.id.clone(),
                SupportCell::Supported {
                    fixture_id: format!(
                        "tests.counterfactual.{}",
                        feature.id.as_str().replace('.', "-")
                    ),
                },
            )
        })
        .collect();
    let descriptor = SolverDescriptor {
        id: backend_id.clone(),
        display_name: "Counterfactual runtime test backend".to_owned(),
        version: "1.0.0".to_owned(),
        adapter_version: "1.0.0".to_owned(),
        distribution: SolverDistribution::BuiltIn,
        license: LicenseMetadata {
            spdx_expression: "Apache-2.0".to_owned(),
            license_name: "Apache License 2.0".to_owned(),
            source_url: None,
        },
        stability: BackendStability::Experimental,
        capabilities: SolverCapabilities {
            supported,
            degraded: BTreeSet::new(),
        },
    };
    let matrix = CapabilityMatrix::new(
        SUPPORT_MATRIX_SCHEMA_VERSION,
        SUPPORT_MATRIX_IR_SCHEMA_VERSION,
        features,
        vec![BackendSupportColumn {
            backend_id,
            backend_version: descriptor.version.clone(),
            adapter_version: descriptor.adapter_version.clone(),
            cells,
        }],
        Vec::new(),
    )?;
    let runtime_identity = BackendRuntimeIdentity::new(
        descriptor.id.clone(),
        descriptor.version.clone(),
        descriptor.adapter_version.clone(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        1,
        0,
    )?;
    let backend = Arc::new(CounterfactualTestBackend {
        descriptor,
        runtime_identity,
        matrix: matrix.clone(),
        wait_for_cancel,
        invocations: AtomicUsize::new(0),
        entered: AtomicBool::new(false),
        observed_cancel: AtomicBool::new(false),
    });
    let registered: Vec<Arc<dyn SolverBackend>> = vec![backend.clone()];
    Ok((SolverRegistry::new(matrix, registered)?, backend))
}

async fn start_solve_with_capture(
    store: &SqliteScenarioStore,
    document: ScenarioDocument,
    request: NewSolveRunV1,
) -> Result<eutheto_store::StartedSolveRunV1, Box<dyn Error>> {
    let control =
        eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new());
    let (capture, _) = store
        .capture_solve_snapshot(
            request.scenario_id,
            request.expected_revision,
            document,
            request.started_at,
            None,
            control.clone(),
            None,
        )
        .await?;
    store
        .start_solve_run(request, capture.stamp, Some(capture.snapshot_id), control)
        .await
        .map_err(Into::into)
}
#[allow(clippy::too_many_lines)]
async fn persist_application_accepted_result_with_status(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    revision: Revision,
    suffix: u16,
    objective_policy: &[u8],
    status: SolveStatus,
) -> Result<StoredAcceptedResultV2, Box<dyn Error>> {
    let registry = official_registry()?;
    let pack_id = "official.test".parse()?;
    let pack = registry.require(&pack_id)?;
    let project = store.get_project(scenario_id).await?;
    let compile_context = CompileContext {
        objective_context: None,
        scenario_revision: revision.value(),
        semantic_metadata: BTreeMap::new(),
        control: eutheto_types::OperationControl::Cancellation(
            eutheto_types::CancellationToken::new(),
        ),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    };
    let problem = pack.compile(&project.document, &compile_context)?;
    let model_hash = canonical_ir_hash(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let started_at = SystemClock.now();
    let finished_at = Rfc3339Timestamp::from_timestamp(
        started_at
            .as_timestamp()
            .checked_add(std::time::Duration::from_secs(1))?,
    );
    let started = start_solve_with_capture(
        store,
        project.document.clone(),
        NewSolveRunV1 {
            objective_context: None,
            run_id: SolveRunId::from_uuid(solution_test_id(0x8100, suffix)?),
            request_id: RequestId::from_uuid(solution_test_id(0x8200, suffix)?),
            scenario_id,
            expected_revision: revision,
            planning_ir_schema_version: PLANNING_IR_SCHEMA_VERSION,
            compiler_version: problem.metadata.compiler_version.clone(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            backend_id: BackendId::new("synthetic.test")?,
            backend_version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            worker_version: "1.0.0".to_owned(),
            solver_version: "1.0.0".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            model_hash: model_hash.clone(),
            objective_policy_hash: blake3_hex(objective_policy),
            solve_options: solution_test_options()?,
            temporary_condition_hash: None,
            started_at,
        },
    )
    .await?;
    let mut candidate = CandidateValues::default();
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(variable) => {
                candidate.booleans.insert(variable.id.clone(), false);
            }
            Variable::Integer(variable) => {
                candidate.integers.insert(variable.id.clone(), 0);
            }
            Variable::Interval(_) => {}
        }
    }
    let solution_id = SolutionId::from_uuid(solution_test_id(0x8300, suffix)?);
    let solution = pack.project(
        &problem,
        &candidate,
        solution_id,
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let scope = pack.verification_scope(
        &project.document,
        revision.value(),
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let context = VerificationContextV1::new(
        scenario_id,
        revision.value(),
        started.input.snapshot_document_hash.clone(),
        model_hash,
        solution.canonical_hash()?,
        scope.checksum,
    )?;
    let authoritative_score = pack.score(
        &project.document,
        &solution,
        started.input.objective_context.as_ref(),
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let verification = pack.verify(
        &project.document,
        &solution,
        started.input.objective_context.as_ref(),
        &context,
        &authoritative_score,
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let accepted = AcceptedResult::new(solution, verification)?;
    let manifest = RunManifest::new(
        started.input.run_id,
        started.input.checksum,
        RunTerminalOutcomeV1::Accepted {
            status,
            solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        started.started_at,
        finished_at,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(100)?),
        Some(DurationMillis::new(500)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let evidence = accepted
        .solution
        .assignments
        .iter()
        .flat_map(|assignment| assignment.evidence.iter())
        .chain(
            accepted
                .verification
                .required_rule_results
                .iter()
                .flat_map(|rule| rule.evidence.iter()),
        )
        .cloned()
        .map(|id| (id, VerificationValue::Boolean(true)))
        .collect();
    store
        .finalize_accepted_run(accepted, manifest, evidence)
        .await?;
    Ok(store.load_accepted_result(solution_id).await?)
}
async fn persist_application_accepted_result(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    revision: Revision,
    suffix: u16,
    objective_policy: &[u8],
) -> Result<StoredAcceptedResultV2, Box<dyn Error>> {
    persist_application_accepted_result_with_status(
        store,
        scenario_id,
        revision,
        suffix,
        objective_policy,
        SolveStatus::Optimal,
    )
    .await
}

async fn persist_application_feasible_result(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    revision: Revision,
    suffix: u16,
    objective_policy: &[u8],
) -> Result<StoredAcceptedResultV2, Box<dyn Error>> {
    persist_application_accepted_result_with_status(
        store,
        scenario_id,
        revision,
        suffix,
        objective_policy,
        SolveStatus::Feasible,
    )
    .await
}
async fn persist_application_no_result_run(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    revision: Revision,
    suffix: u16,
    status: SolveStatus,
) -> Result<RunManifest, Box<dyn Error>> {
    let registry = official_registry()?;
    let pack_id = "official.test".parse()?;
    let pack = registry.require(&pack_id)?;
    let project = store.get_project(scenario_id).await?;
    let problem = pack.compile(
        &project.document,
        &CompileContext {
            objective_context: None,
            scenario_revision: revision.value(),
            semantic_metadata: BTreeMap::new(),
            control: eutheto_types::OperationControl::Cancellation(
                eutheto_types::CancellationToken::new(),
            ),
            planning_limits: PlanningIrLimitsV1::DEFAULT,
        },
    )?;
    let started_at = SystemClock.now();
    let finished_at = Rfc3339Timestamp::from_timestamp(
        started_at
            .as_timestamp()
            .checked_add(std::time::Duration::from_secs(1))?,
    );
    let started = start_solve_with_capture(
        store,
        project.document.clone(),
        NewSolveRunV1 {
            objective_context: None,
            run_id: SolveRunId::from_uuid(solution_test_id(0x8500, suffix)?),
            request_id: RequestId::from_uuid(solution_test_id(0x8600, suffix)?),
            scenario_id,
            expected_revision: revision,
            planning_ir_schema_version: PLANNING_IR_SCHEMA_VERSION,
            compiler_version: problem.metadata.compiler_version.clone(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            backend_id: BackendId::new("synthetic.test")?,
            backend_version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            worker_version: "1.0.0".to_owned(),
            solver_version: "1.0.0".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            model_hash: canonical_ir_hash(&problem, PlanningIrLimitsV1::DEFAULT)?,
            objective_policy_hash: blake3_hex(b"status-objective-policy"),
            solve_options: solution_test_options()?,
            temporary_condition_hash: None,
            started_at,
        },
    )
    .await?;
    let manifest = RunManifest::new(
        started.input.run_id,
        started.input.checksum.clone(),
        RunTerminalOutcomeV1::NoResult { status },
        started.started_at,
        finished_at,
        Some(DurationMillis::new(1_000)?),
        None,
        None,
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    store.finalize_terminal_run(manifest.clone(), None).await?;
    Ok(manifest)
}

// Keep the actual compile, projection, independent verification and store finalization together.
#[allow(clippy::too_many_lines)]
async fn persist_workforce_accepted_result(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    revision: Revision,
    suffix: u16,
) -> Result<StoredAcceptedResultV2, Box<dyn Error>> {
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(eutheto_workforce::WorkforcePack)
        .build()?;
    let pack_id = "official.workforce".parse()?;
    let pack = registry.require(&pack_id)?;
    let project = store.get_project(scenario_id).await?;
    let compile_context = CompileContext {
        objective_context: None,
        scenario_revision: revision.value(),
        semantic_metadata: BTreeMap::new(),
        control: eutheto_types::OperationControl::Cancellation(
            eutheto_types::CancellationToken::new(),
        ),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    };
    let problem = pack.compile(&project.document, &compile_context)?;
    let mut candidate = CandidateValues::default();
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(variable) => {
                candidate.booleans.insert(variable.id.clone(), false);
            }
            Variable::Integer(variable) => {
                candidate.integers.insert(variable.id.clone(), 0);
            }
            Variable::Interval(_) => {}
        }
    }
    let selected_pair = format!(
        "{}.{}",
        workforce_fixture::id(if suffix == 2 { 12 } else { 1 }),
        workforce_fixture::id(8)
    );
    let mut selected = false;
    for projection in &problem.projections {
        if projection.entity.id.as_str() != selected_pair {
            continue;
        }
        let ProjectionExpression::Boolean(variable) = &projection.expression else {
            return Err("Workforce fixture assignment projection is not Boolean".into());
        };
        candidate.booleans.insert(variable.clone(), true);
        selected = true;
    }
    if !selected {
        return Err("Workforce fixture assignment projection is missing".into());
    }
    let model_hash = canonical_ir_hash(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let started_at = SystemClock.now();
    let finished_at = Rfc3339Timestamp::from_timestamp(
        started_at
            .as_timestamp()
            .checked_add(std::time::Duration::from_secs(1))?,
    );
    let started = start_solve_with_capture(
        store,
        project.document.clone(),
        NewSolveRunV1 {
            objective_context: None,
            run_id: SolveRunId::from_uuid(solution_test_id(0x9100, suffix)?),
            request_id: RequestId::from_uuid(solution_test_id(0x9200, suffix)?),
            scenario_id,
            expected_revision: revision,
            planning_ir_schema_version: PLANNING_IR_SCHEMA_VERSION,
            compiler_version: problem.metadata.compiler_version.clone(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            backend_id: BackendId::new("synthetic.test")?,
            backend_version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            worker_version: "1.0.0".to_owned(),
            solver_version: "1.0.0".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            model_hash: model_hash.clone(),
            objective_policy_hash: blake3_hex(b"workforce-adoption-objective-policy"),
            solve_options: solution_test_options()?,
            temporary_condition_hash: None,
            started_at,
        },
    )
    .await?;
    let solution_id = SolutionId::from_uuid(solution_test_id(0x9300, suffix)?);
    let solution = pack.project(
        &problem,
        &candidate,
        solution_id,
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let scope = pack.verification_scope(
        &project.document,
        revision.value(),
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let context = VerificationContextV1::new(
        scenario_id,
        revision.value(),
        started.input.snapshot_document_hash.clone(),
        model_hash,
        solution.canonical_hash()?,
        scope.checksum,
    )?;
    let verified_score = pack.score(
        &project.document,
        &solution,
        started.input.objective_context.as_ref(),
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let verification = pack.verify(
        &project.document,
        &solution,
        started.input.objective_context.as_ref(),
        &context,
        &verified_score,
        &eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
    )?;
    let accepted = AcceptedResult::new(solution, verification)?;
    let manifest = RunManifest::new(
        started.input.run_id,
        started.input.checksum,
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Optimal,
            solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        started.started_at,
        finished_at,
        Some(DurationMillis::new(1_000)?),
        Some(DurationMillis::new(100)?),
        Some(DurationMillis::new(500)?),
        RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let evidence = accepted
        .solution
        .assignments
        .iter()
        .flat_map(|assignment| assignment.evidence.iter())
        .chain(
            accepted
                .verification
                .required_rule_results
                .iter()
                .flat_map(|rule| rule.evidence.iter()),
        )
        .cloned()
        .map(|id| (id, VerificationValue::Boolean(true)))
        .collect();
    store
        .finalize_accepted_run(accepted, manifest, evidence)
        .await?;
    Ok(store.load_accepted_result(solution_id).await?)
}

fn adoption_request(
    scenario_id: ScenarioId,
    expected_revision: Revision,
    result: AcceptedResultRefV1,
    truncate_redo: bool,
) -> Result<SolutionAdoptBaseRequestV1, Box<dyn Error>> {
    let ids = SystemIdGenerator;
    Ok(SolutionAdoptBaseRequestV1 {
        schema_version: SOLUTION_ADOPT_BASE_SCHEMA_VERSION,
        request_id: RequestId::new(&ids)?,
        command_id: CommandId::new(&ids)?,
        scenario_id,
        expected_revision,
        result,
        actor: ActorRef {
            actor_id: Some("test.adopter".to_owned()),
            display_name: "Base adoption test".to_owned(),
        },
        truncate_redo,
    })
}

fn manual_admission_request(
    scenario_id: ScenarioId,
    expected_revision: Revision,
    original: AcceptedResultRefV1,
    assignments: Vec<eutheto_workforce::model::AssignmentPair>,
) -> Result<ManualSelectionAdmissionRequestV1, Box<dyn Error>> {
    Ok(ManualSelectionAdmissionRequestV1 {
        schema_version: eutheto_core::MANUAL_SELECTION_ADMISSION_SCHEMA_VERSION,
        request_id: request_id()?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision,
        actor: ActorRef {
            actor_id: Some("test.manual".to_owned()),
            display_name: "Manual selection test".to_owned(),
        },
        original,
        assignments,
        lock_changes: Vec::new(),
        truncate_redo: false,
    })
}

fn stored_workforce_manual(
    document: &ScenarioDocument,
) -> Result<(EntityId, eutheto_workforce::model::ManualSelection), Box<dyn Error>> {
    let (id, value) = document
        .domain
        .entities
        .iter()
        .find(|(_, value)| {
            value.get("kind").and_then(serde_json::Value::as_str) == Some("manualSelection")
        })
        .ok_or("Workforce manual selection is missing")?;
    let eutheto_workforce::model::WorkforceEntity::ManualSelection(manual) =
        serde_json::from_value(value.clone())?
    else {
        return Err("Workforce manual entity has an unexpected kind".into());
    };
    Ok((*id, manual))
}

fn stored_workforce_base(
    document: &ScenarioDocument,
) -> Result<eutheto_workforce::model::BaseSchedule, Box<dyn Error>> {
    let (_, value) = document
        .domain
        .entities
        .iter()
        .find(|(_, value)| {
            value.get("kind").and_then(serde_json::Value::as_str) == Some("baseSchedule")
        })
        .ok_or("Workforce base schedule is missing")?;
    let eutheto_workforce::model::WorkforceEntity::BaseSchedule(base) =
        serde_json::from_value(value.clone())?
    else {
        return Err("Workforce base entity has an unexpected kind".into());
    };
    Ok(base)
}

// The saved source, replacement, CAS/no-op, journal, and restart form one durable behavior.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn workforce_base_adoption_replacement_undo_redo_and_restart_preserve_identity()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let generated_base_id = Uuid::parse_str("018f7b40-a000-7000-8000-000000000200")?;
    let first_dependencies = dependencies_with_fixed_ids(&directory, [generated_base_id])?;
    let (store, initialization) =
        SqliteScenarioStore::open(&first_dependencies.paths.database).await?;
    let store = Arc::new(store);
    let document = workforce_adoption_document()?;
    let scenario_id = document.scenario_id;
    store.create_project(NewProject { document }).await?;
    let app =
        EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, first_dependencies)
            .boxed()?;

    let first =
        persist_workforce_accepted_result(&store, scenario_id, Revision::INITIAL, 1).await?;
    let first_ref = AcceptedResultRefV1::from_result(&first.portable.accepted_result)?;
    let selected = app
        .execute(AppCommand::SolutionSelect(SolutionSelectRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::INITIAL,
            solution_id: first_ref.solution_id,
        }))
        .await
        .boxed()?;
    assert!(matches!(selected, AppCommandResult::SolutionSelected(_)));
    assert!(
        stored_workforce_base(&store.get_project(scenario_id).await?.document).is_err(),
        "view selection must not adopt the base schedule"
    );

    let adopted = app
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            Revision::INITIAL,
            first_ref.clone(),
            false,
        )?))
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(first_receipt) = adopted else {
        return Err("first base adoption did not return a scenario receipt".into());
    };
    assert_eq!(first_receipt.new_revision, Revision::new(1));
    assert!(first_receipt.inverse.is_some());
    let first_project = store.get_project(scenario_id).await?;
    let first_base = stored_workforce_base(&first_project.document)?;
    assert_eq!(first_base.source_solution_id, first_ref.solution_id);
    assert_eq!(first_base.source_revision, Revision::INITIAL);
    assert_eq!(first_base.assignments.len(), 1);
    let base_id = first_base.id;

    let second =
        persist_workforce_accepted_result(&store, scenario_id, first_receipt.new_revision, 2)
            .await?;
    let second_ref = AcceptedResultRefV1::from_result(&second.portable.accepted_result)?;
    let replaced = app
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            first_receipt.new_revision,
            second_ref.clone(),
            false,
        )?))
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(second_receipt) = replaced else {
        return Err("base replacement did not return a scenario receipt".into());
    };
    assert_eq!(second_receipt.new_revision, Revision::new(2));
    let replaced_project = store.get_project(scenario_id).await?;
    let replaced_base = stored_workforce_base(&replaced_project.document)?;
    assert_eq!(replaced_base.id, base_id);
    assert_eq!(replaced_base.source_solution_id, second_ref.solution_id);
    assert_eq!(replaced_base.source_revision, Revision::new(1));
    assert_ne!(replaced_base.assignments, first_base.assignments);

    assert!(matches!(
        app.execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            first_receipt.new_revision,
            second_ref.clone(),
            false,
        )?))
        .await,
        Err(AppError::Conflict {
            expected_revision,
            actual_revision,
        }) if expected_revision == Revision::new(1) && actual_revision == Revision::new(2)
    ));
    let no_op = app
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            second_receipt.new_revision,
            second_ref.clone(),
            false,
        )?))
        .await;
    assert!(matches!(no_op, Err(AppError::Validation(_))));
    assert_eq!(
        store.get_project(scenario_id).await?.summary.revision,
        second_receipt.new_revision
    );

    let undone = app
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: second_receipt.new_revision,
        })
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(undo_receipt) = undone else {
        return Err("base replacement undo did not return a scenario receipt".into());
    };
    let undone_base = stored_workforce_base(&store.get_project(scenario_id).await?.document)?;
    assert_eq!(undone_base, first_base);
    let redone = app
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: undo_receipt.new_revision,
        })
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(redo_receipt) = redone else {
        return Err("base replacement redo did not return a scenario receipt".into());
    };
    assert_eq!(redo_receipt.new_revision, Revision::new(4));
    let redone_base = stored_workforce_base(&store.get_project(scenario_id).await?.document)?;
    assert_eq!(redone_base.id, base_id);
    assert_eq!(redone_base, replaced_base);

    drop(app);
    drop(store);
    let dependencies = dependencies(&directory)?;
    let (reopened_store, initialization) =
        SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let reopened_store = Arc::new(reopened_store);
    let reopened = EuthetoApp::from_initialized_store(
        Arc::clone(&reopened_store),
        initialization,
        dependencies,
    )
    .boxed()?;
    let restarted_base =
        stored_workforce_base(&reopened_store.get_project(scenario_id).await?.document)?;
    assert_eq!(restarted_base.id, base_id);
    assert_eq!(restarted_base.source_solution_id, second_ref.solution_id);
    assert_eq!(
        reopened_store
            .get_project(scenario_id)
            .await?
            .summary
            .revision,
        redo_receipt.new_revision
    );
    drop(reopened);
    Ok(())
}

#[tokio::test]
async fn workforce_manual_admission_rejects_unapproved_worker_source() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let document = workforce_adoption_document()?;
    let scenario_id = document.scenario_id;
    store.create_project(NewProject { document }).await?;
    let app = EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, dependencies)
        .boxed()?;

    let accepted =
        persist_workforce_accepted_result(&store, scenario_id, Revision::INITIAL, 1).await?;
    let original_ref = AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?;
    let adopted = app
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            Revision::INITIAL,
            original_ref.clone(),
            false,
        )?))
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(adoption_receipt) = adopted else {
        return Err("Workforce base adoption did not return a scenario receipt".into());
    };
    let source_base = stored_workforce_base(&store.get_project(scenario_id).await?.document)?;
    let Err(error) = app
        .admit_manual_selection(
            manual_admission_request(
                scenario_id,
                adoption_receipt.new_revision,
                original_ref,
                source_base.assignments,
            )?,
            eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )
        .await
    else {
        return Err("a synthetic backend was admitted without approved worker observations".into());
    };
    assert!(matches!(
        error,
        AppError::Verification(failure) if failure.code == "solution.reverification_mismatch"
    ));
    let current = store.get_project(scenario_id).await?;
    assert_eq!(current.summary.revision, adoption_receipt.new_revision);
    assert!(stored_workforce_manual(&current.document).is_err());
    assert_eq!(store.history(scenario_id).await?.len(), 1);
    let Err(error) = app
        .manual_edit(
            eutheto_core::ManualEditRequestV1::Read {
                schema_version: 1,
                request_id: RequestId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: adoption_receipt.new_revision,
                offset: 0,
                limit: 10,
            },
            eutheto_types::OperationControl::Cancellation(eutheto_types::CancellationToken::new()),
        )
        .await
    else {
        return Err("guided read authenticated a synthetic worker source".into());
    };
    assert!(matches!(error, AppError::Verification(failure)
        if failure.code == "solution.reverification_mismatch"));
    Ok(())
}

#[tokio::test]
async fn workforce_base_adoption_rejects_stale_current_entity_without_history()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let document = workforce_adoption_document()?;
    let scenario_id = document.scenario_id;
    store.create_project(NewProject { document }).await?;
    let app = EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, dependencies)
        .boxed()?;
    let accepted =
        persist_workforce_accepted_result(&store, scenario_id, Revision::INITIAL, 3).await?;
    let accepted_ref = AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?;
    let remove = app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: Some("test.stale".to_owned()),
                    display_name: "Stale base test".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::REMOVE_ENTITY.to_owned(),
                    payload: json!({"entityId": workforce_fixture::id(1)}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(remove_receipt) = remove else {
        return Err("stale-base setup did not return a scenario receipt".into());
    };
    assert_eq!(remove_receipt.new_revision, Revision::new(1));

    let rejected = app
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            remove_receipt.new_revision,
            accepted_ref,
            false,
        )?))
        .await;
    assert!(matches!(rejected, Err(AppError::Validation(_))));
    assert_eq!(
        store.get_project(scenario_id).await?.summary.revision,
        remove_receipt.new_revision
    );
    let history = app.query(AppQuery::History(scenario_id)).await.boxed()?;
    let AppQueryResult::History(history) = history else {
        return Err("stale-base history query returned an unexpected result".into());
    };
    assert_eq!(history.len(), 1);
    Ok(())
}

struct SolutionApplicationFixture {
    app: EuthetoApp,
    store: Arc<SqliteScenarioStore>,
    scenario_id: ScenarioId,
    stale: StoredAcceptedResultV2,
    current: StoredAcceptedResultV2,
}

async fn solution_application_fixture(
    directory: &TempDir,
) -> Result<SolutionApplicationFixture, Box<dyn Error>> {
    let dependencies = dependencies(directory)?;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, dependencies)
        .boxed()?;
    let scenario_id = create_project(&app, "Accepted solutions").await?;
    let first = app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_entity_envelope(scenario_id, Revision::INITIAL, "first")?,
            truncate_redo: false,
        })
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(first) = first else {
        return Err("expected first scenario command result".into());
    };
    let stale = persist_application_accepted_result(
        &store,
        scenario_id,
        first.new_revision,
        1,
        b"solution-api-objective-policy",
    )
    .await?;
    let second = app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_entity_envelope(scenario_id, first.new_revision, "second")?,
            truncate_redo: false,
        })
        .await
        .boxed()?;
    let AppCommandResult::ScenarioCommand(second) = second else {
        return Err("expected second scenario command result".into());
    };
    let current = persist_application_accepted_result(
        &store,
        scenario_id,
        second.new_revision,
        2,
        b"solution-api-objective-policy",
    )
    .await?;
    Ok(SolutionApplicationFixture {
        app,
        store,
        scenario_id,
        stale,
        current,
    })
}

#[cfg(debug_assertions)]
fn reseal_application_forged_score(
    path: &std::path::Path,
    portable: &PortableAcceptedResult,
) -> Result<(), Box<dyn Error>> {
    let original = &portable.accepted_result;
    let original_report = &original.verification;
    let context = VerificationContextV1::new(
        original_report.scenario_id,
        original_report.evaluated_revision,
        original_report.document_hash.clone(),
        original_report.planning_model_hash.clone(),
        original.solution.canonical_hash()?,
        original_report.verification_scope_checksum.clone(),
    )?;
    let mut score = original_report.score.clone();
    let level = score.levels.first_mut().ok_or("missing score level")?;
    level.value = level.value.checked_add(1).ok_or("score overflow")?;
    let report = VerificationReport::new(
        &context,
        original_report.required_rule_results.clone(),
        score,
        original_report.warnings.clone(),
        original_report.metrics.clone(),
    )?;
    let accepted = AcceptedResult::new(original.solution.clone(), report)?;
    let RunTerminalOutcomeV1::Accepted { status, .. } = &portable.run_manifest.outcome else {
        return Err("accepted fixture has no accepted terminal outcome".into());
    };
    let manifest = RunManifest::new(
        portable.run_manifest.run_id,
        portable.run_manifest.run_input_checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: *status,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        portable.run_manifest.started_at,
        portable.run_manifest.finished_at,
        portable.run_manifest.elapsed_milliseconds,
        portable.run_manifest.first_incumbent_milliseconds,
        portable.run_manifest.first_verified_feasible_milliseconds,
        portable.run_manifest.phase_timings,
        portable.run_manifest.verification_warnings.clone(),
    )?;
    let forged = PortableAcceptedResult::new(
        portable.run_input.clone(),
        manifest,
        accepted,
        portable.evidence.clone(),
    )?;
    let connection = Connection::open(path)?;
    connection.execute(
        "UPDATE solutions SET normalized_solution_json = ?1, score_json = ?2, verification_report_json = ?3 WHERE id = ?4",
        rusqlite::params![
            serde_json::to_string(&forged.accepted_result.solution)?,
            serde_json::to_string(&forged.accepted_result.verification.score)?,
            serde_json::to_string(&forged.accepted_result.verification)?,
            forged.accepted_result.solution.solution_id.to_string(),
        ],
    )?;
    connection.execute(
        "UPDATE solve_runs SET run_manifest_json = ?1 WHERE id = ?2",
        rusqlite::params![
            serde_json::to_string(&forged.run_manifest)?,
            forged.run_manifest.run_id.to_string(),
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[cfg(debug_assertions)]
#[tokio::test]
async fn accepted_snapshot_fresh_gate_rejects_coherent_reseal_after_cache()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let solution_id = fixture
        .current
        .portable
        .accepted_result
        .solution
        .solution_id;
    let summary = fixture
        .app
        .query(AppQuery::SolutionGetSummary(SolutionSummaryRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id,
        }))
        .await
        .boxed()?;
    assert!(matches!(summary, AppQueryResult::SolutionSummary(_)));
    let view = fixture
        .app
        .query(AppQuery::SolutionGetView(SolutionViewRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id,
            view_id: "official.test.result.summary".to_owned(),
            parameters: json!({}),
            continuation: None,
        }))
        .await
        .boxed()?;
    assert!(matches!(view, AppQueryResult::SolutionView(_)));

    reseal_application_forged_score(
        &directory.path().join("eutheto.sqlite"),
        &fixture.current.portable,
    )?;
    let AppQueryResult::SolutionList(list) = fixture
        .app
        .query(AppQuery::SolutionList(SolutionListRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
        }))
        .await
        .boxed()?
    else {
        return Err("expected saved result metadata list".into());
    };
    assert!(!list.freshly_verified);
    assert_ne!(
        list.solutions
            .iter()
            .find(|entry| entry.result.solution_id == solution_id)
            .ok_or("missing saved result metadata")?
            .score,
        fixture.current.portable.accepted_result.verification.score
    );

    let Err(detail_error) = fixture
        .app
        .query(AppQuery::SolutionGetSummary(SolutionSummaryRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id,
        }))
        .await
    else {
        return Err("coherent forged detail must fail fresh verification".into());
    };
    assert!(matches!(
        detail_error,
        AppError::Verification(failure) if failure.code == "solution.reverification_mismatch"
    ));
    let Err(view_error) = fixture
        .app
        .query(AppQuery::SolutionGetView(SolutionViewRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id,
            view_id: "official.test.result.summary".to_owned(),
            parameters: json!({}),
            continuation: None,
        }))
        .await
    else {
        return Err("coherent forged view must fail fresh verification".into());
    };
    assert!(matches!(
        view_error,
        AppError::Verification(failure) if failure.code == "solution.reverification_mismatch"
    ));
    let Err(scenario_error) = fixture
        .app
        .query(AppQuery::ExportScenario {
            scenario_id: fixture.scenario_id,
            cancellation: fixture.app.setup_cancellation(),
        })
        .await
    else {
        return Err("scenario archive must not export a coherently forged local result".into());
    };
    assert!(matches!(
        scenario_error,
        AppError::Verification(failure) if failure.code == "solution.reverification_mismatch"
    ));
    let Err(backup_error) = fixture
        .app
        .query(AppQuery::ExportBackup {
            cancellation: fixture.app.setup_cancellation(),
            title: "Accepted result backup".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::IncludeAll,
                include_audit: false,
            },
        })
        .await
    else {
        return Err("result-inclusive backup must not export forged acceptance".into());
    };
    assert!(matches!(
        backup_error,
        AppError::Verification(failure) if failure.code == "solution.reverification_mismatch"
    ));
    assert!(matches!(
        fixture
            .app
            .query(AppQuery::ExportBackup {
                cancellation: fixture.app.setup_cancellation(),
                title: "Editable backup without results".to_owned(),
                selection: BackupSelection {
                    include_results: false,
                    assets: BackupAssetSelection::IncludeAll,
                    include_audit: false,
                },
            })
            .await
            .boxed()?,
        AppQueryResult::BackupBundle { .. }
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_view_releases_capture_lock_and_keeps_exact_result_document()
-> Result<(), Box<dyn Error>> {
    use setup_control::{ControlledPack, PauseAt, PreparationGate, ReleaseOnDrop};
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let gate = Arc::new(PreparationGate::default());
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    gate.pause_at.store(PauseAt::View as u8, Ordering::SeqCst);
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(ControlledPack {
            pack: eutheto_command::OfficialTestPack,
            gate: Arc::clone(&gate),
        })
        .build()?;
    let app = EuthetoApp::from_initialized_store_with_pack_registry(
        Arc::clone(&fixture.store),
        fixture.app.initialization().clone(),
        dependencies(&directory)?,
        registry,
    )
    .boxed()?;
    let expected = AcceptedResultRefV1::from_result(&fixture.current.portable.accepted_result)?;
    let revision = Revision::new(
        fixture
            .current
            .portable
            .accepted_result
            .solution
            .scenario_revision,
    );
    let scenario_id = fixture.scenario_id;
    gate.armed.store(true, Ordering::SeqCst);
    let reading = tokio::spawn({
        let app = app.clone();
        async move {
            app.query(AppQuery::SolutionGetView(SolutionViewRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                solution_id: expected.solution_id,
                view_id: "official.test.result.summary".to_owned(),
                parameters: json!({}),
                continuation: None,
            }))
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.entered.notified()).await?;
    let committed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        app.execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_entity_envelope(scenario_id, revision, "third")?,
            truncate_redo: false,
        }),
    )
    .await?
    .boxed()?;
    let AppCommandResult::ScenarioCommand(committed) = committed else {
        return Err("expected committed edit while accepted view was paused".into());
    };
    assert_eq!(committed.new_revision.value(), revision.value() + 1);
    gate.release();
    let AppQueryResult::SolutionView(view) = reading.await?.boxed()? else {
        return Err("expected accepted solution view".into());
    };
    assert_eq!(view.result, expected);
    assert_eq!(view.scenario_revision, revision);
    assert_eq!(view.current_revision, revision);
    assert_eq!(view.view.data["entityCount"], 2);
    assert_eq!(
        view.view.data["assignmentCount"].as_u64(),
        Some(u64::try_from(
            fixture
                .current
                .portable
                .accepted_result
                .solution
                .assignments
                .len()
        )?),
    );
    Ok(())
}

#[tokio::test]
async fn accepted_view_reports_cancellation_without_cancelling_sibling_reads()
-> Result<(), Box<dyn Error>> {
    use setup_control::{ControlledPack, PauseAt, PreparationGate, ReleaseOnDrop};
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let gate = Arc::new(PreparationGate::default());
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    gate.pause_at.store(PauseAt::View as u8, Ordering::SeqCst);
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(ControlledPack {
            pack: eutheto_command::OfficialTestPack,
            gate: Arc::clone(&gate),
        })
        .build()?;
    let app = EuthetoApp::from_initialized_store_with_pack_registry(
        Arc::clone(&fixture.store),
        fixture.app.initialization().clone(),
        dependencies(&directory)?,
        registry,
    )
    .boxed()?;
    let request = SolutionViewRequestV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        scenario_id: fixture.scenario_id,
        solution_id: fixture
            .current
            .portable
            .accepted_result
            .solution
            .solution_id,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    let cancellation = app.setup_cancellation();
    gate.armed.store(true, Ordering::SeqCst);
    let reading = tokio::spawn({
        let app = app.clone();
        let request = request.clone();
        let cancellation = cancellation.clone();
        async move {
            app.solution_view_with_cancellation(request, cancellation)
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.entered.notified()).await?;
    cancellation.cancel();
    gate.release();
    assert!(
        matches!(reading.await?, Err(AppError::Protocol(error)) if error.code == "operation.cancelled")
    );
    let sibling = app
        .solution_view_with_cancellation(request, app.setup_cancellation())
        .await
        .boxed()?;
    assert_eq!(sibling.view.data["entityCount"], 2);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_accepted_source_read_cancels_promptly_while_actor_finishes_unrelated_commit()
-> Result<(), Box<dyn Error>> {
    struct ReleaseHook(Option<CommandCommitTestHook>);
    impl ReleaseHook {
        fn release(&mut self) {
            if let Some(hook) = self.0.take() {
                hook.release();
            }
        }
    }
    impl Drop for ReleaseHook {
        fn drop(&mut self) {
            self.release();
        }
    }

    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let other_scenario = create_project(&fixture.app, "Independent actor commit").await?;
    let dependencies = dependencies(&directory)?;
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::BeforeFinalCancellationCheck);
    let (store, initialization) = SqliteScenarioStore::open_with_options(
        &dependencies.paths.database,
        OpenOptions::new(SnapshotPolicy::default()).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, dependencies)
        .boxed()?;
    let request = SolutionViewRequestV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        scenario_id: fixture.scenario_id,
        solution_id: fixture
            .current
            .portable
            .accepted_result
            .solution
            .solution_id,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    let command = AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: add_entity_envelope(other_scenario, Revision::INITIAL, "pending")?,
        truncate_redo: false,
    };
    let committing = tokio::spawn({
        let app = app.clone();
        async move { app.execute(command).await }
    });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    let mut release = ReleaseHook(Some(hook));
    let cancellation = app.setup_cancellation();
    let mut reading = tokio::spawn({
        let app = app.clone();
        let request = request.clone();
        let cancellation = cancellation.clone();
        async move {
            app.solution_view_with_cancellation(request, cancellation)
                .await
        }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut reading)
            .await
            .is_err(),
        "accepted source read did not queue behind the paused actor"
    );
    cancellation.cancel();
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(2), reading).await??;
    assert!(matches!(
        cancelled,
        Err(AppError::Protocol(error)) if error.code == "operation.cancelled"
    ));
    release.release();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(10), committing)
            .await??
            .boxed()?,
        AppCommandResult::ScenarioCommand(_)
    ));
    let sibling = app
        .solution_view_with_cancellation(request, app.setup_cancellation())
        .await
        .boxed()?;
    assert_eq!(sibling.view.data["entityCount"], 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_accepted_view_returns_before_blocked_verifier_and_does_not_cache_it()
-> Result<(), Box<dyn Error>> {
    use setup_control::{ControlledPack, PauseAt, PreparationGate, ReleaseOnDrop};
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let gate = Arc::new(PreparationGate::default());
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    gate.pause_at.store(PauseAt::Score as u8, Ordering::SeqCst);
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(ControlledPack {
            pack: eutheto_command::OfficialTestPack,
            gate: Arc::clone(&gate),
        })
        .build()?;
    let app = EuthetoApp::from_initialized_store_with_pack_registry(
        Arc::clone(&fixture.store),
        fixture.app.initialization().clone(),
        dependencies(&directory)?,
        registry,
    )
    .boxed()?;
    let request = SolutionViewRequestV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        scenario_id: fixture.scenario_id,
        solution_id: fixture
            .current
            .portable
            .accepted_result
            .solution
            .solution_id,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    let cancellation = app.setup_cancellation();
    gate.armed.store(true, Ordering::SeqCst);
    let reading = tokio::spawn({
        let app = app.clone();
        let request = request.clone();
        let cancellation = cancellation.clone();
        async move {
            app.solution_view_with_cancellation(request, cancellation)
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.entered.notified()).await?;
    cancellation.cancel();
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(2), reading).await??;
    assert!(matches!(
        cancelled,
        Err(AppError::Protocol(error)) if error.code == "operation.cancelled"
    ));
    gate.release();
    let sibling = app
        .solution_view_with_cancellation(request, app.setup_cancellation())
        .await
        .boxed()?;
    assert_eq!(sibling.view.data["entityCount"], 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_views_bound_captured_results_through_blocked_projection()
-> Result<(), Box<dyn Error>> {
    use setup_control::{ControlledPack, PauseAt, PreparationGate, ReleaseOnDrop};
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let gate = Arc::new(PreparationGate::default());
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    gate.pause_at.store(PauseAt::View as u8, Ordering::SeqCst);
    // The fifth pause detects an accidental admission past the four captured-result slots.
    gate.remaining_pauses.store(5, Ordering::SeqCst);
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(ControlledPack {
            pack: eutheto_command::OfficialTestPack,
            gate: Arc::clone(&gate),
        })
        .build()?;
    let app = EuthetoApp::from_initialized_store_with_pack_registry(
        Arc::clone(&fixture.store),
        fixture.app.initialization().clone(),
        dependencies(&directory)?,
        registry,
    )
    .boxed()?;
    let request = SolutionViewRequestV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        scenario_id: fixture.scenario_id,
        solution_id: fixture
            .current
            .portable
            .accepted_result
            .solution
            .solution_id,
        view_id: "official.test.result.summary".to_owned(),
        parameters: json!({}),
        continuation: None,
    };
    let mut readers = Vec::new();
    for _ in 0..4 {
        let app = app.clone();
        let request = request.clone();
        readers.push(tokio::spawn(async move {
            app.solution_view_with_cancellation(request, app.setup_cancellation())
                .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while gate.entered_count.load(Ordering::SeqCst) != 4 {
            gate.entered.notified().await;
        }
    })
    .await?;
    let fifth_cancellation = app.setup_cancellation();
    let fifth = tokio::spawn({
        let app = app.clone();
        let request = request.clone();
        let cancellation = fifth_cancellation.clone();
        async move {
            app.solution_view_with_cancellation(request, cancellation)
                .await
        }
    });
    let fifth_entered = tokio::time::timeout(std::time::Duration::from_millis(300), async {
        while gate.entered_count.load(Ordering::SeqCst) != 5 {
            gate.entered.notified().await;
        }
    })
    .await;
    assert!(
        fifth_entered.is_err(),
        "a fifth captured view entered blocked projection"
    );
    fifth_cancellation.cancel();
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(2), fifth).await??;
    assert!(matches!(
        cancelled,
        Err(AppError::Protocol(error)) if error.code == "operation.cancelled"
    ));
    gate.release();
    for reader in readers {
        assert_eq!(
            reader.await?.boxed()?.view.data["entityCount"],
            2,
            "admitted views still return their verified original document"
        );
    }
    Ok(())
}

#[tokio::test]
async fn create_edit_reopen_and_stale_conflict_use_the_application_service()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    let app = EuthetoApp::open(dependencies.clone()).await.boxed()?;
    let scenario_id = create_project(&app, "Roster").await.boxed()?;
    let mut changes = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;

    let first = add_entity_envelope(scenario_id, Revision::INITIAL, "Ada")?;
    let first_command_id = first.command_id;
    let initiating_request_id = request_id()?;
    let committed = app
        .execute(AppCommand::ApplyScenario {
            request_id: initiating_request_id,
            envelope: first,
            truncate_redo: false,
        })
        .await
        .boxed()?;
    match committed {
        AppCommandResult::ScenarioCommand(result) => {
            assert_eq!(result.new_revision, Revision::new(1));
        }
        other => return Err(format!("unexpected command result: {other:?}").into()),
    }
    let event = changes.recv().await.boxed()?;
    assert!(matches!(
        event.payload,
        EventPayload::ScenarioChanged { context, .. }
            if context.scenario_id == Some(scenario_id)
                && context.revision == Some(Revision::new(1))
                && context.request_id == Some(initiating_request_id)
    ));

    let stale = add_entity_envelope(scenario_id, Revision::INITIAL, "Grace")?;
    let stale_error = app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: stale,
            truncate_redo: false,
        })
        .await;
    assert!(matches!(
        stale_error,
        Err(AppError::Conflict {
            expected_revision,
            actual_revision,
        }) if expected_revision == Revision::INITIAL && actual_revision == Revision::new(1)
    ));

    drop(app);
    let reopened = EuthetoApp::open(dependencies).await.boxed()?;
    let view = scenario_view(&reopened, scenario_id).await.boxed()?;
    assert_eq!(view.revision, Revision::new(1));
    assert_eq!(view.document.domain.entities.len(), 1);
    assert!(matches!(
        reopened
            .query(AppQuery::ValidateScenario(scenario_id))
            .await
            .boxed()?,
        AppQueryResult::Validation(report) if report.issues.is_empty()
    ));
    let metadata = reopened
        .query(AppQuery::ProjectMetadata(scenario_id))
        .await
        .boxed()?;
    assert!(matches!(
        metadata,
        AppQueryResult::Project(project)
            if project.scenario_id == scenario_id
                && project.title == "Roster"
                && project.description == "integration project"
                && project.revision == Revision::new(1)
                && project.created_at == timestamp("2026-01-10T12:00:00Z")?
                && project.updated_at == timestamp("2026-01-10T12:00:00Z")?
                && project.archived_at.is_none()
    ));
    let history = reopened
        .query(AppQuery::History(scenario_id))
        .await
        .boxed()?;
    assert!(matches!(
        history,
        AppQueryResult::History(entries)
            if entries.len() == 1
                && entries[0].id == first_command_id
                && entries[0].revision_before == Revision::INITIAL
                && entries[0].revision_after == Revision::new(1)
                && entries[0].actor.actor_id.as_deref() == Some("test.actor")
                && entries[0].actor.display_name == "Integration Test"
                && entries[0].source == CommandSource::System
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_mutations_serialize_per_scenario_without_a_global_lock()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let clock = Arc::new(BlockingFirstClock::new(timestamp("2026-01-10T12:00:00Z")?));
    let mut app_dependencies = dependencies(&directory)?;
    app_dependencies.clock = clock.clone();
    let app = EuthetoApp::open(app_dependencies).await.boxed()?;
    let first_scenario = create_project(&app, "Contended scenario").await.boxed()?;
    let independent_scenario = create_project(&app, "Independent scenario").await.boxed()?;
    let first_command = add_entity_envelope(first_scenario, Revision::INITIAL, "First mutation")?;
    let same_scenario_command =
        add_entity_envelope(first_scenario, Revision::new(1), "Serialized mutation")?;
    let independent_command = add_entity_envelope(
        independent_scenario,
        Revision::INITIAL,
        "Independent mutation",
    )?;
    let entered = Arc::clone(&clock.entered);
    let release = Arc::clone(&clock.release);

    clock.arm();
    let first_app = app.clone();
    let first_request_id = request_id()?;
    let first_task = tokio::spawn(async move {
        first_app
            .execute(AppCommand::ApplyScenario {
                request_id: first_request_id,
                envelope: first_command,
                truncate_redo: false,
            })
            .await
    });
    tokio::task::spawn_blocking(move || {
        entered.wait();
    })
    .await?;

    let mut same_scenario = Box::pin(app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: same_scenario_command,
        truncate_redo: false,
    }));
    let mut same_scenario_result = None;
    poll_fn(|context| {
        if let Poll::Ready(result) = same_scenario.as_mut().poll(context) {
            same_scenario_result = Some(result);
        }
        Poll::Ready(())
    })
    .await;
    let same_scenario_serialized = same_scenario_result.is_none() && clock.calls_after_arm() == 1;

    let calls_before_independent = clock.calls_after_arm();
    // Custody reads may yield: prove completion before release, not progress in a single poll.
    let independent_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        app.execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: independent_command,
            truncate_redo: false,
        }),
    )
    .await;
    let independent_made_progress =
        independent_result.is_ok() && clock.calls_after_arm() > calls_before_independent;
    let independent_result = independent_result.ok();

    tokio::task::spawn_blocking(move || {
        release.wait();
    })
    .await?;
    let first_result = first_task.await?;
    if same_scenario_result.is_none() {
        same_scenario_result = Some(same_scenario.await);
    }

    if !same_scenario_serialized {
        return Err("a contending same-scenario mutation reached the clock before release".into());
    }
    if !independent_made_progress {
        return Err("an independent scenario mutation was blocked by a global lock".into());
    }
    first_result.boxed()?;
    match independent_result {
        Some(result) => result.boxed()?,
        None => return Err("the independent scenario mutation did not complete".into()),
    };
    match same_scenario_result {
        Some(result) => result.boxed()?,
        None => return Err("the same-scenario mutation did not complete after release".into()),
    };
    let first_view = scenario_view(&app, first_scenario).await.boxed()?;
    let independent_view = scenario_view(&app, independent_scenario).await.boxed()?;
    assert_eq!(first_view.revision, Revision::new(2));
    assert_eq!(first_view.document.domain.entities.len(), 2);
    assert_eq!(independent_view.revision, Revision::new(1));
    assert_eq!(independent_view.document.domain.entities.len(), 1);
    Ok(())
}

#[tokio::test]
async fn committed_mutations_reach_independent_subscribers_and_lag_is_recoverable()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let mut first_window = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let mut second_window = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let create_request_id = request_id()?;
    let created = app
        .execute(AppCommand::CreateProject {
            request_id: create_request_id,
            title: "Shared window state".to_owned(),
            description: String::new(),
            domain_pack: DomainPackRef {
                id: "official.test".parse()?,
                schema_version: 1,
            },
            settings: settings()?,
        })
        .await
        .boxed()?;
    let scenario_id = match created {
        AppCommandResult::Project(project) => project.scenario_id,
        other => return Err(format!("unexpected create result: {other:?}").into()),
    };
    for event in [
        first_window.recv().await.boxed()?,
        second_window.recv().await.boxed()?,
    ] {
        assert!(matches!(
            event.payload,
            EventPayload::ScenarioChanged { context, .. }
                if context.request_id == Some(create_request_id)
                    && context.scenario_id == Some(scenario_id)
        ));
    }

    let mut lagged = app.subscribe(EventTopic::AppNotification).await.boxed()?;
    for index in 0..300 {
        let notification_request_id = request_id()?;
        app.execute(AppCommand::SetSetting {
            expected_library_revision: app
                .application_settings_snapshot()
                .await
                .boxed()?
                .library_revision,
            request_id: notification_request_id,
            key: "appearance".to_owned(),
            value: json!({"theme": if index % 2 == 0 { "dark" } else { "light" }}),
        })
        .await
        .boxed()?;
    }
    assert!(matches!(
        lagged.recv().await,
        Err(AppError::Protocol(failure))
            if failure.code == "event.subscription_lagged" && failure.retryable
    ));
    let recovered = lagged.recv().await.boxed()?;
    assert!(matches!(
        recovered.payload,
        EventPayload::AppNotification { context, .. }
            if context.request_id.is_some()
    ));
    Ok(())
}
#[tokio::test]
async fn batch_undo_and_redo_survive_restart() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    let app = EuthetoApp::open(dependencies.clone()).await.boxed()?;
    let scenario_id = create_project(&app, "History").await.boxed()?;
    let initial_document = scenario_view(&app, scenario_id).await.boxed()?.document;
    let ids = SystemIdGenerator;
    let first = EntityId::new(&ids)?;
    let second = EntityId::new(&ids)?;
    let envelope = CommandEnvelope {
        command_id: CommandId::new(&ids)?,
        scenario_id,
        expected_revision: Revision::INITIAL,
        actor: ActorRef {
            actor_id: None,
            display_name: "Batch Test".to_owned(),
        },
        source: CommandSource::System,
        command: ScenarioCommand::ApplyBatch(CommandBatch {
            label: Some("Add people".to_owned()),
            commands: vec![
                ScenarioCommand::AddEntity(AddEntity {
                    entity_id: first,
                    value: json!({"id": first, "enabled": false, "target": 0}),
                }),
                ScenarioCommand::AddEntity(AddEntity {
                    entity_id: second,
                    value: json!({"id": second, "enabled": false, "target": 0}),
                }),
            ],
        }),
    };
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope,
        truncate_redo: false,
    })
    .await
    .boxed()?;
    app.execute(AppCommand::Undo {
        request_id: request_id()?,
        scenario_id,
        expected_revision: Revision::new(1),
    })
    .await
    .boxed()?;
    let undone = scenario_view(&app, scenario_id).await.boxed()?;
    assert_eq!(undone.document, initial_document);
    assert!(undone.document.domain.entities.is_empty());

    drop(app);
    let reopened = EuthetoApp::open(dependencies).await.boxed()?;
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(2),
        })
        .await
        .boxed()?;
    let view = scenario_view(&reopened, scenario_id).await.boxed()?;
    assert_eq!(view.revision, Revision::new(3));
    assert_eq!(view.document.domain.entities.len(), 2);
    let history = reopened
        .query(AppQuery::History(scenario_id))
        .await
        .boxed()?;
    assert!(matches!(history, AppQueryResult::History(entries) if entries.len() == 1));
    Ok(())
}

#[tokio::test]
async fn branch_truncation_requires_explicit_application_confirmation() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let scenario_id = create_project(&app, "Explicit branch truncation")
        .await
        .boxed()?;
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: add_entity_envelope(scenario_id, Revision::INITIAL, "Original branch")?,
        truncate_redo: false,
    })
    .await
    .boxed()?;
    app.execute(AppCommand::Undo {
        request_id: request_id()?,
        scenario_id,
        expected_revision: Revision::new(1),
    })
    .await
    .boxed()?;

    let replacement = add_entity_envelope(scenario_id, Revision::new(2), "Replacement branch")?;
    let unconfirmed = app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: replacement.clone(),
            truncate_redo: false,
        })
        .await;
    assert!(matches!(
        unconfirmed,
        Err(AppError::Validation(report))
            if report.issues.len() == 1
                && report.issues[0].code == "history.redo_branch_requires_truncation"
                && report.issues[0].field_path.as_deref() == Some("/truncateRedo")
    ));
    let unchanged = scenario_view(&app, scenario_id).await.boxed()?;
    assert_eq!(unchanged.revision, Revision::new(2));
    assert!(unchanged.document.domain.entities.is_empty());

    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: replacement,
        truncate_redo: true,
    })
    .await
    .boxed()?;
    let truncated = scenario_view(&app, scenario_id).await.boxed()?;
    assert_eq!(truncated.revision, Revision::new(3));
    assert_eq!(truncated.document.domain.entities.len(), 1);
    assert!(matches!(
        app.execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(3),
        })
        .await,
        Err(AppError::Validation(report))
            if report.issues.len() == 1
                && report.issues[0].code == "history.redo_unavailable"
    ));
    Ok(())
}

#[tokio::test]
async fn duplicate_archive_delete_and_settings_are_real_lifecycle_operations()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let source_id = create_project(&app, "Source").await.boxed()?;
    let duplicate = app
        .execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            expected_revision: Revision::INITIAL,
            source_id,
            title: "Copy".to_owned(),
        })
        .await
        .boxed()?;
    let duplicate_id = match duplicate {
        AppCommandResult::Project(project) => project.scenario_id,
        other => return Err(format!("unexpected duplicate result: {other:?}").into()),
    };
    assert_ne!(source_id, duplicate_id);
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: add_entity_envelope(source_id, Revision::INITIAL, "Changed source")?,
        truncate_redo: false,
    })
    .await
    .boxed()?;
    assert!(matches!(
        app.execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            source_id,
            expected_revision: Revision::INITIAL,
            title: "Stale copy".to_owned(),
        })
        .await,
        Err(AppError::Conflict {
            expected_revision,
            actual_revision,
        }) if expected_revision == Revision::INITIAL && actual_revision == Revision::new(1)
    ));

    app.execute(AppCommand::ArchiveProject {
        request_id: request_id()?,
        scenario_id: duplicate_id,
        expected_revision: Revision::INITIAL,
    })
    .await
    .boxed()?;
    let archived = app
        .query(AppQuery::ListProjects(ProjectScope::Archived))
        .await
        .boxed()?;
    assert!(matches!(archived, AppQueryResult::Projects(items) if items.len() == 1));
    app.execute(AppCommand::UnarchiveProject {
        request_id: request_id()?,
        scenario_id: duplicate_id,
        expected_revision: Revision::INITIAL,
    })
    .await
    .boxed()?;

    app.execute(AppCommand::SetSetting {
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .boxed()?
            .library_revision,
        request_id: request_id()?,
        key: "appearance".to_owned(),
        value: json!({"theme": "dark"}),
    })
    .await
    .boxed()?;
    assert!(matches!(
        app.query(AppQuery::Setting("appearance".to_owned()))
            .await
            .boxed()?,
        AppQueryResult::Setting(Some(setting)) if setting.value == json!({"theme": "dark"})
    ));
    assert!(matches!(
        app.execute(AppCommand::DeleteSetting {
            request_id: request_id()?,
            expected_library_revision: app.application_settings_snapshot().await.boxed()?.library_revision,
            key: "appearance".to_owned(),
        })
        .await
        .boxed()?,
        AppCommandResult::SettingsWritten(result) if result.changed
    ));

    app.execute(AppCommand::DeleteProject {
        request_id: request_id()?,
        scenario_id: duplicate_id,
        expected_revision: Revision::INITIAL,
    })
    .await
    .boxed()?;
    assert!(matches!(
        app.query(AppQuery::ProjectMetadata(duplicate_id)).await,
        Err(AppError::NotFound(_))
    ));
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "Keeps the complete stale-review lifecycle in one regression."
)]
#[tokio::test]
async fn portable_preview_is_stale_after_library_mutation() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let scenario_id = create_project(&app, "Portable").await.boxed()?;
    let add_command = add_entity_envelope(scenario_id, Revision::INITIAL, "Portable person")?;
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: add_command,
        truncate_redo: false,
    })
    .await
    .boxed()?;
    let bytes = match app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected export result: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert_eq!(inspected.scenarios.len(), 1);
    assert_eq!(inspected.scenarios[0].revision, Revision::new(1));
    let options = ImportOptions {
        restore_mode: RestoreMode::ImportScenario,
        include_results: false,
        include_assets: false,
    };
    let fresh_preview_id = match app
        .query(AppQuery::PreviewImport {
            cancellation: app.setup_cancellation(),
            bytes: bytes.clone(),
            options: options.clone(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected preview result: {other:?}").into()),
    };
    app.execute(AppCommand::ApplyImport {
        cancellation: app.setup_cancellation(),
        request_id: request_id()?,
        preview_id: fresh_preview_id,
        collision_plan: CollisionPlan {
            scenarios: BTreeMap::from([(scenario_id, CollisionAction::CreateCopy)]),
            supplemental: BTreeMap::new(),
        },
    })
    .await
    .boxed()?;
    assert!(matches!(
        app.query(AppQuery::ListProjects(ProjectScope::All))
            .await
            .boxed()?,
        AppQueryResult::Projects(projects) if projects.len() == 2
    ));

    let preview_id = match app
        .query(AppQuery::PreviewImport {
            cancellation: app.setup_cancellation(),
            bytes,
            options,
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected preview result: {other:?}").into()),
    };
    app.execute(AppCommand::DuplicateProject {
        request_id: request_id()?,
        expected_revision: Revision::new(1),
        source_id: scenario_id,
        title: "Mutation".to_owned(),
    })
    .await
    .boxed()?;
    let apply = app
        .execute(AppCommand::ApplyImport {
            cancellation: app.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan {
                scenarios: BTreeMap::new(),
                supplemental: BTreeMap::new(),
            },
        })
        .await;
    assert!(matches!(apply, Err(AppError::Conflict { .. })));
    let retry = app
        .execute(AppCommand::ApplyImport {
            cancellation: app.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan {
                scenarios: BTreeMap::new(),
                supplemental: BTreeMap::new(),
            },
        })
        .await;
    assert!(
        matches!(retry, Err(AppError::Protocol(failure)) if failure.code == "portable.preview_not_found")
    );
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "Keeps byte identity and rejected-publication cases in one fixture."
)]
#[tokio::test]
async fn prepared_scenario_publication_is_byte_exact_and_rejects_stale_or_changed_bytes()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let scenario_id = create_project(&app, "Prepared scenario").await?;
    let (bytes, revision, library_revision) = match app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle {
            bytes,
            scenario_revision,
            library_revision,
        } => (bytes, scenario_revision, library_revision),
        other => return Err(format!("unexpected prepared scenario: {other:?}").into()),
    };
    let digest = eutheto_export::sha256_hex(&bytes);
    let destination = directory.path().join("prepared-scenario.eutheto");
    app.execute(AppCommand::PublishPreparedPortable {
        cancellation: app.setup_cancellation(),
        destination: destination.clone(),
        bytes: bytes.clone(),
        expected_sha256: digest.clone(),
        binding: eutheto_core::PreparedPortableBinding::Scenario {
            scenario_id,
            expected_revision: revision,
            expected_library_revision: library_revision,
        },
    })
    .await
    .boxed()?;
    assert_eq!(std::fs::read(&destination)?, bytes);
    assert_eq!(
        eutheto_export::sha256_hex(&std::fs::read(&destination)?),
        digest
    );

    let stale_destination = directory.path().join("stale-scenario.eutheto");
    app.execute(AppCommand::SetSetting {
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .boxed()?
            .library_revision,
        request_id: request_id()?,
        key: "appearance".to_owned(),
        value: json!({"theme": "dark"}),
    })
    .await
    .boxed()?;
    let stale = app
        .execute(AppCommand::PublishPreparedPortable {
            cancellation: app.setup_cancellation(),
            destination: stale_destination.clone(),
            bytes,
            expected_sha256: digest,
            binding: eutheto_core::PreparedPortableBinding::Scenario {
                scenario_id,
                expected_revision: revision,
                expected_library_revision: library_revision,
            },
        })
        .await;
    assert!(matches!(stale, Err(AppError::Conflict { .. })));
    assert!(!stale_destination.exists());

    let (current_bytes, current_revision, current_library_revision) = match app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle {
            bytes,
            scenario_revision,
            library_revision,
        } => (bytes, scenario_revision, library_revision),
        other => return Err(format!("unexpected current scenario: {other:?}").into()),
    };
    let changed_destination = directory.path().join("changed-scenario.eutheto");
    let changed = app
        .execute(AppCommand::PublishPreparedPortable {
            cancellation: app.setup_cancellation(),
            destination: changed_destination.clone(),
            bytes: current_bytes,
            expected_sha256: "0".repeat(64),
            binding: eutheto_core::PreparedPortableBinding::Scenario {
                scenario_id,
                expected_revision: current_revision,
                expected_library_revision: current_library_revision,
            },
        })
        .await;
    assert!(matches!(
        changed,
        Err(AppError::Protocol(failure)) if failure.code == "portable.prepared_digest_mismatch"
    ));
    assert!(!changed_destination.exists());
    Ok(())
}

#[tokio::test]
async fn prepared_backup_publication_binds_the_library_revision() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    create_project(&app, "Prepared backup").await?;
    let (bytes, library_revision) = match app
        .query(AppQuery::ExportBackup {
            cancellation: app.setup_cancellation(),
            title: "Prepared backup".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle {
            bytes,
            library_revision,
            ..
        } => (bytes, library_revision),
        other => return Err(format!("unexpected prepared backup: {other:?}").into()),
    };
    app.execute(AppCommand::SetSetting {
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .boxed()?
            .library_revision,
        request_id: request_id()?,
        key: "locale".to_owned(),
        value: json!("en-GB"),
    })
    .await
    .boxed()?;
    let destination = directory.path().join("stale-backup.eutheto");
    let result = app
        .execute(AppCommand::PublishPreparedPortable {
            cancellation: app.setup_cancellation(),
            destination: destination.clone(),
            expected_sha256: eutheto_export::sha256_hex(&bytes),
            bytes,
            binding: eutheto_core::PreparedPortableBinding::Backup {
                expected_library_revision: library_revision,
            },
        })
        .await;
    assert!(matches!(result, Err(AppError::Conflict { .. })));
    assert!(!destination.exists());
    Ok(())
}

#[tokio::test]
async fn cancelled_portable_publication_leaves_no_destination_temp_or_storage_mutation()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let cancellation = eutheto_types::CancellationToken::new();
    let mut app_dependencies = dependencies(&directory)?;
    app_dependencies.cancellation = cancellation.clone();
    let app = EuthetoApp::open(app_dependencies).await.boxed()?;
    let scenario_id = create_project(&app, "Cancelled export").await?;
    let before = scenario_view(&app, scenario_id).await?;
    let destination = directory.path().join("cancelled.eutheto");
    cancellation.cancel();
    let result = app
        .execute(AppCommand::ExportScenario {
            cancellation: app.setup_cancellation(),
            scenario_id,
            destination: destination.clone(),
        })
        .await;
    assert!(matches!(
        result,
        Err(AppError::Protocol(failure))
            if failure.code == "operation.cancelled" && !failure.retryable
    ));
    assert!(!destination.exists());
    drop(app);
    let reopened = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    assert_eq!(scenario_view(&reopened, scenario_id).await?, before);
    let names = std::fs::read_dir(directory.path())?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        names
            .iter()
            .all(|name| !name.starts_with(".eutheto-bundle-"))
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_observed_after_portable_inspection_retains_no_preview_or_store_state()
-> Result<(), Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "Inspection cancellation").await?;
    let bytes = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected inspection seed: {other:?}").into()),
    };
    let target_directory = private_tempdir()?;
    let cancellation = eutheto_types::CancellationToken::new();
    let mut target_dependencies = dependencies(&target_directory)?;
    target_dependencies.cancellation = cancellation.clone();
    target_dependencies.ids = Arc::new(CancellingIdGenerator {
        cancellation,
        id: "018f1e2d-3c4b-7a69-8def-012345678980".parse()?,
    });
    let target = EuthetoApp::open(target_dependencies).await.boxed()?;
    let result = target
        .query(AppQuery::PreviewImport {
            cancellation: target.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await;
    assert!(matches!(
        result,
        Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled"
    ));
    drop(target);
    let (store, _) =
        SqliteScenarioStore::open(target_directory.path().join("eutheto.sqlite")).await?;
    let snapshot = store
        .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
        .await?;
    assert_eq!(snapshot.revision, Revision::INITIAL);
    assert!(snapshot.projects.is_empty());
    assert!(snapshot.provenance.is_empty());
    Ok(())
}

#[tokio::test]
async fn cancellation_immediately_before_portable_apply_leaves_store_unmodified()
-> Result<(), Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "Cancelled import").await?;
    let bytes = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected cancellation seed: {other:?}").into()),
    };
    let target_directory = private_tempdir()?;
    let cancellation = eutheto_types::CancellationToken::new();
    let mut target_dependencies = dependencies(&target_directory)?;
    target_dependencies.cancellation = cancellation.clone();
    let target = EuthetoApp::open(target_dependencies).await.boxed()?;
    let preview_id = match target
        .query(AppQuery::PreviewImport {
            cancellation: target.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected cancellation preview: {other:?}").into()),
    };
    cancellation.cancel();
    let result = target
        .execute(AppCommand::ApplyImport {
            cancellation: target.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
        })
        .await;
    assert!(matches!(
        result,
        Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled"
    ));
    drop(target);
    let (store, _) =
        SqliteScenarioStore::open(target_directory.path().join("eutheto.sqlite")).await?;
    let snapshot = store
        .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
        .await?;
    assert_eq!(snapshot.revision, Revision::INITIAL);
    assert!(snapshot.projects.is_empty());
    assert!(snapshot.provenance.is_empty());
    Ok(())
}

// One sequential lifecycle is required to prove revision monotonicity across restart boundaries.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn replace_and_tombstone_reimport_publish_authoritative_monotonic_revisions()
-> Result<(), Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "ABA source").await?;
    for (revision, name) in [
        (Revision::INITIAL, "Source one"),
        (Revision::new(1), "Source two"),
    ] {
        source
            .execute(AppCommand::ApplyScenario {
                request_id: request_id()?,
                envelope: add_entity_envelope(scenario_id, revision, name)?,
                truncate_redo: false,
            })
            .await
            .boxed()?;
    }
    let source_revision_two = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected ABA scenario export: {other:?}").into()),
    };
    let source_backup = match source
        .query(AppQuery::ExportBackup {
            cancellation: source.setup_cancellation(),
            title: "ABA overlap".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected ABA backup export: {other:?}").into()),
    };

    let target_directory = private_tempdir()?;
    let mut target_ids = vec![scenario_id.as_uuid()];
    target_ids.extend(
        [
            "018f1e2d-3c4b-7a69-8def-012345678960",
            "018f1e2d-3c4b-7a69-8def-012345678961",
            "018f1e2d-3c4b-7a69-8def-012345678962",
            "018f1e2d-3c4b-7a69-8def-012345678963",
            "018f1e2d-3c4b-7a69-8def-012345678964",
            "018f1e2d-3c4b-7a69-8def-012345678965",
            "018f1e2d-3c4b-7a69-8def-012345678966",
            "018f1e2d-3c4b-7a69-8def-012345678967",
        ]
        .into_iter()
        .map(str::parse::<Uuid>)
        .collect::<Result<Vec<_>, _>>()?,
    );
    let target = EuthetoApp::open(dependencies_with_fixed_ids(&target_directory, target_ids)?)
        .await
        .boxed()?;
    assert_eq!(create_project(&target, "ABA local").await?, scenario_id);
    for (revision, name) in [
        (Revision::INITIAL, "Local one"),
        (Revision::new(1), "Local two"),
    ] {
        target
            .execute(AppCommand::ApplyScenario {
                request_id: request_id()?,
                envelope: add_entity_envelope(scenario_id, revision, name)?,
                truncate_redo: false,
            })
            .await
            .boxed()?;
    }
    let mut changes = target
        .subscribe(EventTopic::ScenarioChanged)
        .await
        .boxed()?;
    let overlap_preview = match target
        .query(AppQuery::PreviewRestore {
            cancellation: target.setup_cancellation(),
            bytes: source_backup,
            options: ImportOptions {
                restore_mode: RestoreMode::ReplaceLibrary,
                include_results: true,
                include_assets: true,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => {
            assert_eq!(preview.scenarios[0].source_revision, Revision::new(2));
            assert_eq!(
                preview.scenarios[0].same_identity_revision,
                Revision::new(3)
            );
            preview_id
        }
        other => return Err(format!("unexpected ABA overlap preview: {other:?}").into()),
    };
    target
        .execute(AppCommand::ApplyRestore {
            cancellation: target.setup_cancellation(),
            request_id: request_id()?,
            preview_id: overlap_preview,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .boxed()?;
    let overlap_event = changes.recv().await.boxed()?;
    assert!(matches!(
        overlap_event.payload,
        EventPayload::ScenarioChanged { context, change_set }
            if context.scenario_id == Some(scenario_id)
                && context.revision == Some(Revision::new(3))
                && change_set.changes[0].kind == eutheto_types::ChangeKind::Updated
    ));

    target
        .execute(AppCommand::DeleteProject {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(3),
        })
        .await
        .boxed()?;
    let _removed = changes.recv().await.boxed()?;
    let tombstone_preview = match target
        .query(AppQuery::PreviewImport {
            cancellation: target.setup_cancellation(),
            bytes: source_revision_two.clone(),
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => {
            assert_eq!(preview.scenarios[0].source_revision, Revision::new(2));
            assert_eq!(
                preview.scenarios[0].same_identity_revision,
                Revision::new(4)
            );
            assert!(
                preview.scenarios[0]
                    .same_identity_revision_warning
                    .is_some()
            );
            preview_id
        }
        other => return Err(format!("unexpected tombstone preview: {other:?}").into()),
    };
    target
        .execute(AppCommand::ApplyImport {
            cancellation: target.setup_cancellation(),
            request_id: request_id()?,
            preview_id: tombstone_preview,
            collision_plan: CollisionPlan::default(),
        })
        .await
        .boxed()?;
    let reimport_event = changes.recv().await.boxed()?;
    assert!(matches!(
        reimport_event.payload,
        EventPayload::ScenarioChanged { context, change_set }
            if context.revision == Some(Revision::new(4))
                && change_set.changes[0].kind == eutheto_types::ChangeKind::Added
    ));
    let stale = target
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_entity_envelope(scenario_id, Revision::new(2), "Stale ABA")?,
            truncate_redo: false,
        })
        .await;
    assert!(matches!(
        stale,
        Err(AppError::Conflict {
            expected_revision,
            actual_revision,
        }) if expected_revision == Revision::new(2) && actual_revision == Revision::new(4)
    ));
    drop(changes);
    drop(target);

    let reopened = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    assert_eq!(
        scenario_view(&reopened, scenario_id).await?.revision,
        Revision::new(4)
    );
    for (revision, name) in [
        (Revision::new(2), "Source three"),
        (Revision::new(3), "Source four"),
        (Revision::new(4), "Source five"),
    ] {
        source
            .execute(AppCommand::ApplyScenario {
                request_id: request_id()?,
                envelope: add_entity_envelope(scenario_id, revision, name)?,
                truncate_redo: false,
            })
            .await
            .boxed()?;
    }
    let source_revision_five = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected high ABA export: {other:?}").into()),
    };
    reopened
        .execute(AppCommand::DeleteProject {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(4),
        })
        .await
        .boxed()?;
    let high_preview = match reopened
        .query(AppQuery::PreviewImport {
            cancellation: reopened.setup_cancellation(),
            bytes: source_revision_five,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => {
            assert_eq!(preview.scenarios[0].source_revision, Revision::new(5));
            assert_eq!(
                preview.scenarios[0].same_identity_revision,
                Revision::new(5)
            );
            assert!(
                preview.scenarios[0]
                    .same_identity_revision_warning
                    .is_none()
            );
            preview_id
        }
        other => return Err(format!("unexpected high ABA preview: {other:?}").into()),
    };
    reopened
        .execute(AppCommand::ApplyImport {
            cancellation: reopened.setup_cancellation(),
            request_id: request_id()?,
            preview_id: high_preview,
            collision_plan: CollisionPlan::default(),
        })
        .await
        .boxed()?;
    assert_eq!(
        scenario_view(&reopened, scenario_id).await?.revision,
        Revision::new(5)
    );
    reopened
        .execute(AppCommand::DeleteProject {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(5),
        })
        .await
        .boxed()?;
    drop(reopened);
    let fresh_id: Uuid = "018f1e2d-3c4b-7a69-8def-012345678970".parse()?;
    let duplicate_id: Uuid = "018f1e2d-3c4b-7a69-8def-012345678971".parse()?;
    let nested_id: Uuid = "018f1e2d-3c4b-7a69-8def-012345678972".parse()?;
    let creator = EuthetoApp::open(dependencies_with_fixed_ids(
        &target_directory,
        [scenario_id.as_uuid(), fresh_id],
    )?)
    .await
    .boxed()?;
    let created_id = create_project(&creator, "Skip tombstone").await?;
    assert_eq!(created_id.as_uuid(), fresh_id);
    creator
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_entity_envelope(created_id, Revision::INITIAL, "Nested identity")?,
            truncate_redo: false,
        })
        .await
        .boxed()?;
    drop(creator);
    let duplicator = EuthetoApp::open(dependencies_with_fixed_ids(
        &target_directory,
        [duplicate_id, scenario_id.as_uuid(), nested_id],
    )?)
    .await
    .boxed()?;
    let duplicate = duplicator
        .execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            source_id: created_id,
            expected_revision: Revision::new(1),
            title: "Skip nested tombstone".to_owned(),
        })
        .await
        .boxed()?;
    assert!(matches!(
        duplicate,
        AppCommandResult::Project(project) if project.scenario_id.as_uuid() == duplicate_id
    ));
    Ok(())
}

// This fixture seeds one complete cross-section identity closure atomically for its consumers.
#[allow(clippy::too_many_lines)]
async fn seed_local_identity_closure(
    directory: &TempDir,
) -> Result<(ScenarioId, Revision, Vec<String>), Box<dyn Error>> {
    let source = EuthetoApp::open(dependencies(directory)?).await.boxed()?;
    let scenario_id = create_project(&source, "Identity closure").await?;
    let (seed_bytes, local_library_revision) = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle {
            bytes,
            library_revision,
            ..
        } => (bytes, library_revision),
        other => return Err(format!("unexpected identity seed export: {other:?}").into()),
    };
    let seed = inspect_bundle(
        &seed_bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let entity_id = "018f1e2d-3c4b-7a69-8def-012345678940";
    let nested_id = "018f1e2d-3c4b-7a69-8def-012345678941";
    let semantic_id = "018f1e2d-3c4b-7a69-8def-012345678942";
    let historical_id = "018f1e2d-3c4b-7a69-8def-012345678943";
    let result_id = "018f1e2d-3c4b-7a69-8def-012345678944";
    let capability = SemanticCapability {
        id: "vendor.current-owned".to_owned(),
        version: 1,
    };
    let mut current = seed.scenarios[0].clone();
    current.revision = Revision::new(7);
    current.project = Some(PortableProjectMetadata { archived_at: None });
    current.document.domain.entities.insert(
        entity_id.parse()?,
        json!({
            "id": entity_id,
            "nested": BTreeMap::from([(
                nested_id.to_owned(),
                json!({"id": nested_id}),
            )]),
        }),
    );
    current.required_capabilities.insert(capability);
    current.semantic_extensions.insert(
        "vendor.current-owned".to_owned(),
        json!(BTreeMap::from([(
            semantic_id.to_owned(),
            json!({"id": semantic_id}),
        )])),
    );
    let mut historical = current.clone();
    historical.revision = Revision::INITIAL;
    historical.project = None;
    historical.required_capabilities.clear();
    historical.semantic_extensions.clear();
    historical
        .document
        .domain
        .entities
        .insert(historical_id.parse()?, json!({"id": historical_id}));
    let result = serde_json::to_vec(&json!({
        "resultId": result_id,
        "scenarioId": scenario_id,
        "scenarioRevision": historical.revision,
        "result": {},
    }))?;
    let staged = StagedImport {
        binding: PreviewBinding {
            file_sha256: eutheto_export::sha256_hex(&seed_bytes),
            options_sha256: "trusted-identity-fixture".to_owned(),
            local_library_revision,
            format_version: 1,
            schema_version: 1,
        },
        mode: RestoreMode::ImportScenario,
        scenarios: vec![StagedScenario {
            original_id: scenario_id,
            source_revision: current.revision,
            disposition: StagedDisposition::Replace,
            scenario: current,
            id_remap: BTreeMap::new(),
        }],
        scenario_revisions: vec![historical],
        reference_remap: BTreeMap::new(),
        results: BTreeMap::from([(result_id.to_owned(), result)]),
        shared_records: BTreeMap::new(),
        preferences: BTreeMap::new(),
        manifest_extensions: BTreeMap::new(),
        nonsemantic_extensions: BTreeSet::new(),
        assets: BTreeMap::new(),
        supplemental_replacements: BTreeSet::new(),
        provenance: ImportProvenance {
            source_bundle_id: seed.manifest.bundle_id,
            source_application: seed.manifest.application,
            source_created_at: seed.manifest.created_at.parse()?,
            original_format_version: 1,
            original_schema_version: 1,
            source_file_sha256: eutheto_export::sha256_hex(&seed_bytes),
            applied_migrations: Vec::new(),
        },
    };
    let (store, _) = SqliteScenarioStore::open(directory.path().join("eutheto.sqlite")).await?;
    store
        .apply_staged_library(
            StagedLibraryApply::Import(staged),
            timestamp("2026-01-10T12:00:01Z")?,
            eutheto_types::CancellationToken::new(),
        )
        .await?;
    let revision = store.get_project(scenario_id).await?.summary.revision;
    Ok((
        scenario_id,
        revision,
        [nested_id, semantic_id, historical_id, result_id]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    ))
}

fn collision_scenario_bundle(
    seed: &InspectedBundle,
    collision_id: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut scenario = seed.scenarios[0].clone();
    scenario
        .document
        .domain
        .entities
        .insert(collision_id.parse()?, json!({"id": collision_id}));
    Ok(assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: seed.manifest.bundle_id,
            created_at: seed.manifest.created_at.clone(),
            application: seed.manifest.application.clone(),
            title: "Identity collision".to_owned(),
            scenario,
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        },
        &portable_encode::encode_fixture_domain,
    )?)
}

#[tokio::test]
async fn preview_detects_local_nested_semantic_historical_and_result_identities()
-> Result<(), Box<dyn Error>> {
    let target_directory = private_tempdir()?;
    let (_, _, collision_ids) = seed_local_identity_closure(&target_directory).await?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;

    let import_directory = private_tempdir()?;
    let import_source = EuthetoApp::open(dependencies(&import_directory)?)
        .await
        .boxed()?;
    let import_id = create_project(&import_source, "Imported identity").await?;
    let seed_bytes = match import_source
        .query(AppQuery::ExportScenario {
            scenario_id: import_id,
            cancellation: import_source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected collision seed export: {other:?}").into()),
    };
    let seed = inspect_bundle(
        &seed_bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    for collision_id in collision_ids {
        let result = target
            .query(AppQuery::PreviewImport {
                cancellation: target.setup_cancellation(),
                bytes: collision_scenario_bundle(&seed, &collision_id)?,
                options: ImportOptions {
                    restore_mode: RestoreMode::ImportScenario,
                    include_results: false,
                    include_assets: false,
                },
            })
            .await
            .boxed()?;
        assert!(matches!(
            result,
            AppQueryResult::PortablePreview { preview, .. }
                if preview.scenarios.len() == 1 && preview.scenarios[0].collides
        ));
    }
    Ok(())
}

async fn seed_identity_closure_library(
    directory: &TempDir,
) -> Result<(ScenarioId, Revision), Box<dyn Error>> {
    let (source_id, source_revision, _) = seed_local_identity_closure(directory).await?;
    Ok((source_id, source_revision))
}
fn identity_fixture_inspection_policy() -> InspectionPolicy {
    let mut policy = InspectionPolicy::default();
    policy
        .supported_capabilities
        .insert("vendor.current-owned".to_owned(), 1);
    policy
}

#[tokio::test]
async fn duplicate_uses_injected_ids_for_the_complete_owned_graph() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let (source_id, source_revision) = seed_identity_closure_library(&directory).await?;
    let replacements = [
        "018f1e2d-3c4b-7a69-8def-012345678950",
        "018f1e2d-3c4b-7a69-8def-012345678951",
        "018f1e2d-3c4b-7a69-8def-012345678952",
        "018f1e2d-3c4b-7a69-8def-012345678953",
        "018f1e2d-3c4b-7a69-8def-012345678954",
    ]
    .into_iter()
    .map(str::parse::<Uuid>)
    .collect::<Result<Vec<_>, _>>()?;
    let app = EuthetoApp::open(dependencies_with_fixed_ids(
        &directory,
        replacements.iter().copied(),
    )?)
    .await
    .boxed()?;
    let duplicate = app
        .execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            source_id,
            expected_revision: source_revision,
            title: "Deterministic duplicate".to_owned(),
        })
        .await
        .boxed()?;
    let duplicate_id = match duplicate {
        AppCommandResult::Project(project) => project.scenario_id,
        other => return Err(format!("unexpected duplicate result: {other:?}").into()),
    };
    assert_eq!(duplicate_id.as_uuid(), replacements[0]);
    let bytes = match app
        .query(AppQuery::ExportScenario {
            scenario_id: duplicate_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected duplicate export: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &bytes,
        &identity_fixture_inspection_policy(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let exported_owned = std::iter::once(&inspected.scenarios[0])
        .chain(inspected.scenario_revisions.iter())
        .flat_map(collect_scenario_owned_uuids)
        .collect::<BTreeSet<_>>();
    assert_eq!(exported_owned, replacements[..4].iter().copied().collect());
    Ok(())
}

#[tokio::test]
async fn duplicate_identity_collision_and_exhaustion_are_safe_and_do_not_mutate()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let (source_id, source_revision) = seed_identity_closure_library(&directory).await?;
    let collision_app = EuthetoApp::open(dependencies_with_fixed_ids(
        &directory,
        vec![source_id.as_uuid(); 64],
    )?)
    .await
    .boxed()?;
    let collision = collision_app
        .execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            source_id,
            expected_revision: source_revision,
            title: "Collision".to_owned(),
        })
        .await;
    assert!(matches!(
        collision,
        Err(AppError::Protocol(failure)) if failure.code == "identity.collision_exhausted"
    ));
    drop(collision_app);

    let exhausted_app =
        EuthetoApp::open(dependencies_with_fixed_ids(&directory, Vec::<Uuid>::new())?)
            .await
            .boxed()?;
    let exhausted = exhausted_app
        .execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            source_id,
            expected_revision: source_revision,
            title: "Exhausted".to_owned(),
        })
        .await;
    assert!(matches!(
        exhausted,
        Err(AppError::Protocol(failure)) if failure.code == "identity.generation_failed"
    ));
    assert!(matches!(
        exhausted_app
            .query(AppQuery::ListProjects(ProjectScope::All))
            .await
            .boxed()?,
        AppQueryResult::Projects(projects) if projects.len() == 1
    ));
    Ok(())
}

#[tokio::test]
async fn portable_preview_binds_bundle_kind_before_retaining_state() -> Result<(), Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "Kind-bound export").await.boxed()?;
    let scenario_export = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected scenario export: {other:?}").into()),
    };
    let (full_backup, _) = create_inspected_backup().await?;

    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    assert!(matches!(
        target
            .query(AppQuery::PreviewRestore {
                cancellation: target.setup_cancellation(),
                bytes: scenario_export,
                options: ImportOptions {
                    restore_mode: RestoreMode::AddBackup,
                    include_results: true,
                    include_assets: true,
                },
            })
            .await,
        Err(AppError::Validation(report))
            if report.issues.iter().any(|issue| issue.code == "portable.bundle_kind_invalid")
    ));
    assert!(matches!(
        target
            .query(AppQuery::PreviewImport {
                cancellation: target.setup_cancellation(),
                bytes: full_backup,
                options: ImportOptions {
                    restore_mode: RestoreMode::ImportScenario,
                    include_results: false,
                    include_assets: false,
                },
            })
            .await,
        Err(AppError::Validation(report))
            if report.issues.iter().any(|issue| issue.code == "portable.bundle_kind_invalid")
    ));
    Ok(())
}

async fn backup_with_large_preview_titles() -> Result<Vec<u8>, Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "Preview budget seed").await?;
    let seed_bytes = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected scenario export: {other:?}").into()),
    };
    let mut seed = inspect_bundle(
        &seed_bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let mut scenario = seed.scenarios.remove(0);
    scenario.project = Some(PortableProjectMetadata::default());
    // Each title is below the 1 MiB string limit. Deterministic high-entropy
    // text also stays below the archive compression-ratio ceiling.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    scenario.document.metadata.title = (0..900 * 1024)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(b'a' + state.to_le_bytes()[4] % 26)
        })
        .collect();
    let scenarios = (0..40)
        .map(|index| {
            let mut scenario = scenario.clone();
            scenario.document.scenario_id = ScenarioId::from_uuid(Uuid::from_u128(
                0x018f_1e2d_3c4b_7a69_8def_0123_4567_8000 + index,
            ));
            scenario
        })
        .collect();
    let bytes = assemble_full_backup(
        &FullBackupSnapshot {
            bundle_id: seed.manifest.bundle_id,
            created_at: seed.manifest.created_at,
            application: seed.manifest.application,
            title: "Derived preview memory".to_owned(),
            scenarios,
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: seed.manifest.nonsemantic_extensions,
            manifest_extensions: complete_full_backup_extensions()?,
        },
        &portable_encode::encode_fixture_domain,
    )?;
    Ok(bytes)
}

#[tokio::test]
async fn portable_preview_rejects_derived_titles_exceeding_retained_budget_without_writes()
-> Result<(), Box<dyn Error>> {
    let bytes = backup_with_large_preview_titles().await?;
    let inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let title_bytes: usize = inspected
        .scenarios
        .iter()
        .map(|scenario| scenario.document.metadata.title.len())
        .sum();
    assert!(inspected.retained_memory_bytes < 64 * 1024 * 1024);
    assert!(inspected.retained_memory_bytes + title_bytes > 64 * 1024 * 1024);
    drop(inspected);

    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let result = target
        .query(AppQuery::PreviewRestore {
            cancellation: target.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::AddBackup,
                include_results: true,
                include_assets: true,
            },
        })
        .await;
    assert!(matches!(
        result,
        Err(AppError::Protocol(failure)) if failure.code == "portable.preview_too_large"
    ));
    drop(target);
    let (store, _) =
        SqliteScenarioStore::open(target_directory.path().join("eutheto.sqlite")).await?;
    let snapshot = store
        .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
        .await?;
    assert_eq!(snapshot.revision, Revision::INITIAL);
    assert!(snapshot.projects.is_empty());
    assert!(snapshot.provenance.is_empty());
    Ok(())
}

async fn create_inspected_backup() -> Result<(Vec<u8>, ScenarioId), Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let restored_id = create_project(&source, "Restored").await.boxed()?;
    let add_command = add_entity_envelope(restored_id, Revision::INITIAL, "Restored person")?;
    source
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_command,
            truncate_redo: false,
        })
        .await
        .boxed()?;
    source
        .execute(AppCommand::ArchiveProject {
            request_id: request_id()?,
            scenario_id: restored_id,
            expected_revision: Revision::new(1),
        })
        .await
        .boxed()?;
    source
        .execute(AppCommand::SetSetting {
            expected_library_revision: source
                .application_settings_snapshot()
                .await
                .boxed()?
                .library_revision,
            request_id: request_id()?,
            key: "appearance".to_owned(),
            value: json!({"theme": "dark", "reducedMotion": true}),
        })
        .await
        .boxed()?;
    let backup = match source
        .query(AppQuery::ExportBackup {
            cancellation: source.setup_cancellation(),
            title: "Library backup".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected backup result: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &backup,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert_eq!(inspected.scenarios.len(), 1);
    assert_eq!(inspected.scenarios[0].revision, Revision::new(1));
    assert!(
        inspected.scenarios[0]
            .project
            .as_ref()
            .is_some_and(|project| project.archived_at.is_some())
    );
    Ok((backup, restored_id))
}
async fn preview_restore(app: &EuthetoApp, bytes: Vec<u8>) -> Result<RequestId, Box<dyn Error>> {
    match app
        .query(AppQuery::PreviewRestore {
            cancellation: app.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ReplaceLibrary,
                include_results: true,
                include_assets: true,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview { preview_id, .. } => Ok(preview_id),
        other => Err(format!("unexpected restore preview: {other:?}").into()),
    }
}

async fn preview_add_backup(
    app: &EuthetoApp,
    bytes: Vec<u8>,
) -> Result<(RequestId, eutheto_import::ImportPreview), Box<dyn Error>> {
    match app
        .query(AppQuery::PreviewRestore {
            cancellation: app.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::AddBackup,
                include_results: true,
                include_assets: true,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => Ok((preview_id, *preview)),
        other => Err(format!("unexpected add-backup preview: {other:?}").into()),
    }
}
async fn preview_replace_and_assert_removal(
    target: &EuthetoApp,
    backup: Vec<u8>,
    removed_id: ScenarioId,
) -> Result<RequestId, Box<dyn Error>> {
    match target
        .query(AppQuery::PreviewRestore {
            cancellation: target.setup_cancellation(),
            bytes: backup,
            options: ImportOptions {
                restore_mode: RestoreMode::ReplaceLibrary,
                include_results: true,
                include_assets: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => {
            assert_eq!(preview.removed_scenarios.len(), 1);
            assert_eq!(preview.removed_scenarios[0].scenario_id, removed_id);
            assert_eq!(preview.removed_scenarios[0].title, "Removed by restore");
            assert_eq!(preview.removed_scenarios[0].revision, Revision::INITIAL);
            assert!(!preview.removed_scenarios[0].archived);
            assert_eq!(preview.settings_changed, vec!["appearance".to_owned()]);
            assert_eq!(preview.settings_removed, vec!["locale".to_owned()]);
            Ok(preview_id)
        }
        other => Err(format!("unexpected restore preview: {other:?}").into()),
    }
}

async fn apply_restore_and_assert_events(
    target: &EuthetoApp,
    preview_id: RequestId,
    removed_id: ScenarioId,
    restored_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let mut changed = target
        .subscribe(EventTopic::ScenarioChanged)
        .await
        .boxed()?;
    let mut refreshed = target
        .subscribe(EventTopic::AppNotification)
        .await
        .boxed()?;
    let restore_request_id = request_id()?;
    let applied = target
        .execute(AppCommand::ApplyRestore {
            cancellation: target.setup_cancellation(),
            request_id: restore_request_id,
            preview_id,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .boxed()?;
    assert!(matches!(
        applied,
        AppCommandResult::PortableApplied { scenarios, .. }
            if scenarios.len() == 1
                && scenarios[0].source_scenario_id == restored_id
                && scenarios[0].scenario_id == restored_id
    ));
    let refresh = refreshed.recv().await.boxed()?;
    assert!(matches!(
        refresh.payload,
        EventPayload::AppNotification { context, code, .. }
            if context.request_id == Some(restore_request_id)
                && code == "library.refreshed"
    ));
    let events = [changed.recv().await.boxed()?, changed.recv().await.boxed()?];
    let affected_ids = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ScenarioChanged { context, .. }
                if context.request_id == Some(restore_request_id) =>
            {
                context.scenario_id
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        affected_ids,
        [removed_id, restored_id].into_iter().collect()
    );
    Ok(())
}

async fn assert_restored_backup(
    target: &EuthetoApp,
    target_directory: &TempDir,
    removed_id: ScenarioId,
    restored_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    assert!(matches!(
        target.query(AppQuery::ProjectMetadata(removed_id)).await,
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        target
            .query(AppQuery::ProjectMetadata(restored_id))
            .await
            .boxed()?,
        AppQueryResult::Project(_)
    ));
    let restored = scenario_view(target, restored_id).await.boxed()?;
    assert_eq!(restored.revision, Revision::new(1));
    assert!(matches!(
        target.query(AppQuery::Setting("appearance".to_owned())).await.boxed()?,
        AppQueryResult::Setting(Some(setting))
            if setting.value == json!({"theme": "dark", "reducedMotion": true})
    ));
    assert!(matches!(
        target
            .query(AppQuery::Setting("locale".to_owned()))
            .await
            .boxed()?,
        AppQueryResult::Setting(None)
    ));
    let restored_export = match target
        .query(AppQuery::ExportScenario {
            scenario_id: restored_id,
            cancellation: target.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected restored export result: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &restored_export,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert_eq!(inspected.scenarios.len(), 1);
    assert_eq!(inspected.scenarios[0].revision, Revision::new(1));
    assert!(
        std::fs::read_dir(target_directory.path().join("backups"))?
            .next()
            .transpose()?
            .is_some()
    );
    Ok(())
}

async fn restore_backup_and_assert(
    backup: Vec<u8>,
    restored_id: ScenarioId,
) -> Result<(), Box<dyn Error>> {
    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let removed_id = create_project(&target, "Removed by restore")
        .await
        .boxed()?;
    target
        .execute(AppCommand::SetSetting {
            expected_library_revision: target
                .application_settings_snapshot()
                .await
                .boxed()?
                .library_revision,
            request_id: request_id()?,
            key: "locale".to_owned(),
            value: json!("en-GB"),
        })
        .await
        .boxed()?;
    let preview_id = preview_replace_and_assert_removal(&target, backup, removed_id).await?;
    apply_restore_and_assert_events(&target, preview_id, removed_id, restored_id).await?;
    assert_restored_backup(&target, &target_directory, removed_id, restored_id).await
}

#[tokio::test]
async fn backup_audit_selection_is_explicitly_deferred_and_never_silently_omitted()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let scenario_id = create_project(&app, "Audit exclusion").await.boxed()?;
    let envelope = add_entity_envelope(scenario_id, Revision::INITIAL, "Journalled person")?;
    let command_id = envelope.command_id;
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope,
        truncate_redo: false,
    })
    .await
    .boxed()?;

    let bytes = match app
        .query(AppQuery::ExportBackup {
            cancellation: app.setup_cancellation(),
            title: "No audit".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::IncludeAll,
                include_audit: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected backup result: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    for path in inspected.checksums.files.keys() {
        let path = path.to_ascii_lowercase();
        assert!(!path.contains("audit"));
        assert!(!path.contains("command"));
        assert!(!path.contains("history"));
        assert!(!path.contains("journal"));
    }
    let scenario_bytes = serde_json::to_vec(&inspected.scenarios)?;
    let command_id = command_id.to_string();
    assert!(
        !scenario_bytes
            .windows(command_id.len())
            .any(|window| window == command_id.as_bytes())
    );
    assert!(inspected.additional_entries.values().all(|content| {
        !content
            .windows(command_id.len())
            .any(|window| window == command_id.as_bytes())
    }));
    assert!(matches!(
        app.query(AppQuery::ExportBackup {
            cancellation: app.setup_cancellation(),
            title: "Audit requested".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::IncludeAll,
                include_audit: true,
            },
        })
        .await,
        Err(AppError::Unsupported(feature)) if feature.code == "backup.audit_unavailable"
    ));
    Ok(())
}

async fn restore_large_asset_fixture() -> Result<(EuthetoApp, TempDir, ScenarioId), Box<dyn Error>>
{
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "Large asset").await.boxed()?;
    let scenario_bundle = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected scenario export: {other:?}").into()),
    };
    let mut seed = inspect_bundle(
        &scenario_bundle,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let mut scenario = seed.scenarios.remove(0);
    scenario.extensions.insert(
        "vendor.asset-reference".to_owned(),
        json!({"assetKey": "large.txt"}),
    );
    scenario.project = Some(PortableProjectMetadata::default());
    let mut sections = BackupSections::default();
    let mut large_text = Vec::with_capacity(PORTABLE_LARGE_ASSET_BYTES_V1 + 1);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    while large_text.len() < PORTABLE_LARGE_ASSET_BYTES_V1 + 1 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        large_text.extend_from_slice(format!("{state:016x}").as_bytes());
    }
    large_text.truncate(PORTABLE_LARGE_ASSET_BYTES_V1 + 1);
    sections.assets.insert(
        "large.txt".to_owned(),
        PortableAsset {
            bytes: large_text,
            media_type: "text/plain; charset=utf-8".to_owned(),
            redistribution_permitted: true,
        },
    );
    let restore_bundle = assemble_full_backup(
        &FullBackupSnapshot {
            bundle_id: seed.manifest.bundle_id,
            created_at: seed.manifest.created_at,
            application: seed.manifest.application,
            title: "Large asset restore".to_owned(),
            scenarios: vec![scenario],
            scenario_revisions: Vec::new(),
            sections,
            nonsemantic_extensions: seed.manifest.nonsemantic_extensions,
            manifest_extensions: complete_full_backup_extensions()?,
        },
        &portable_encode::encode_fixture_domain,
    )?;

    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let options = ImportOptions {
        restore_mode: RestoreMode::AddBackup,
        include_results: true,
        include_assets: true,
    };
    let preview_id = match target
        .query(AppQuery::PreviewRestore {
            cancellation: target.setup_cancellation(),
            bytes: restore_bundle,
            options,
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected restore preview: {other:?}").into()),
    };
    target
        .execute(AppCommand::ApplyRestore {
            cancellation: target.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: false,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .boxed()?;
    Ok((target, target_directory, scenario_id))
}

// The shared helper exercises both restore modes and every placeholder re-export invariant.
#[allow(clippy::too_many_lines)]
async fn assert_omitted_placeholder_survives_restore(
    backup: &[u8],
    restore_mode: RestoreMode,
    expected_placeholder: &[u8],
) -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    if restore_mode == RestoreMode::ReplaceLibrary {
        create_project(&app, "Removed during placeholder restore").await?;
    }
    let preview_id = match app
        .query(AppQuery::PreviewRestore {
            cancellation: app.setup_cancellation(),
            bytes: backup.to_vec(),
            options: ImportOptions {
                restore_mode,
                include_results: true,
                include_assets: true,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => {
            assert!(
                preview
                    .source_backup_selection
                    .as_ref()
                    .is_some_and(|selection| {
                        selection.asset_selection
                            == eutheto_export::PortableBackupAssetSelection::V1Threshold
                            && selection.fixed_exclusions == all_fixed_exclusions()
                            && selection.excluded_asset_ids.contains("large.txt")
                    })
            );
            assert!(preview.omitted_assets.contains_key("large.txt"));
            assert!(
                preview
                    .excluded_sections
                    .contains("assets-above-v1-threshold")
            );
            preview_id
        }
        other => return Err(format!("unexpected placeholder restore preview: {other:?}").into()),
    };
    app.execute(AppCommand::ApplyRestore {
        cancellation: app.setup_cancellation(),
        request_id: request_id()?,
        preview_id,
        collision_plan: CollisionPlan::default(),
        authorization: RestoreAuthorization {
            destructive_action_confirmed: restore_mode == RestoreMode::ReplaceLibrary,
            safety_backup: SafetyBackupEvidence::NotRequired,
            prospective_failure_receipt_token: None,
            collision_plan_sha256: None,
        },
    })
    .await
    .boxed()?;
    for assets in [
        BackupAssetSelection::IncludeAll,
        BackupAssetSelection::IncludeUnderThreshold,
    ] {
        let exported = match app
            .query(AppQuery::ExportBackup {
                cancellation: app.setup_cancellation(),
                title: "Placeholder retained".to_owned(),
                selection: BackupSelection {
                    include_results: true,
                    assets,
                    include_audit: false,
                },
            })
            .await
            .boxed()?
        {
            AppQueryResult::BackupBundle { bytes, summary, .. } => {
                assert_eq!(summary.excluded_asset_count, 1);
                assert_eq!(summary.fixed_exclusions, all_fixed_exclusions());
                assert_eq!(summary.excluded_asset_ids, vec!["large.txt".to_owned()]);
                bytes
            }
            other => return Err(format!("unexpected placeholder re-export: {other:?}").into()),
        };
        let inspected = inspect_bundle(
            &exported,
            &InspectionPolicy::default(),
            &MigrationRegistries::default(),
            &portable_decode::decode_fixture_domain,
        )?;
        assert_eq!(
            inspected
                .additional_entries
                .get("assets/large.txt")
                .map(Vec::as_slice),
            Some(expected_placeholder)
        );
        let selection = backup_selection_from_manifest(&inspected.manifest)?
            .ok_or("re-export selection metadata is missing")?;
        assert_eq!(selection.fixed_exclusions, all_fixed_exclusions());
        assert_eq!(
            selection.excluded_asset_ids,
            BTreeSet::from(["large.txt".to_owned()])
        );
    }
    Ok(())
}

fn assert_threshold_placeholder(
    excluded: &InspectedBundle,
    original_asset: &[u8],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let selection = backup_selection_from_manifest(&excluded.manifest)?
        .ok_or("excluded backup selection metadata is missing")?;
    assert!(selection.include_results);
    assert_eq!(selection.fixed_exclusions, all_fixed_exclusions());
    assert_eq!(
        selection.excluded_asset_ids,
        BTreeSet::from(["large.txt".to_owned()])
    );
    assert_eq!(selection.threshold_version, Some(1));
    assert_eq!(
        selection.threshold_bytes,
        Some(u64::try_from(PORTABLE_LARGE_ASSET_BYTES_V1)?)
    );
    let placeholder_bytes = excluded
        .additional_entries
        .get("assets/large.txt")
        .ok_or("omitted-asset placeholder is missing")?;
    assert_ne!(placeholder_bytes, original_asset);
    let metadata = excluded
        .manifest
        .asset_metadata
        .get("large.txt")
        .ok_or("omitted-asset metadata is missing")?;
    let placeholder = parse_omitted_asset_placeholder(&PortableAsset {
        bytes: placeholder_bytes.clone(),
        media_type: metadata.media_type.clone(),
        redistribution_permitted: metadata.redistribution_permitted,
    })?
    .ok_or("omitted asset was not recognized as a placeholder")?;
    assert_eq!(
        placeholder.content_sha256,
        eutheto_export::sha256_hex(original_asset)
    );
    Ok(placeholder_bytes.clone())
}

#[tokio::test]
async fn large_asset_exclusion_is_explicit_and_distinct_from_including_assets()
-> Result<(), Box<dyn Error>> {
    let (target, _target_directory, _) = restore_large_asset_fixture().await?;
    let included_bytes = match target
        .query(AppQuery::ExportBackup {
            cancellation: target.setup_cancellation(),
            title: "Included".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::IncludeAll,
                include_audit: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, summary, .. } => {
            assert_eq!(summary.excluded_asset_count, 0);
            assert!(summary.excluded_asset_ids.is_empty());
            bytes
        }
        other => return Err(format!("unexpected included backup: {other:?}").into()),
    };
    let included = inspect_bundle(
        &included_bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let original_asset = included
        .additional_entries
        .get("assets/large.txt")
        .ok_or("included large asset is missing")?;

    let excluded_bytes = match target
        .query(AppQuery::ExportBackup {
            cancellation: target.setup_cancellation(),
            title: "Excluded".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::IncludeUnderThreshold,
                include_audit: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, summary, .. } => {
            assert_eq!(summary.excluded_asset_count, 1);
            assert_eq!(summary.excluded_asset_ids, vec!["large.txt".to_owned()]);
            assert_eq!(
                summary.exclusion_scope.as_deref(),
                Some("assets-above-version-1-threshold")
            );
            bytes
        }
        other => return Err(format!("unexpected excluded backup: {other:?}").into()),
    };
    let excluded = inspect_bundle(
        &excluded_bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let placeholder_bytes = assert_threshold_placeholder(&excluded, original_asset)?;
    assert_omitted_placeholder_survives_restore(
        &excluded_bytes,
        RestoreMode::AddBackup,
        &placeholder_bytes,
    )
    .await?;
    assert_omitted_placeholder_survives_restore(
        &excluded_bytes,
        RestoreMode::ReplaceLibrary,
        &placeholder_bytes,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn exclude_all_assets_emits_reconnection_placeholders_for_referenced_assets()
-> Result<(), Box<dyn Error>> {
    let (target, _target_directory, _) = restore_large_asset_fixture().await?;
    let bytes = match target
        .query(AppQuery::ExportBackup {
            cancellation: target.setup_cancellation(),
            title: "Exclude all assets".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::ExcludeAll,
                include_audit: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, summary, .. } => {
            assert_eq!(summary.excluded_asset_count, 1);
            assert_eq!(summary.excluded_asset_ids, vec!["large.txt".to_owned()]);
            assert_eq!(summary.exclusion_scope.as_deref(), Some("all-assets"));
            bytes
        }
        other => return Err(format!("unexpected exclude-all backup: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let selection = backup_selection_from_manifest(&inspected.manifest)?
        .ok_or("exclude-all backup selection metadata is missing")?;
    assert_eq!(selection.fixed_exclusions, all_fixed_exclusions());
    assert!(selection.threshold_version.is_none());
    assert!(selection.threshold_bytes.is_none());
    let metadata = inspected
        .manifest
        .asset_metadata
        .get("large.txt")
        .ok_or("exclude-all placeholder metadata is missing")?;
    let placeholder = parse_omitted_asset_placeholder(&PortableAsset {
        bytes: inspected
            .additional_entries
            .get("assets/large.txt")
            .ok_or("exclude-all placeholder is missing")?
            .clone(),
        media_type: metadata.media_type.clone(),
        redistribution_permitted: metadata.redistribution_permitted,
    })?
    .ok_or("exclude-all asset was not recognized as a placeholder")?;
    assert_eq!(
        placeholder.reason,
        eutheto_export::OmittedAssetReason::ExcludeAll
    );
    Ok(())
}

#[tokio::test]
async fn backup_selection_metadata_distinguishes_empty_results_from_excluded_results()
-> Result<(), Box<dyn Error>> {
    let (target, _target_directory, _) = restore_large_asset_fixture().await?;
    for include_results in [true, false] {
        let bytes = match target
            .query(AppQuery::ExportBackup {
                cancellation: target.setup_cancellation(),
                title: "Result selection".to_owned(),
                selection: BackupSelection {
                    include_results,
                    assets: BackupAssetSelection::IncludeAll,
                    include_audit: false,
                },
            })
            .await
            .boxed()?
        {
            AppQueryResult::BackupBundle { bytes, summary, .. } => {
                assert_eq!(summary.include_results, include_results);
                bytes
            }
            other => return Err(format!("unexpected result-selection backup: {other:?}").into()),
        };
        let inspected = inspect_bundle(
            &bytes,
            &InspectionPolicy::default(),
            &MigrationRegistries::default(),
            &portable_decode::decode_fixture_domain,
        )?;
        assert_eq!(inspected.manifest.counts.results, 0);
        assert_eq!(
            backup_selection_from_manifest(&inspected.manifest)?
                .ok_or("backup selection metadata is missing")?
                .include_results,
            include_results
        );
    }
    Ok(())
}

fn assert_scenario_placeholder_selection(
    inspected: &InspectedBundle,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let selection = backup_selection_from_manifest(&inspected.manifest)?
        .ok_or("scenario placeholder selection metadata is missing")?;
    assert_eq!(
        selection.scope,
        eutheto_export::BackupSelectionScope::Scenario
    );
    assert_eq!(selection.fixed_exclusions, all_fixed_exclusions());
    assert_eq!(
        selection.asset_selection,
        eutheto_export::PortableBackupAssetSelection::All
    );
    assert_eq!(
        selection.excluded_asset_ids,
        BTreeSet::from(["large.txt".to_owned()])
    );
    Ok(inspected
        .additional_entries
        .get("assets/large.txt")
        .ok_or("scenario placeholder payload is missing")?
        .clone())
}

#[allow(
    clippy::too_many_lines,
    reason = "Keeps the backup/export/re-import preservation path in one regression."
)]
#[tokio::test]
async fn scenario_export_and_reimport_preserve_omitted_asset_reconnection_metadata()
-> Result<(), Box<dyn Error>> {
    let (source, _source_directory, scenario_id) = restore_large_asset_fixture().await?;
    let backup = match source
        .query(AppQuery::ExportBackup {
            cancellation: source.setup_cancellation(),
            title: "Scenario placeholder source".to_owned(),
            selection: BackupSelection {
                include_results: true,
                assets: BackupAssetSelection::IncludeUnderThreshold,
                include_audit: false,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected placeholder source backup: {other:?}").into()),
    };
    let restored_directory = private_tempdir()?;
    let restored = EuthetoApp::open(dependencies(&restored_directory)?)
        .await
        .boxed()?;
    let (preview_id, _) = preview_add_backup(&restored, backup).await?;
    apply_restore_with_authorization(
        &restored,
        preview_id,
        CollisionPlan::default(),
        SafetyBackupEvidence::NotRequired,
        None,
    )
    .await?
    .boxed()?;
    let scenario_export = match restored
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: restored.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected placeholder scenario export: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &scenario_export,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let expected_placeholder = assert_scenario_placeholder_selection(&inspected)?;

    let imported_directory = private_tempdir()?;
    let imported = EuthetoApp::open(dependencies(&imported_directory)?)
        .await
        .boxed()?;
    let preview_id = match imported
        .query(AppQuery::PreviewImport {
            cancellation: imported.setup_cancellation(),
            bytes: scenario_export,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: true,
                include_assets: true,
            },
        })
        .await
        .boxed()?
    {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => {
            assert!(preview.omitted_assets.contains_key("large.txt"));
            preview_id
        }
        other => return Err(format!("unexpected placeholder import preview: {other:?}").into()),
    };
    imported
        .execute(AppCommand::ApplyImport {
            cancellation: imported.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
        })
        .await
        .boxed()?;
    let reexported = match imported
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: imported.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => {
            return Err(format!("unexpected placeholder scenario re-export: {other:?}").into());
        }
    };
    let inspected = inspect_bundle(
        &reexported,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert_eq!(
        assert_scenario_placeholder_selection(&inspected)?,
        expected_placeholder
    );
    Ok(())
}
async fn create_historical_closure_backup() -> Result<(ScenarioId, Vec<u8>), Box<dyn Error>> {
    let source_directory = private_tempdir()?;
    let source = EuthetoApp::open(dependencies(&source_directory)?)
        .await
        .boxed()?;
    let scenario_id = create_project(&source, "Historical closure").await?;
    let seed_bytes = match source
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: source.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected scenario export: {other:?}").into()),
    };
    let seed = inspect_bundle(
        &seed_bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    let mut current = seed.scenarios[0].clone();
    current.revision = Revision::new(7);
    current.project = Some(PortableProjectMetadata { archived_at: None });
    let mut historical = current.clone();
    historical.revision = Revision::INITIAL;
    historical.project = None;
    historical.extensions.insert(
        "vendor.history".to_owned(),
        json!({"assetKey": "historical-only.txt"}),
    );
    let mut sections = BackupSections::default();
    sections.results.insert(
        "018f1e2d-3c4b-7a69-8def-012345678930".to_owned(),
        json!({
            "resultId": "018f1e2d-3c4b-7a69-8def-012345678930",
            "scenarioId": scenario_id,
            "scenarioRevision": current.revision,
            "result": {"score": 1}
        }),
    );
    sections.results.insert(
        "018f1e2d-3c4b-7a69-8def-012345678931".to_owned(),
        json!({
            "resultId": "018f1e2d-3c4b-7a69-8def-012345678931",
            "scenarioId": scenario_id,
            "scenarioRevision": historical.revision,
            "result": {"score": 7}
        }),
    );
    sections.assets.insert(
        "historical-only.txt".to_owned(),
        PortableAsset {
            bytes: b"historical".to_vec(),
            media_type: "text/plain".to_owned(),
            redistribution_permitted: true,
        },
    );
    let bundle = assemble_full_backup(
        &FullBackupSnapshot {
            bundle_id: seed.manifest.bundle_id,
            created_at: seed.manifest.created_at,
            application: seed.manifest.application,
            title: "Historical closure".to_owned(),
            scenarios: vec![current],
            scenario_revisions: vec![historical],
            sections,
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: complete_full_backup_extensions()?,
        },
        &portable_encode::encode_fixture_domain,
    )?;
    Ok((scenario_id, bundle))
}

#[tokio::test]
async fn scenario_export_keeps_historical_results_and_historical_only_assets()
-> Result<(), Box<dyn Error>> {
    let (scenario_id, bundle) = create_historical_closure_backup().await?;

    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let (preview_id, _) = preview_add_backup(&target, bundle).await?;
    target
        .execute(AppCommand::ApplyRestore {
            cancellation: target.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: false,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .boxed()?;
    let exported = match target
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: target.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected selected export: {other:?}").into()),
    };
    let inspected = inspect_bundle(
        &exported,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert_eq!(inspected.scenario_revisions.len(), 1);
    assert_eq!(
        inspected
            .additional_entries
            .keys()
            .filter(|path| path.starts_with("results/"))
            .count(),
        2
    );
    assert!(
        inspected
            .additional_entries
            .contains_key("assets/historical-only.txt")
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn failed_destructive_restore_rolls_back_across_restart_and_keeps_verified_backup()
-> Result<(), Box<dyn Error>> {
    let (backup, restored_id) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    std::fs::create_dir_all(&dependencies.paths.safety_backups)?;
    let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store(
        Arc::clone(&store),
        initialization,
        dependencies.clone(),
    )
    .boxed()?;
    let prior_id = create_project(&app, "Prior library").await.boxed()?;
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: add_entity_envelope(prior_id, Revision::INITIAL, "Prior person")?,
        truncate_redo: false,
    })
    .await
    .boxed()?;
    app.execute(AppCommand::SetSetting {
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .boxed()?
            .library_revision,
        request_id: request_id()?,
        key: "locale".to_owned(),
        value: json!("en-GB"),
    })
    .await
    .boxed()?;
    let prior_before = serde_json::to_value(scenario_view(&app, prior_id).await.boxed()?)?;
    let setting_before = match app
        .query(AppQuery::Setting("locale".to_owned()))
        .await
        .boxed()?
    {
        AppQueryResult::Setting(Some(setting)) => setting,
        other => return Err(format!("unexpected prior setting: {other:?}").into()),
    };
    let preview_id = preview_restore(&app, backup).await?;
    store.set_failpoint(Failpoint::AfterSupplementalWrite)?;
    assert!(
        app.execute(AppCommand::ApplyRestore {
            cancellation: app.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .is_err()
    );
    let safety_backups =
        std::fs::read_dir(&dependencies.paths.safety_backups)?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(safety_backups.len(), 1);
    let safety_backup_path = safety_backups[0].path();
    inspect_bundle(
        &std::fs::read(&safety_backup_path)?,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    drop(app);
    drop(store);

    let reopened = EuthetoApp::open(dependencies).await.boxed()?;
    let prior = scenario_view(&reopened, prior_id).await.boxed()?;
    assert_eq!(serde_json::to_value(&prior)?, prior_before);
    assert!(matches!(
        reopened
            .query(AppQuery::Setting("locale".to_owned()))
            .await
            .boxed()?,
        AppQueryResult::Setting(Some(setting)) if setting == setting_before
    ));
    assert!(matches!(
        reopened.query(AppQuery::ProjectMetadata(restored_id)).await,
        Err(AppError::NotFound(_))
    ));
    assert!(safety_backup_path.exists());
    assert_eq!(
        std::fs::read_dir(directory.path().join("backups"))?
            .collect::<Result<Vec<_>, _>>()?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn backup_restore_replace_applies_atomically_through_the_store() -> Result<(), Box<dyn Error>>
{
    let (backup, restored_id) = create_inspected_backup().await?;
    restore_backup_and_assert(backup, restored_id).await
}

async fn apply_restore_with_authorization(
    app: &EuthetoApp,
    preview_id: RequestId,
    collision_plan: CollisionPlan,
    safety_backup: SafetyBackupEvidence,
    prospective_failure_receipt_token: Option<String>,
) -> Result<Result<AppCommandResult, AppError>, Box<dyn Error>> {
    Ok(app
        .execute(AppCommand::ApplyRestore {
            cancellation: app.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan,
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup,
                prospective_failure_receipt_token,
                collision_plan_sha256: None,
            },
        })
        .await)
}

async fn apply_restore_with_safety_evidence(
    app: &EuthetoApp,
    preview_id: RequestId,
    safety_backup: SafetyBackupEvidence,
) -> Result<Result<AppCommandResult, AppError>, Box<dyn Error>> {
    apply_restore_with_authorization(
        app,
        preview_id,
        CollisionPlan::default(),
        safety_backup,
        None,
    )
    .await
}

async fn assert_preview_consumed(
    app: &EuthetoApp,
    preview_id: RequestId,
) -> Result<(), Box<dyn Error>> {
    assert!(matches!(
        apply_restore_with_safety_evidence(
            app,
            preview_id,
            SafetyBackupEvidence::NotRequired
        )
        .await?,
        Err(AppError::Protocol(failure)) if failure.code == "portable.preview_not_found"
    ));
    Ok(())
}

fn disable_safety_backups(directory: &TempDir) -> Result<(), Box<dyn Error>> {
    let path = directory.path().join("backups");
    if path.is_dir() {
        std::fs::remove_dir_all(&path)?;
    }
    std::fs::write(path, b"not a directory")?;
    Ok(())
}

fn failure_proof(proof: impl Into<String>) -> SafetyBackupEvidence {
    SafetyBackupEvidence::FailedWithStrongConfirmation {
        proof: proof.into(),
    }
}

fn assert_receipt_rejected(result: Result<AppCommandResult, AppError>) {
    assert!(matches!(
        result,
        Err(AppError::Validation(report))
            if report
                .issues
                .iter()
                .any(|issue| issue.code == "restore.safety_backup_receipt_rejected")
    ));
}

async fn record_failed_backup_receipt(
    app: &EuthetoApp,
    backup: Vec<u8>,
    collision_plan: CollisionPlan,
    proof: String,
) -> Result<(), Box<dyn Error>> {
    let preview_id = preview_restore(app, backup).await?;
    assert!(matches!(
        apply_restore_with_authorization(
            app,
            preview_id,
            collision_plan,
            SafetyBackupEvidence::NotRequired,
            Some(proof),
        )
        .await?,
        Err(AppError::Protocol(failure)) if failure.code == "restore.safety_backup_failed"
    ));
    Ok(())
}

#[tokio::test]
async fn add_restore_rejects_replace_receipt_fields() -> Result<(), Box<dyn Error>> {
    let (backup, _) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let (preview_id, _) = preview_add_backup(&app, backup).await?;
    let result = app
        .execute(AppCommand::ApplyRestore {
            cancellation: app.setup_cancellation(),
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: false,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: Some(request_id()?.to_string()),
                collision_plan_sha256: Some("a".repeat(64)),
            },
        })
        .await;
    assert!(matches!(
        result,
        Err(AppError::Validation(report))
            if report
                .issues
                .iter()
                .any(|issue| issue.code == "restore.safety_backup_evidence_invalid")
    ));
    Ok(())
}

#[tokio::test]
async fn first_replace_cannot_bypass_backup_and_same_session_phrase_uses_recorded_failure()
-> Result<(), Box<dyn Error>> {
    let (backup, restored_id) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    disable_safety_backups(&directory)?;

    let bypass_preview = preview_restore(&app, backup.clone()).await?;
    assert!(matches!(
        apply_restore_with_safety_evidence(
            &app,
            bypass_preview,
            failure_proof("REPLACE WITHOUT BACKUP"),
        )
        .await?,
        Err(AppError::Validation(report))
            if report.issues.iter().any(|issue| issue.code == "restore.safety_backup_override_not_available")
    ));
    assert_preview_consumed(&app, bypass_preview).await?;

    let retained_preview = preview_restore(&app, backup).await?;
    let reviewed_revision = app
        .application_settings_snapshot()
        .await
        .boxed()?
        .library_revision;
    assert!(
        !app.portable_restore_retry_is_retained(retained_preview, reviewed_revision)
            .await
    );
    assert!(matches!(
        apply_restore_with_safety_evidence(
            &app,
            retained_preview,
            SafetyBackupEvidence::NotRequired,
        )
        .await?,
        Err(AppError::Protocol(failure)) if failure.code == "restore.safety_backup_failed"
    ));
    assert!(
        app.portable_restore_retry_is_retained(retained_preview, reviewed_revision)
            .await
    );
    let applied = apply_restore_with_safety_evidence(
        &app,
        retained_preview,
        failure_proof("REPLACE WITHOUT BACKUP"),
    )
    .await?
    .boxed()?;
    assert!(matches!(
        applied,
        AppCommandResult::PortableApplied {
            scenarios, safety_backup: SafetyBackupOutcome::ConfirmedBypass, ..
        }
            if scenarios.len() == 1
                && scenarios[0].source_scenario_id == restored_id
                && scenarios[0].scenario_id == restored_id
    ));
    assert!(
        !app.portable_restore_retry_is_retained(retained_preview, reviewed_revision)
            .await
    );
    Ok(())
}

#[tokio::test]
async fn actual_failure_receipt_survives_restart_and_is_consumed_once() -> Result<(), Box<dyn Error>>
{
    let (backup, restored_id) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    let app = EuthetoApp::open(dependencies.clone()).await.boxed()?;
    disable_safety_backups(&directory)?;
    let proof = request_id()?.to_string();
    record_failed_backup_receipt(
        &app,
        backup.clone(),
        CollisionPlan::default(),
        proof.clone(),
    )
    .await?;
    drop(app);

    let reopened = EuthetoApp::open(dependencies).await.boxed()?;
    let preview_id = preview_restore(&reopened, backup.clone()).await?;
    let applied =
        apply_restore_with_safety_evidence(&reopened, preview_id, failure_proof(proof.clone()))
            .await?
            .boxed()?;
    assert!(matches!(
        applied,
        AppCommandResult::PortableApplied { scenarios, .. }
            if scenarios.len() == 1
                && scenarios[0].source_scenario_id == restored_id
                && scenarios[0].scenario_id == restored_id
    ));
    let replay_preview = preview_restore(&reopened, backup).await?;
    assert_receipt_rejected(
        apply_restore_with_safety_evidence(&reopened, replay_preview, failure_proof(proof)).await?,
    );
    Ok(())
}

#[tokio::test]
async fn failure_receipt_rejects_a_stale_preview_binding() -> Result<(), Box<dyn Error>> {
    let (backup, _) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    disable_safety_backups(&directory)?;
    let proof = request_id()?.to_string();
    record_failed_backup_receipt(
        &app,
        backup.clone(),
        CollisionPlan::default(),
        proof.clone(),
    )
    .await?;
    app.execute(AppCommand::SetSetting {
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .boxed()?
            .library_revision,
        request_id: request_id()?,
        key: "appearance".to_owned(),
        value: json!({"theme": "dark"}),
    })
    .await
    .boxed()?;
    let stale_preview = preview_restore(&app, backup).await?;
    assert_receipt_rejected(
        apply_restore_with_safety_evidence(&app, stale_preview, failure_proof(proof)).await?,
    );
    Ok(())
}

#[tokio::test]
async fn failure_receipt_rejects_a_changed_collision_plan() -> Result<(), Box<dyn Error>> {
    let (backup, scenario_id) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let (add_preview, _) = preview_add_backup(&app, backup.clone()).await?;
    apply_restore_with_authorization(
        &app,
        add_preview,
        CollisionPlan::default(),
        SafetyBackupEvidence::NotRequired,
        None,
    )
    .await?
    .boxed()?;
    disable_safety_backups(&directory)?;
    let proof = request_id()?.to_string();
    let recorded_plan = CollisionPlan::default();
    record_failed_backup_receipt(&app, backup.clone(), recorded_plan, proof.clone()).await?;
    let changed_plan = CollisionPlan {
        scenarios: BTreeMap::from([(scenario_id, CollisionAction::Skip)]),
        supplemental: BTreeMap::new(),
    };
    let stale_preview = preview_restore(&app, backup.clone()).await?;
    let invalid = apply_restore_with_authorization(
        &app,
        stale_preview,
        changed_plan,
        failure_proof(proof.clone()),
        None,
    )
    .await?;
    assert!(matches!(
        invalid,
        Err(AppError::Validation(report))
            if report
                .issues
                .iter()
                .any(|issue| issue.code == "portable.restore_invalid")
    ));
    let valid_preview = preview_restore(&app, backup).await?;
    apply_restore_with_safety_evidence(&app, valid_preview, failure_proof(proof))
        .await?
        .boxed()?;
    Ok(())
}

#[tokio::test]
async fn verified_safety_backup_does_not_create_a_failure_receipt() -> Result<(), Box<dyn Error>> {
    let (backup, _) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let prospective_proof = request_id()?.to_string();
    let preview_id = preview_restore(&app, backup.clone()).await?;
    let applied = apply_restore_with_authorization(
        &app,
        preview_id,
        CollisionPlan::default(),
        SafetyBackupEvidence::NotRequired,
        Some(prospective_proof.clone()),
    )
    .await?
    .boxed()?;
    let AppCommandResult::PortableApplied {
        library_revision,
        safety_backup: SafetyBackupOutcome::CreatedAndVerified { artifact_name },
        ..
    } = applied
    else {
        return Err("a committed replacement must report its verified safety artifact".into());
    };
    assert_eq!(
        library_revision,
        app.application_settings_snapshot()
            .await
            .boxed()?
            .library_revision
    );
    assert_eq!(
        std::path::Path::new(&artifact_name)
            .file_name()
            .and_then(std::ffi::OsStr::to_str),
        Some(artifact_name.as_str())
    );
    let safety_backup = inspect_bundle(
        &std::fs::read(directory.path().join("backups").join(artifact_name))?,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &portable_decode::decode_fixture_domain,
    )?;
    assert!(safety_backup.scenarios.is_empty());
    disable_safety_backups(&directory)?;
    let fresh_preview = preview_restore(&app, backup).await?;
    assert_receipt_rejected(
        apply_restore_with_safety_evidence(&app, fresh_preview, failure_proof(prospective_proof))
            .await?,
    );
    Ok(())
}
#[tokio::test]
async fn timestamp_only_setting_restore_refreshes_once_then_identical_restore_is_noop()
-> Result<(), Box<dyn Error>> {
    let (backup, scenario_id) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let first = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let (initial_preview, _) = preview_add_backup(&first, backup.clone()).await?;
    first
        .execute(AppCommand::ApplyRestore {
            cancellation: first.setup_cancellation(),
            request_id: request_id()?,
            preview_id: initial_preview,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: false,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .boxed()?;
    drop(first);

    let app = EuthetoApp::open(dependencies_at(&directory, "2026-01-11T12:00:00Z")?)
        .await
        .boxed()?;
    let setting_value = json!({"theme": "dark", "reducedMotion": true});
    app.execute(AppCommand::SetSetting {
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .boxed()?
            .library_revision,
        request_id: request_id()?,
        key: "appearance".to_owned(),
        value: setting_value,
    })
    .await
    .boxed()?;

    let plan = CollisionPlan {
        scenarios: BTreeMap::from([(scenario_id, CollisionAction::Skip)]),
        supplemental: BTreeMap::new(),
    };
    let (changed_preview_id, changed_preview) = preview_add_backup(&app, backup.clone()).await?;
    assert_eq!(
        changed_preview.settings_changed,
        vec!["appearance".to_owned()]
    );
    let mut changed_notifications = app.subscribe(EventTopic::AppNotification).await.boxed()?;
    let changed_request_id = request_id()?;
    let changed = app
        .execute(AppCommand::ApplyRestore {
            cancellation: app.setup_cancellation(),
            request_id: changed_request_id,
            preview_id: changed_preview_id,
            collision_plan: plan.clone(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: false,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
        })
        .await
        .boxed()?;
    assert!(matches!(
        changed,
        AppCommandResult::PortableApplied { scenarios, .. } if scenarios.is_empty()
    ));
    assert!(matches!(
        changed_notifications.recv().await.boxed()?.payload,
        EventPayload::AppNotification { context, code, .. }
            if context.request_id == Some(changed_request_id) && code == "library.refreshed"
    ));

    let (noop_preview_id, noop_preview) = preview_add_backup(&app, backup.clone()).await?;
    assert!(noop_preview.settings_changed.is_empty());
    let revision_before = noop_preview.binding.local_library_revision;
    let mut noop_notifications = app.subscribe(EventTopic::AppNotification).await.boxed()?;
    app.execute(AppCommand::ApplyRestore {
        cancellation: app.setup_cancellation(),
        request_id: request_id()?,
        preview_id: noop_preview_id,
        collision_plan: plan,
        authorization: RestoreAuthorization {
            destructive_action_confirmed: false,
            safety_backup: SafetyBackupEvidence::NotRequired,
            prospective_failure_receipt_token: None,
            collision_plan_sha256: None,
        },
    })
    .await
    .boxed()?;
    assert!(noop_notifications.try_recv().boxed()?.is_none());
    let (_, after_noop) = preview_add_backup(&app, backup).await?;
    assert_eq!(after_noop.binding.local_library_revision, revision_before);
    Ok(())
}

#[tokio::test]
async fn deferred_solve_and_ai_calls_are_typed_unsupported() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    for capability in [
        DeferredCapability::Solve,
        DeferredCapability::ArtificialIntelligence,
    ] {
        assert!(matches!(
            app.query(AppQuery::Deferred(capability)).await,
            Err(AppError::Unsupported(_))
        ));
    }
    for capability in [
        DeferredCapability::Solve,
        DeferredCapability::ArtificialIntelligence,
    ] {
        assert!(matches!(
            app.execute(AppCommand::Deferred(capability)).await,
            Err(AppError::Unsupported(_))
        ));
    }
    Ok(())
}

#[tokio::test]
async fn registries_expose_only_validated_static_metadata_in_stable_order()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;

    let packs = match app.query(AppQuery::ListDomainPacks).await.boxed()? {
        AppQueryResult::DomainPacks(packs) => packs,
        other => return Err(format!("unexpected pack-list result: {other:?}").into()),
    };
    assert_eq!(
        packs
            .iter()
            .map(|pack| pack.id.as_str())
            .collect::<Vec<_>>(),
        vec!["official.test", "official.workforce"]
    );
    assert!(packs.windows(2).all(|pair| pair[0].id < pair[1].id));
    for pack in &packs {
        let pack_id = pack.id.clone();
        assert!(matches!(
            app.query(AppQuery::DescribeDomainPack(pack_id.clone()))
                .await
                .boxed()?,
            AppQueryResult::DomainPack(metadata)
                if metadata.as_ref().descriptor.id == pack_id
                    && metadata.as_ref().catalog.pack_id == pack_id
        ));
    }
    let missing_pack: eutheto_types::PackId = "vendor.future".parse()?;
    assert!(matches!(
        app.query(AppQuery::DescribeDomainPack(missing_pack.clone()))
            .await,
        Err(AppError::NotFound(eutheto_types::ResourceRef::Pack(id))) if id == missing_pack
    ));

    assert!(matches!(
        app.query(AppQuery::ListSolvers).await.boxed()?,
        AppQueryResult::Solvers(solvers) if solvers.is_empty()
    ));
    let fake_production: BackendId = "solver.fake-production".parse()?;
    assert!(matches!(
        app.query(AppQuery::DescribeSolver(fake_production.clone()))
            .await,
        Err(AppError::NotFound(eutheto_types::ResourceRef::Backend(id)))
            if id == fake_production
    ));
    let matrix = match app.query(AppQuery::SolverSupportMatrix).await.boxed()? {
        AppQueryResult::SolverSupportMatrix(matrix) => matrix,
        other => return Err(format!("unexpected matrix result: {other:?}").into()),
    };
    assert_eq!(
        matrix
            .production_backend_ids
            .iter()
            .map(BackendId::as_str)
            .collect::<Vec<_>>(),
        vec!["solver.ortools-cp-sat"]
    );
    assert_eq!(matrix.backend_columns.len(), 1);
    assert_eq!(
        matrix.backend_columns[0].backend_id.as_str(),
        "solver.ortools-cp-sat"
    );
    assert_eq!(matrix.backend_columns[0].backend_version, "9.15.6755");
    assert_eq!(matrix.backend_columns[0].adapter_version, "0.1.0");
    assert_eq!(matrix.backend_columns[0].cells.len(), matrix.features.len());
    assert!(!matrix.features.is_empty());
    assert!(
        matrix
            .features
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id)
    );
    let gates = match app.query(AppQuery::DeferredSolverGates).await.boxed()? {
        AppQueryResult::DeferredSolverGates(gates) => gates,
        other => return Err(format!("unexpected deferred-gates result: {other:?}").into()),
    };
    assert_eq!(
        gates
            .iter()
            .map(|gate| (gate.backend_id.as_str(), gate.owning_phase))
            .collect::<Vec<_>>(),
        vec![("solver.pumpkin", 8)]
    );
    Ok(())
}

#[tokio::test]
async fn unknown_pack_is_rejected_before_invalid_domain_is_materialized()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let scenario_id = create_project(&app, "Preflight ordering").await?;
    let bytes = match app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected bundle result: {other:?}").into()),
    };
    let bytes = with_unknown_pack_and_invalid_domain(&bytes)?;
    let result = app
        .query(AppQuery::PreviewImport {
            cancellation: app.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await;
    assert!(matches!(
        result,
        Err(AppError::Validation(report))
            if report.issues.iter().any(|issue| issue.code == "domain_pack.unsupported")
    ));
    match app
        .query(AppQuery::ListProjects(ProjectScope::All))
        .await
        .boxed()?
    {
        AppQueryResult::Projects(projects) => {
            assert_eq!(projects.len(), 1);
            assert_eq!(projects[0].scenario_id, scenario_id);
        }
        other => return Err(format!("unexpected projects result: {other:?}").into()),
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn unopened_bundle_capability_preserves_exact_newer_bytes_and_is_consumed_safely()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let scenario_id = create_project(&app, "Unopened").await?;
    let current = match app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected bundle result: {other:?}").into()),
    };
    let newer = with_newer_unknown_semantics(&current)?;
    let import_options = ImportOptions {
        restore_mode: RestoreMode::ImportScenario,
        include_results: false,
        include_assets: false,
    };
    assert!(matches!(
        app.query(AppQuery::PreviewImport {
            cancellation: app.setup_cancellation(),
            bytes: newer.clone(),
            options: import_options,
        })
        .await,
        Err(AppError::Validation(report))
            if report.issues.iter().any(|issue| issue.code == "portable.version_newer")
    ));

    let preview_id = match app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes: newer.clone(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::UnopenedBundlePreview {
            preview_id,
            metadata,
        } => {
            assert_eq!(
                metadata.portable_schema_version,
                CURRENT_PORTABLE_SCHEMA_VERSION.saturating_add(1)
            );
            assert_eq!(metadata.required_capabilities.len(), 1);
            assert_eq!(metadata.required_capabilities[0].id, "future.semantic");
            preview_id
        }
        other => return Err(format!("unexpected unopened result: {other:?}").into()),
    };
    let destination = directory.path().join("preserved.eutheto");
    assert!(matches!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id,
            destination: destination.clone(),
        })
        .await
        .boxed()?,
        AppCommandResult::UnopenedBundleReexported
    ));
    assert_eq!(std::fs::read(&destination)?, newer);
    match app
        .query(AppQuery::ListProjects(ProjectScope::All))
        .await
        .boxed()?
    {
        AppQueryResult::Projects(projects) => {
            assert_eq!(projects.len(), 1);
            assert_eq!(projects[0].scenario_id, scenario_id);
        }
        other => return Err(format!("unexpected projects result: {other:?}").into()),
    }
    assert!(matches!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id,
            destination: directory.path().join("second.eutheto"),
        })
        .await,
        Err(AppError::Protocol(failure)) if failure.code == "portable.preview_not_found"
    ));

    let cancelled = match app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes: current.clone(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::UnopenedBundlePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected unopened result: {other:?}").into()),
    };
    app.execute(AppCommand::CancelPortablePreview {
        preview_id: cancelled,
    })
    .await
    .boxed()?;
    assert!(matches!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id: cancelled,
            destination: directory.path().join("cancelled.eutheto"),
        })
        .await,
        Err(AppError::Protocol(failure)) if failure.code == "portable.preview_not_found"
    ));

    let no_clobber = match app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes: current,
        })
        .await
        .boxed()?
    {
        AppQueryResult::UnopenedBundlePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected unopened result: {other:?}").into()),
    };
    let occupied = directory.path().join("occupied.eutheto");
    std::fs::write(&occupied, b"existing")?;
    assert!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id: no_clobber,
            destination: occupied.clone(),
        })
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&occupied)?, b"existing");
    assert!(matches!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id: no_clobber,
            destination: directory.path().join("retry.eutheto"),
        })
        .await,
        Err(AppError::Protocol(failure)) if failure.code == "portable.preview_not_found"
    ));
    let invalid_parent = match app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes: newer.clone(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::UnopenedBundlePreview { preview_id, .. } => preview_id,
        other => return Err(format!("unexpected unopened result: {other:?}").into()),
    };
    let missing_destination = directory.path().join("missing-parent").join("bundle");
    assert!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id: invalid_parent,
            destination: missing_destination.clone(),
        })
        .await
        .is_err()
    );
    assert!(!missing_destination.exists());
    assert!(matches!(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id: invalid_parent,
            destination: directory.path().join("invalid-parent-retry"),
        })
        .await,
        Err(AppError::Protocol(failure)) if failure.code == "portable.preview_not_found"
    ));
    Ok(())
}

#[tokio::test]
async fn initialized_app_exposes_the_exact_startup_recovery_outcome() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let dependencies = dependencies(&directory)?;
    let (store, mut initialization) =
        SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let interrupted_run_id = SolveRunId::new(&SystemIdGenerator)?;
    initialization.recovery.interrupted_solve_run_ids = vec![interrupted_run_id];
    let expected = initialization.clone();

    let app = EuthetoApp::from_initialized_store(Arc::new(store), initialization, dependencies)
        .boxed()?;

    assert_eq!(app.initialization(), &expected);
    assert_eq!(
        app.initialization()
            .recovery
            .interrupted_solve_run_ids
            .as_slice(),
        &[interrupted_run_id]
    );
    Ok(())
}

#[tokio::test]
async fn application_settings_accept_only_the_documented_nonsecret_schema()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    for (key, value) in [
        (
            "appearance",
            json!({"theme": "dark", "reducedMotion": true}),
        ),
        ("locale", json!("en-US")),
        ("units", json!("us-customary")),
    ] {
        app.execute(AppCommand::SetSetting {
            expected_library_revision: app
                .application_settings_snapshot()
                .await
                .boxed()?
                .library_revision,
            request_id: request_id()?,
            key: key.to_owned(),
            value,
        })
        .await
        .boxed()?;
    }
    for (key, value) in [
        ("appearance", json!({"theme": "dark", "token": "secret"})),
        ("locale", json!("/home/user/config")),
        ("units", json!("database")),
        ("credential", json!("secret")),
    ] {
        assert!(matches!(
            app.execute(AppCommand::SetSetting {
                request_id: request_id()?,
                expected_library_revision: app
                    .application_settings_snapshot()
                    .await
                    .boxed()?
                    .library_revision,
                key: key.to_owned(),
                value,
            })
            .await,
            Err(AppError::Validation(_))
        ));
    }
    Ok(())
}

const PATH_SENTINEL: &str = "SUPPORT_PATH_SENTINEL_DO_NOT_LEAK";
const CONTENT_SENTINEL: &str = "SUPPORT_DOCUMENT_SENTINEL_DO_NOT_LEAK";
const SECRET_SENTINEL: &str = "SUPPORT_CREDENTIAL_SENTINEL_DO_NOT_LEAK";
const ENVIRONMENT_SENTINEL: &str = "SUPPORT_ENV_SENTINEL_DO_NOT_LEAK";

fn support_dependencies(directory: &TempDir) -> Result<AppDependencies, Box<dyn Error>> {
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory
                .path()
                .join(PATH_SENTINEL)
                .join("private-library-SUPPORT_FILENAME_SENTINEL.sqlite"),
            safety_backups: directory
                .path()
                .join("private-backups-SUPPORT_BACKUP_PATH_SENTINEL"),
        },
        clock: Arc::new(FixedClock::new(timestamp("2026-01-10T12:00:00Z")?)),
        monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: eutheto_types::CancellationToken::default(),
    };
    let Some(database_parent) = dependencies.paths.database.parent() else {
        return Err(std::io::Error::other("the injected database path has no parent").into());
    };
    std::fs::create_dir_all(database_parent)?;
    std::fs::create_dir_all(&dependencies.paths.safety_backups)?;
    Ok(dependencies)
}

async fn seeded_support_app(
    directory: &TempDir,
) -> Result<(EuthetoApp, AppDependencies, u32), Box<dyn Error>> {
    const SECRET_SETTING_KEYS: &[&str] = &["credential.SUPPORT_CREDENTIAL_SENTINEL_DO_NOT_LEAK"];
    let dependencies = support_dependencies(directory)?;
    let (store, mut initialization) =
        SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let storage_schema_version = initialization.schema_version;
    initialization.recovery.interrupted_solve_run_ids = vec![SolveRunId::new(&SystemIdGenerator)?];
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store(
        Arc::clone(&store),
        initialization,
        dependencies.clone(),
    )
    .boxed()?;
    let scenario_id = create_project(&app, CONTENT_SENTINEL).await.boxed()?;
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: add_entity_envelope(scenario_id, Revision::INITIAL, CONTENT_SENTINEL)?,
        truncate_redo: false,
    })
    .await
    .boxed()?;
    store
        .replace_settings(
            SECRET_SETTING_KEYS,
            store.settings_snapshot(SECRET_SETTING_KEYS).await?,
            BTreeMap::from([(
                SECRET_SETTING_KEYS[0].to_owned(),
                eutheto_store::AppSetting {
                    value: json!({
                        "secret": SECRET_SENTINEL,
                        "environmentValue": ENVIRONMENT_SENTINEL,
                    }),
                    updated_at: timestamp("2026-01-10T12:00:00Z")?,
                },
            )]),
            eutheto_types::CancellationToken::new(),
        )
        .await?;
    Ok((app, dependencies, storage_schema_version))
}

async fn support_preview(app: &EuthetoApp) -> Result<SupportPreviewDto, Box<dyn Error>> {
    match app.query(AppQuery::SupportPreview).await.boxed()? {
        AppQueryResult::SupportPreview(preview) => Ok(preview),
        other => Err(format!("unexpected support preview result: {other:?}").into()),
    }
}

fn assert_support_preview(
    preview: &SupportPreviewDto,
    storage_schema_version: u32,
) -> Result<(), Box<dyn Error>> {
    assert_eq!(preview.schema_version, SUPPORT_PREVIEW_SCHEMA_VERSION);
    assert_eq!(preview.generated_at, timestamp("2026-01-10T12:00:00Z")?);
    assert_eq!(
        preview.schemas.scenario_format_version,
        SCENARIO_FORMAT_VERSION
    );
    assert_eq!(
        preview.schemas.storage_schema_version,
        storage_schema_version
    );
    assert_eq!(preview.library.scenario_count, 1);
    assert_eq!(preview.library.active_solve_run_count, 0);
    assert_eq!(preview.library.interrupted_recovery_count, 1);
    assert_eq!(
        preview.directories.application_data,
        DirectoryAvailabilityLabel::Available
    );
    assert_eq!(
        preview.directories.safety_backups,
        DirectoryAvailabilityLabel::Available
    );
    Ok(())
}

fn assert_support_preview_is_redacted(serialized: &str) {
    for sentinel in [
        PATH_SENTINEL,
        "SUPPORT_FILENAME_SENTINEL",
        "SUPPORT_BACKUP_PATH_SENTINEL",
        CONTENT_SENTINEL,
        SECRET_SENTINEL,
        ENVIRONMENT_SENTINEL,
    ] {
        assert!(
            !serialized.contains(sentinel),
            "redacted support preview leaked sentinel {sentinel}"
        );
    }
}

#[tokio::test]
async fn support_preview_is_deterministic_and_structurally_redacted() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let (app, dependencies, storage_schema_version) = seeded_support_app(&directory).await?;

    let first = support_preview(&app).await?;
    let second = support_preview(&app).await?;
    assert_eq!(first, second);
    assert_support_preview(&first, storage_schema_version)?;
    let serialized = serde_json::to_string(&first)?;
    let decoded: SupportPreviewDto = serde_json::from_str(&serialized)?;
    assert_eq!(decoded, first);
    assert_support_preview_is_redacted(&serialized);

    std::fs::remove_dir(&dependencies.paths.safety_backups)?;
    let unavailable = support_preview(&app).await?;
    assert_eq!(
        unavailable.directories.safety_backups,
        DirectoryAvailabilityLabel::Unavailable
    );
    assert_support_preview_is_redacted(&serde_json::to_string(&unavailable)?);
    Ok(())
}

async fn query_solution_explanation(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    source_revision: Revision,
    subject: ExplanationRequestSubjectV1,
) -> Result<SolutionExplanationDtoV2, Box<dyn Error>> {
    match app
        .query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                request: ExplanationRequestV1::new(subject)?,
            },
            source_revision,
            control: eutheto_types::OperationControl::Cancellation(app.setup_cancellation()),
        })
        .await
        .boxed()?
    {
        AppQueryResult::SolutionExplanation(result) => Ok(*result),
        other => Err(format!("unexpected solution explanation result: {other:?}").into()),
    }
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn accepted_solution_reads_bind_exact_authority_and_safe_errors() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let stale_ref = AcceptedResultRefV1::from_result(&fixture.stale.portable.accepted_result)?;
    let current_ref = AcceptedResultRefV1::from_result(&fixture.current.portable.accepted_result)?;

    let list = match fixture
        .app
        .query(AppQuery::SolutionList(SolutionListRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionList(list) => list,
        other => return Err(format!("unexpected solution list result: {other:?}").into()),
    };
    assert_eq!(list.solutions.len(), 2);
    let stale = list
        .solutions
        .iter()
        .find(|summary| summary.result == stale_ref)
        .ok_or("missing stale accepted solution")?;
    assert!(stale.stale);
    assert!(stale.scenario_revision < stale.current_revision);
    let current = list
        .solutions
        .iter()
        .find(|summary| summary.result == current_ref)
        .ok_or("missing current accepted solution")?;
    assert!(!current.stale);
    assert_eq!(current.scenario_revision, current.current_revision);

    let detail = match fixture
        .app
        .query(AppQuery::SolutionGetSummary(SolutionSummaryRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id: current_ref.solution_id,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionSummary(detail) => detail,
        other => return Err(format!("unexpected solution summary result: {other:?}").into()),
    };
    assert_eq!(detail.result, fixture.current.portable.accepted_result);
    assert_eq!(
        detail.scenario_revision.value(),
        detail.result.solution.scenario_revision
    );

    let view = match fixture
        .app
        .query(AppQuery::SolutionGetView(SolutionViewRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id: current_ref.solution_id,
            view_id: "official.test.result.summary".to_owned(),
            parameters: json!({}),
            continuation: None,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionView(view) => view,
        other => return Err(format!("unexpected solution view result: {other:?}").into()),
    };
    assert_eq!(view.result, current_ref);
    assert_eq!(view.view.view_id, "official.test.result.summary");
    assert_eq!(
        view.view.data["assignmentCount"].as_u64(),
        Some(u64::try_from(detail.result.solution.assignments.len())?)
    );

    let verified = match fixture
        .app
        .query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id: current_ref.solution_id,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionVerification(verified) => verified,
        other => return Err(format!("unexpected solution verification result: {other:?}").into()),
    };
    assert_eq!(
        verified.verification,
        fixture.current.portable.accepted_result.verification
    );

    // Historical synthetic records have no admitted source scoring projection.
    let comparison = match fixture
        .app
        .query(AppQuery::SolutionCompareV3(SolutionCompareRequestV3 {
            schema_version: SOLUTION_COMPARE_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            base_solution_id: stale_ref.solution_id,
            candidate_solution_id: current_ref.solution_id,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionComparisonV3(comparison) => comparison,
        other => return Err(format!("unexpected solution comparison result: {other:?}").into()),
    };
    assert_eq!(
        comparison.current_revision,
        Revision::try_new(fixture.current.portable.scenario_revision)?
    );
    let difference = comparison.comparison;
    assert!(matches!(
        difference.score,
        ScoreComparisonV2::Unavailable {
            reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch
        }
    ));
    difference.validate()?;
    assert_eq!(difference.base.accepted_result, stale_ref);
    assert_eq!(difference.candidate.accepted_result, current_ref);
    assert_ne!(
        difference.base.scenario_revision,
        difference.candidate.scenario_revision
    );
    let runs = difference.runs;
    assert_eq!(runs.base.run_id, fixture.stale.portable.run_input.run_id);
    assert_eq!(
        runs.candidate.run_id,
        fixture.current.portable.run_input.run_id
    );

    let assignment_id = fixture
        .current
        .portable
        .accepted_result
        .solution
        .assignments[0]
        .id
        .clone();
    let subjects = [
        (
            ExplanationRequestSubjectV1::Assignment {
                result: current_ref.clone(),
                assignment_id,
            },
            ExplanationKind::Assignment,
        ),
        (
            ExplanationRequestSubjectV1::OptimalityStatus {
                solve_run_id: fixture.current.portable.run_input.run_id,
                run_manifest_checksum: fixture.current.portable.run_manifest.checksum.clone(),
                result: Some(current_ref.clone()),
            },
            ExplanationKind::OptimalityStatus,
        ),
    ];
    for (subject, expected_kind) in subjects {
        let result = query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            Revision::try_new(fixture.current.portable.scenario_revision)?,
            subject,
        )
        .await?;
        assert_eq!(result.explanation.evidence.kind(), expected_kind);
        assert_eq!(
            result.explanation.rendered.evidence_checksum,
            result.explanation.evidence.checksum
        );
        result.explanation.validate()?;
    }
    for subject in [
        ExplanationRequestSubjectV1::SolutionDifference {
            left: stale_ref.clone(),
            right: current_ref.clone(),
        },
        ExplanationRequestSubjectV1::Repair {
            current: current_ref.clone(),
            base: stale_ref.clone(),
        },
    ] {
        let repair = matches!(&subject, ExplanationRequestSubjectV1::Repair { .. });
        let explanation = query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            Revision::try_new(fixture.current.portable.scenario_revision)?,
            subject,
        )
        .await?;
        explanation.explanation.validate()?;
        let base_revision = Revision::try_new(fixture.stale.portable.scenario_revision)?;
        let current_revision = Revision::try_new(fixture.current.portable.scenario_revision)?;
        assert_eq!(
            explanation.scenario_revisions,
            if repair {
                vec![current_revision, base_revision]
            } else {
                vec![base_revision, current_revision]
            },
        );
        let comparison = match &explanation.explanation.evidence.evidence {
            ExplanationEvidencePayloadV2::SolutionDifference { comparison } if !repair => {
                comparison.as_ref()
            }
            ExplanationEvidencePayloadV2::Repair {
                repair: repair_evidence,
            } if repair => {
                assert_eq!(
                    repair_evidence.causality,
                    eutheto_domain_ir::RepairCausalityV1::NotEstablished
                );
                &repair_evidence.comparison
            }
            _ => return Err("cross-revision explanation returned the wrong subject".into()),
        };
        assert_eq!(comparison.base.accepted_result, stale_ref);
        assert_eq!(comparison.candidate.accepted_result, current_ref);
        assert_eq!(
            comparison.runs.base.run_id,
            fixture.stale.portable.run_input.run_id
        );
        assert_eq!(
            comparison.runs.candidate.run_id,
            fixture.current.portable.run_input.run_id
        );
        assert!(matches!(
            comparison.score,
            ScoreComparisonV2::Unavailable { .. }
        ));
    }

    assert!(matches!(
        fixture
            .app
            .query(AppQuery::SolutionList(SolutionListRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION + 1,
                scenario_id: fixture.scenario_id,
            }))
            .await,
        Err(AppError::Validation(_))
    ));
    let wrong_scenario = ScenarioId::from_uuid(solution_test_id(0x8400, 1)?);
    assert!(matches!(
        fixture
            .app
            .query(AppQuery::SolutionGetSummary(SolutionSummaryRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id: wrong_scenario,
                solution_id: current_ref.solution_id,
            }))
            .await,
        Err(AppError::Validation(_))
    ));
    let mut wrong_ref = current_ref.clone();
    wrong_ref.result_checksum = blake3_hex(b"wrong-result-reference");
    assert!(
        query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            Revision::try_new(fixture.current.portable.scenario_revision)?,
            ExplanationRequestSubjectV1::Assignment {
                result: wrong_ref,
                assignment_id: fixture
                    .current
                    .portable
                    .accepted_result
                    .solution
                    .assignments[0]
                    .id
                    .clone(),
            },
        )
        .await
        .is_err()
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn validation_explanation_binds_native_issue_identity_and_rejects_stale_or_forged_requests()
-> Result<(), Box<dyn Error>> {
    use setup_control::{ControlledPack, PauseAt, PreparationGate, ReleaseOnDrop};

    let directory = private_tempdir()?;
    let app_dependencies = dependencies(&directory)?;
    let (store, initialization) =
        SqliteScenarioStore::open(&app_dependencies.paths.database).await?;
    let store = Arc::new(store);
    let app =
        EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, app_dependencies)
            .boxed()?;
    let scenario_id = create_project(&app, "Validation explanation").await?;
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: invalid_add_entity_envelope(scenario_id, Revision::INITIAL)?,
        truncate_redo: false,
    })
    .await
    .boxed()?;

    let full = app
        .full_validate_setup(
            scenario_id,
            Revision::new(1),
            OperationId::new(&SystemIdGenerator)?,
            app.setup_cancellation(),
        )
        .await
        .boxed()?;
    let native_issue = full.report.issues.first().ok_or("missing native issue")?;
    let issue_id = full
        .issue_ids
        .first()
        .cloned()
        .ok_or("missing native issue ID")?;
    assert_eq!(full.issue_ids.len(), full.report.issues.len());

    let gate = Arc::new(PreparationGate::default());
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(ControlledPack {
            pack: eutheto_command::OfficialTestPack,
            gate: Arc::clone(&gate),
        })
        .build()?;
    let controlled = EuthetoApp::from_initialized_store_with_pack_registry(
        Arc::clone(&store),
        app.initialization().clone(),
        dependencies(&directory)?,
        registry,
    )
    .boxed()?;

    let explained = query_solution_explanation(
        &controlled,
        scenario_id,
        Revision::new(1),
        ExplanationRequestSubjectV1::Validation {
            issue_id: Some(issue_id.clone()),
        },
    )
    .await?;
    let ExplanationEvidencePayloadV2::Validation { issue } =
        &explained.explanation.evidence.evidence
    else {
        return Err("validation explanation returned the wrong evidence kind".into());
    };
    assert_eq!(issue.issue_id, issue_id);
    assert_eq!(issue.message_key, native_issue.code);
    assert_eq!(explained.scenario_revisions, vec![Revision::new(1)]);
    assert_eq!(explained.current_revision, Revision::new(1));

    let forged_id = VerificationIssueId::new(format!("validation.issue.{}", "0".repeat(64)))?;
    let forged = controlled
        .query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                request: ExplanationRequestV1::new(ExplanationRequestSubjectV1::Validation {
                    issue_id: Some(forged_id),
                })?,
            },
            source_revision: Revision::new(1),
            control: eutheto_types::OperationControl::Cancellation(controlled.setup_cancellation()),
        })
        .await;
    assert!(matches!(forged, Err(AppError::Unsupported(_))));

    gate.pause_at
        .store(PauseAt::FullValidation as u8, Ordering::SeqCst);
    gate.armed.store(true, Ordering::SeqCst);
    let stale_request = ExplanationRequestV1::new(ExplanationRequestSubjectV1::Validation {
        issue_id: Some(issue_id.clone()),
    })?;
    let pending = tokio::spawn({
        let controlled = controlled.clone();
        async move {
            controlled
                .query(AppQuery::SolutionExplain {
                    request: SolutionExplainRequestV1 {
                        schema_version: SOLUTION_API_SCHEMA_VERSION,
                        scenario_id,
                        request: stale_request,
                    },
                    source_revision: Revision::new(1),
                    control: eutheto_types::OperationControl::Cancellation(
                        controlled.setup_cancellation(),
                    ),
                })
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.entered.notified()).await?;
    let committed = controlled
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: add_entity_envelope(scenario_id, Revision::new(1), "stale")?,
            truncate_redo: false,
        })
        .await
        .boxed();
    gate.release();
    committed?;
    let stale = tokio::time::timeout(std::time::Duration::from_secs(10), pending).await??;
    assert!(stale.is_err());
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn optimality_status_explanation_binds_terminal_run_and_provenance()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let current_revision = Revision::new(fixture.current.portable.scenario_revision);
    let stale_ref = AcceptedResultRefV1::from_result(&fixture.stale.portable.accepted_result)?;

    // A retained accepted result from an older source stays bound to that historical snapshot.
    let stale_status = query_solution_explanation(
        &fixture.app,
        fixture.scenario_id,
        Revision::try_new(fixture.stale.portable.scenario_revision)?,
        ExplanationRequestSubjectV1::OptimalityStatus {
            solve_run_id: fixture.stale.portable.run_input.run_id,
            run_manifest_checksum: fixture.stale.portable.run_manifest.checksum.clone(),
            result: Some(stale_ref.clone()),
        },
    )
    .await?;
    assert_eq!(
        stale_status.scenario_revisions,
        vec![Revision::new(fixture.stale.portable.scenario_revision)]
    );
    assert_ne!(
        stale_status.current_revision,
        stale_status.scenario_revisions[0]
    );
    stale_status.explanation.validate()?;

    // Feasible acceptance derives the weaker, independently-verified certainty.
    let feasible = persist_application_feasible_result(
        &fixture.store,
        fixture.scenario_id,
        current_revision,
        9,
        b"status-feasible-policy",
    )
    .await?;
    let feasible_ref = AcceptedResultRefV1::from_result(&feasible.portable.accepted_result)?;
    let feasible_status = query_solution_explanation(
        &fixture.app,
        fixture.scenario_id,
        current_revision,
        ExplanationRequestSubjectV1::OptimalityStatus {
            solve_run_id: feasible.portable.run_input.run_id,
            run_manifest_checksum: feasible.portable.run_manifest.checksum.clone(),
            result: Some(feasible_ref.clone()),
        },
    )
    .await?;
    assert_eq!(
        feasible_status.explanation.evidence.certainty,
        eutheto_domain_ir::ExplanationCertainty::IndependentlyVerified
    );
    feasible_status.explanation.validate()?;

    let no_result_manifest = persist_application_no_result_run(
        &fixture.store,
        fixture.scenario_id,
        current_revision,
        10,
        SolveStatus::NoSolutionWithinLimit,
    )
    .await?;
    let no_result_run = fixture
        .store
        .load_solve_run(fixture.scenario_id, no_result_manifest.run_id)
        .await?
        .ok_or("no-result run was not retained")?;
    let no_result = query_solution_explanation(
        &fixture.app,
        fixture.scenario_id,
        current_revision,
        ExplanationRequestSubjectV1::OptimalityStatus {
            solve_run_id: no_result_manifest.run_id,
            run_manifest_checksum: no_result_manifest.checksum.clone(),
            result: None,
        },
    )
    .await?;
    let ExplanationEvidencePayloadV2::OptimalityStatus { status } =
        &no_result.explanation.evidence.evidence
    else {
        return Err("no-result status did not return status evidence".into());
    };
    assert!(status.result.is_none());
    assert_eq!(&status.run_input, &no_result_run.input);
    assert_eq!(&status.run_manifest, &no_result_manifest);
    no_result.explanation.validate()?;

    // A backend-only infeasible status has no independent feasibility or conflict proof.
    let infeasible_manifest = persist_application_no_result_run(
        &fixture.store,
        fixture.scenario_id,
        current_revision,
        12,
        SolveStatus::Infeasible,
    )
    .await?;
    let infeasible_status = query_solution_explanation(
        &fixture.app,
        fixture.scenario_id,
        current_revision,
        ExplanationRequestSubjectV1::OptimalityStatus {
            solve_run_id: infeasible_manifest.run_id,
            run_manifest_checksum: infeasible_manifest.checksum,
            result: None,
        },
    )
    .await?;
    assert_eq!(
        infeasible_status.explanation.evidence.certainty,
        eutheto_domain_ir::ExplanationCertainty::Deterministic
    );
    infeasible_status.explanation.validate()?;

    // The run checksum is part of the exact historical authority.
    assert!(
        query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            current_revision,
            ExplanationRequestSubjectV1::OptimalityStatus {
                solve_run_id: no_result_manifest.run_id,
                run_manifest_checksum: blake3_hex(b"wrong-manifest"),
                result: None,
            },
        )
        .await
        .is_err()
    );
    // Non-accepted outcomes must not carry a result reference.
    assert!(
        query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            current_revision,
            ExplanationRequestSubjectV1::OptimalityStatus {
                solve_run_id: no_result_manifest.run_id,
                run_manifest_checksum: no_result_manifest.checksum.clone(),
                result: Some(feasible_ref.clone()),
            },
        )
        .await
        .is_err()
    );
    // An accepted run's result reference must match that run's terminal outcome exactly.
    assert!(
        query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            current_revision,
            ExplanationRequestSubjectV1::OptimalityStatus {
                solve_run_id: feasible.portable.run_input.run_id,
                run_manifest_checksum: feasible.portable.run_manifest.checksum.clone(),
                result: Some(stale_ref),
            },
        )
        .await
        .is_err()
    );

    // Scenario-scoped lookup does not disclose or reuse another scenario's retained run.
    let other_scenario = create_project(&fixture.app, "Other status source").await?;
    let other_manifest = persist_application_no_result_run(
        &fixture.store,
        other_scenario,
        Revision::INITIAL,
        11,
        SolveStatus::Cancelled,
    )
    .await?;
    assert!(
        query_solution_explanation(
            &fixture.app,
            fixture.scenario_id,
            Revision::INITIAL,
            ExplanationRequestSubjectV1::OptimalityStatus {
                solve_run_id: other_manifest.run_id,
                run_manifest_checksum: other_manifest.checksum,
                result: None,
            },
        )
        .await
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn comparison_declines_unestablished_scoring_meaning_and_legacy_versions()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let current = &fixture.current.portable;
    let same_policy = persist_application_accepted_result(
        &fixture.store,
        fixture.scenario_id,
        Revision::try_new(current.scenario_revision)?,
        4,
        b"solution-api-objective-policy",
    )
    .await?;
    let request = SolutionCompareRequestV3 {
        schema_version: SOLUTION_COMPARE_SCHEMA_VERSION,
        scenario_id: fixture.scenario_id,
        base_solution_id: current.accepted_result.solution.solution_id,
        candidate_solution_id: same_policy.portable.accepted_result.solution.solution_id,
    };
    let comparison = fixture
        .app
        .query(AppQuery::SolutionCompareV3(request.clone()))
        .await
        .boxed()?;
    let AppQueryResult::SolutionComparisonV3(comparison) = comparison else {
        return Err("missing current comparison".into());
    };
    comparison.comparison.validate()?;
    assert!(matches!(
        comparison.comparison.score,
        ScoreComparisonV2::Unavailable {
            reason: ScoreComparisonUnavailableReasonV1::PolicyOrScopeMismatch
        }
    ));
    assert_eq!(
        comparison
            .comparison
            .candidate
            .accepted_result
            .result_checksum,
        same_policy.portable.accepted_result.checksum
    );
    for schema_version in [1, 2] {
        assert!(matches!(
            fixture
                .app
                .query(AppQuery::SolutionCompareV3(SolutionCompareRequestV3 {
                    schema_version,
                    ..request.clone()
                }))
                .await,
            Err(AppError::Validation(_))
        ));
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn solution_selection_dispatches_typed_viewing_state_without_scenario_mutation()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let other_scenario = create_project(&fixture.app, "Other accepted solutions").await?;
    let other = persist_application_accepted_result(
        &fixture.store,
        other_scenario,
        Revision::INITIAL,
        3,
        b"solution-api-objective-policy",
    )
    .await?;
    let current_id = fixture
        .current
        .portable
        .accepted_result
        .solution
        .solution_id;
    let stale_id = fixture.stale.portable.accepted_result.solution.solution_id;
    let before = scenario_view(&fixture.app, fixture.scenario_id).await?;
    let library_revision = fixture.store.library_metadata_snapshot().await?.revision;

    let request = SolutionSelectRequestV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        request_id: request_id()?,
        scenario_id: fixture.scenario_id,
        expected_revision: before.revision,
        solution_id: current_id,
    };
    let encoded = serde_json::to_value(&request)?;
    assert_eq!(
        serde_json::from_value::<SolutionSelectRequestV1>(encoded.clone())?,
        request
    );
    let mut unknown = encoded.clone();
    unknown
        .as_object_mut()
        .ok_or("selection request was not an object")?
        .insert("unexpected".to_owned(), json!(true));
    assert!(serde_json::from_value::<SolutionSelectRequestV1>(unknown).is_err());
    let mut missing = encoded;
    missing
        .as_object_mut()
        .ok_or("selection request was not an object")?
        .remove("expectedRevision");
    assert!(serde_json::from_value::<SolutionSelectRequestV1>(missing).is_err());

    let selected = fixture
        .app
        .execute(AppCommand::SolutionSelect(request.clone()))
        .await
        .boxed()?;
    let AppCommandResult::SolutionSelected(selected) = selected else {
        return Err("expected selected-solution command result".into());
    };
    assert_eq!(selected.result.solution_id, current_id);
    assert!(selected.selected);
    assert!(!selected.stale);
    let retried = fixture
        .app
        .execute(AppCommand::SolutionSelect(request))
        .await
        .boxed()?;
    assert!(matches!(
        retried,
        AppCommandResult::SolutionSelected(summary) if summary == selected
    ));

    let list = match fixture
        .app
        .query(AppQuery::SolutionList(SolutionListRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionList(list) => list,
        other => return Err(format!("unexpected solution list result: {other:?}").into()),
    };
    assert_eq!(
        list.solutions
            .iter()
            .filter(|solution| solution.selected)
            .map(|solution| solution.result.solution_id)
            .collect::<Vec<_>>(),
        vec![current_id]
    );
    let detail = match fixture
        .app
        .query(AppQuery::SolutionGetSummary(SolutionSummaryRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: fixture.scenario_id,
            solution_id: current_id,
        }))
        .await
        .boxed()?
    {
        AppQueryResult::SolutionSummary(detail) => detail,
        other => return Err(format!("unexpected solution summary result: {other:?}").into()),
    };
    assert!(detail.selected);

    let selected = fixture
        .app
        .execute(AppCommand::SolutionSelect(SolutionSelectRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            request_id: request_id()?,
            scenario_id: fixture.scenario_id,
            expected_revision: before.revision,
            solution_id: stale_id,
        }))
        .await
        .boxed()?;
    assert!(matches!(
        selected,
        AppCommandResult::SolutionSelected(summary)
            if summary.result.solution_id == stale_id && summary.selected && summary.stale
    ));
    assert_eq!(
        scenario_view(&fixture.app, fixture.scenario_id).await?,
        before
    );
    assert_eq!(
        fixture.store.library_metadata_snapshot().await?.revision,
        library_revision
    );

    assert!(matches!(
        fixture
            .app
            .execute(AppCommand::SolutionSelect(SolutionSelectRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                request_id: request_id()?,
                scenario_id: fixture.scenario_id,
                expected_revision: Revision::new(before.revision.value().saturating_sub(1)),
                solution_id: current_id,
            }))
            .await,
        Err(AppError::Conflict {
            expected_revision,
            actual_revision,
        }) if expected_revision.value() + 1 == actual_revision.value()
    ));
    for unavailable in [
        other.portable.accepted_result.solution.solution_id,
        SolutionId::from_uuid(solution_test_id(0x8500, 1)?),
    ] {
        assert!(matches!(
            fixture
                .app
                .execute(AppCommand::SolutionSelect(SolutionSelectRequestV1 {
                    schema_version: SOLUTION_API_SCHEMA_VERSION,
                    request_id: request_id()?,
                    scenario_id: fixture.scenario_id,
                    expected_revision: before.revision,
                    solution_id: unavailable,
                }))
                .await,
            Err(AppError::NotFound(eutheto_types::ResourceRef::Solution(solution_id)))
                if solution_id == unavailable
        ));
    }
    assert!(matches!(
        fixture
            .app
            .execute(AppCommand::SolutionSelect(SolutionSelectRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION + 1,
                request_id: request_id()?,
                scenario_id: fixture.scenario_id,
                expected_revision: before.revision,
                solution_id: current_id,
            }))
            .await,
        Err(AppError::Validation(_))
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn counterfactual_stale_start_is_durable_idempotent_and_job_bound_cancellable()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let base = &fixture.stale.portable;
    let expected_revision = Revision::new(base.run_input.scenario_revision);
    let start_request_id = request_id()?;
    let request = SolutionStartCounterfactualRequestV1 {
        schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
        request_id: start_request_id,
        scenario_id: fixture.scenario_id,
        expected_revision,
        base_solution_id: base.accepted_result.solution.solution_id,
        condition: CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: DomainAssignmentId::new("tests.counterfactual")?,
            value: AssignmentValue::Boolean(true),
        },
        total_budget_milliseconds: DurationMillis::new(1_000)?,
    };
    let mut progress = fixture
        .app
        .subscribe(EventTopic::CounterfactualProgress)
        .await
        .boxed()?;

    let AppCommandResult::CounterfactualStarted(started) = fixture
        .app
        .execute(AppCommand::SolutionStartCounterfactual(request.clone()))
        .await
        .boxed()?
    else {
        return Err("expected counterfactual start result".into());
    };
    assert_eq!(started.schema_version, COUNTERFACTUAL_API_SCHEMA_VERSION);
    assert_eq!(started.request_id, start_request_id);
    assert_eq!(started.job.state, CounterfactualJobState::Failed);
    assert_eq!(
        started.job.error.as_ref().map(|error| error.kind),
        Some(CounterfactualFailureKind::StaleRevision)
    );
    assert!(started.job.result.is_none());
    assert!(started.job.started_at.is_none());
    let encoded = serde_json::to_value(&started)?;
    assert_eq!(
        serde_json::from_value::<SolutionStartCounterfactualDtoV1>(encoded.clone())?,
        started
    );
    let mut unknown = encoded;
    unknown
        .as_object_mut()
        .ok_or("start response was not an object")?
        .insert("unknown".to_owned(), json!(true));
    assert!(serde_json::from_value::<SolutionStartCounterfactualDtoV1>(unknown).is_err());

    let queued = progress.recv().await.boxed()?;
    let failed = progress.recv().await.boxed()?;
    for event in [&queued, &failed] {
        let EventPayload::CounterfactualProgress {
            context, job_id, ..
        } = &event.payload
        else {
            return Err("expected counterfactual progress event".into());
        };
        assert_eq!(*job_id, started.job.request.job_id);
        assert_eq!(context.request_id, Some(start_request_id));
        assert_eq!(context.scenario_id, Some(fixture.scenario_id));
        assert_eq!(context.revision, Some(expected_revision));
        assert_eq!(context.solve_run_id, None);
    }
    assert!(matches!(
        queued.payload,
        EventPayload::CounterfactualProgress {
            phase: eutheto_types::CounterfactualProgressPhase::Queued,
            ..
        }
    ));
    assert!(matches!(
        failed.payload,
        EventPayload::CounterfactualProgress {
            phase: eutheto_types::CounterfactualProgressPhase::Failed,
            ..
        }
    ));

    let AppCommandResult::CounterfactualStarted(replayed) = fixture
        .app
        .execute(AppCommand::SolutionStartCounterfactual(request))
        .await
        .boxed()?
    else {
        return Err("expected replayed counterfactual start result".into());
    };
    assert_eq!(replayed.job, started.job);
    assert!(progress.try_recv().boxed()?.is_none());

    let cancel_request_id = request_id()?;
    let AppCommandResult::CounterfactualCancelled(cancelled) = fixture
        .app
        .execute(AppCommand::SolutionCancelCounterfactual(
            SolutionCancelCounterfactualRequestV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                cancel_request_id,
                scenario_id: fixture.scenario_id,
                expected_revision,
                job_id: started.job.request.job_id,
            },
        ))
        .await
        .boxed()?
    else {
        return Err("expected counterfactual cancellation result".into());
    };
    assert_eq!(cancelled.cancel_request_id, cancel_request_id);
    assert_eq!(cancelled.job, started.job);
    let encoded = serde_json::to_value(&cancelled)?;
    assert_eq!(
        serde_json::from_value::<SolutionCancelCounterfactualDtoV1>(encoded.clone())?,
        cancelled
    );
    let mut unknown = encoded;
    unknown
        .as_object_mut()
        .ok_or("cancel response was not an object")?
        .insert("unknown".to_owned(), json!(true));
    assert!(serde_json::from_value::<SolutionCancelCounterfactualDtoV1>(unknown).is_err());
    Ok(())
}

fn counterfactual_runtime_app(
    directory: &TempDir,
    fixture: &SolutionApplicationFixture,
    registry: SolverRegistry,
) -> Result<EuthetoApp, Box<dyn Error>> {
    let mut dependencies = dependencies(directory)?;
    dependencies.clock = Arc::new(SystemClock);
    Ok(EuthetoApp::from_initialized_store_with_registries(
        Arc::clone(&fixture.store),
        fixture.app.initialization().clone(),
        dependencies,
        Arc::new(official_registry()?),
        Arc::new(registry),
    ))
}

fn counterfactual_runtime_request(
    base: &StoredAcceptedResultV2,
) -> Result<SolutionStartCounterfactualRequestV1, Box<dyn Error>> {
    let registry = official_registry()?;
    let pack = registry.require(&base.portable.run_input.pack_id)?;
    let problem = pack.compile(
        &base.document,
        &CompileContext {
            objective_context: None,
            scenario_revision: base.portable.run_input.scenario_revision,
            semantic_metadata: BTreeMap::new(),
            control: eutheto_types::OperationControl::Cancellation(
                eutheto_types::CancellationToken::new(),
            ),
            planning_limits: PlanningIrLimitsV1::DEFAULT,
        },
    )?;
    let assignment_id = problem
        .projections
        .iter()
        .find_map(|projection| {
            matches!(&projection.expression, ProjectionExpression::Boolean(_))
                .then(|| projection.assignment_id.clone())
        })
        .ok_or("official fixture had no Boolean assignment projection")?;
    Ok(SolutionStartCounterfactualRequestV1 {
        schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
        request_id: request_id()?,
        scenario_id: base.portable.run_input.scenario_id,
        expected_revision: Revision::new(base.portable.run_input.scenario_revision),
        base_solution_id: base.portable.accepted_result.solution.solution_id,
        condition: CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(false),
        },
        total_budget_milliseconds: DurationMillis::new(30_000)?,
    })
}

async fn wait_for_counterfactual_terminal(
    store: &SqliteScenarioStore,
    job_id: CounterfactualJobId,
) -> Result<CounterfactualJobRecordV1, Box<dyn Error>> {
    for _ in 0..10_000 {
        let job = store.load_counterfactual_job(job_id).await?;
        if matches!(
            job.state,
            CounterfactualJobState::Completed
                | CounterfactualJobState::Failed
                | CounterfactualJobState::Cancelled
                | CounterfactualJobState::Interrupted
        ) {
            return Ok(job);
        }
        tokio::task::yield_now().await;
    }
    Err("counterfactual job did not become terminal".into())
}

async fn collect_counterfactual_progress(
    progress: &mut EventSubscription,
) -> Result<
    Vec<(
        eutheto_types::CounterfactualProgressPhase,
        eutheto_types::EventContext,
        CounterfactualJobId,
    )>,
    Box<dyn Error>,
> {
    let mut observed = Vec::new();
    loop {
        let event = progress.recv().await.boxed()?;
        let EventPayload::CounterfactualProgress {
            context,
            job_id,
            phase,
        } = event.payload
        else {
            return Err("counterfactual subscription produced another event type".into());
        };
        let terminal = matches!(
            phase,
            eutheto_types::CounterfactualProgressPhase::Completed
                | eutheto_types::CounterfactualProgressPhase::Failed
                | eutheto_types::CounterfactualProgressPhase::Cancelled
                | eutheto_types::CounterfactualProgressPhase::Interrupted
        );
        observed.push((phase, context, job_id));
        if terminal {
            return Ok(observed);
        }
    }
}
#[cfg(debug_assertions)]
fn reseal_native_solve_fingerprint(
    manifest: &RunManifest,
) -> Result<(RunManifest, String), Box<dyn Error>> {
    let mut observations = manifest
        .native_observations
        .clone()
        .ok_or("run manifest omitted native observations")?;
    let attempt = observations
        .attempt
        .as_mut()
        .ok_or("native observations omitted the actual backend attempt")?;
    let replacement = if attempt.solve_fingerprint.starts_with('0') {
        "1"
    } else {
        "0"
    };
    attempt.solve_fingerprint.replace_range(..1, replacement);
    let changed_fingerprint = attempt.solve_fingerprint.clone();
    let manifest = manifest
        .clone()
        .with_native_observations(Some(observations))?;
    Ok((manifest, changed_fingerprint))
}

#[tokio::test]
async fn counterfactual_runtime_completes_once_through_compile_review_and_atomic_finalize()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let (registry, backend) = counterfactual_solver_registry(false)?;
    let app = counterfactual_runtime_app(&directory, &fixture, registry)?;
    let request = counterfactual_runtime_request(&fixture.current)?;
    let mut progress = app
        .subscribe(EventTopic::CounterfactualProgress)
        .await
        .boxed()?;

    let AppCommandResult::CounterfactualStarted(started) = app
        .execute(AppCommand::SolutionStartCounterfactual(request.clone()))
        .await
        .boxed()?
    else {
        return Err("expected counterfactual start response".into());
    };
    let events = collect_counterfactual_progress(&mut progress).await?;
    let completed =
        wait_for_counterfactual_terminal(&fixture.store, started.job.request.job_id).await?;
    assert_eq!(completed.state, CounterfactualJobState::Completed);
    let result = completed
        .result
        .as_ref()
        .ok_or("completed counterfactual omitted its result")?;
    let RunTerminalOutcomeV1::Accepted { solution_id, .. } = &result.run_manifest.outcome else {
        return Err("completed counterfactual did not persist an accepted run".into());
    };
    let evidence_preparation = result
        .run_manifest
        .phase_timings
        .evidence_persistence_milliseconds
        .ok_or("normal counterfactual terminal omitted evidence preparation timing")?;
    assert!(
        evidence_preparation
            <= result
                .run_manifest
                .elapsed_milliseconds
                .ok_or("normal counterfactual terminal omitted elapsed timing")?
    );
    let solution_id = *solution_id;
    let accepted = fixture.store.load_accepted_result(solution_id).await?;
    assert_eq!(
        accepted.portable.accepted_result.solution.solution_id,
        solution_id
    );

    let phases: Vec<_> = events.iter().map(|(phase, _, _)| *phase).collect();
    assert_eq!(
        phases,
        [
            eutheto_types::CounterfactualProgressPhase::Queued,
            eutheto_types::CounterfactualProgressPhase::Compiling,
            eutheto_types::CounterfactualProgressPhase::Solving,
            eutheto_types::CounterfactualProgressPhase::Verifying,
            eutheto_types::CounterfactualProgressPhase::Finalizing,
            eutheto_types::CounterfactualProgressPhase::Completed,
        ]
    );
    for (index, (_, context, job_id)) in events.iter().enumerate() {
        assert_eq!(*job_id, completed.request.job_id);
        assert_eq!(context.request_id, Some(request.request_id));
        assert_eq!(context.scenario_id, Some(request.scenario_id));
        assert_eq!(context.revision, Some(request.expected_revision));
        if index < 2 {
            assert_eq!(context.solve_run_id, None);
        } else {
            assert_eq!(context.solve_run_id, Some(result.run_input.run_id));
        }
    }

    let AppCommandResult::CounterfactualStarted(replayed) = app
        .execute(AppCommand::SolutionStartCounterfactual(request))
        .await
        .boxed()?
    else {
        return Err("expected replayed counterfactual start response".into());
    };
    assert_eq!(replayed.job, completed);
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    assert!(progress.try_recv().boxed()?.is_none());
    Ok(())
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn resealed_counterfactual_native_fingerprint_is_rejected_by_completed_replays()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let (registry, backend) = counterfactual_solver_registry(false)?;
    let app = counterfactual_runtime_app(&directory, &fixture, registry)?;
    let mut progress = app
        .subscribe(EventTopic::CounterfactualProgress)
        .await
        .boxed()?;
    let request = counterfactual_runtime_request(&fixture.current)?;
    let AppCommandResult::CounterfactualStarted(started) = app
        .execute(AppCommand::SolutionStartCounterfactual(request.clone()))
        .await
        .boxed()?
    else {
        return Err("expected counterfactual start response".into());
    };
    let _events = collect_counterfactual_progress(&mut progress).await?;
    let completed =
        wait_for_counterfactual_terminal(&fixture.store, started.job.request.job_id).await?;
    assert_eq!(completed.state, CounterfactualJobState::Completed);
    let original_result = completed
        .result
        .as_ref()
        .ok_or("completed counterfactual omitted its result")?;
    let (forged_manifest, forged_fingerprint) =
        reseal_native_solve_fingerprint(&original_result.run_manifest)?;
    assert_ne!(
        forged_fingerprint,
        original_result
            .run_manifest
            .native_observations
            .as_ref()
            .and_then(|observations| observations.attempt.as_ref())
            .ok_or("completed run omitted its actual native attempt")?
            .solve_fingerprint
    );
    let mut conclusion = original_result.conclusion.clone();
    let eutheto_domain_ir::CounterfactualConclusionV1::VerifiedAlternative {
        alternative,
        comparison,
        ..
    } = &mut conclusion
    else {
        return Err("expected a verified alternative comparison".into());
    };
    let alternative = fixture
        .store
        .load_accepted_result(alternative.solution_id)
        .await?;
    **comparison = eutheto_domain_ir::compare_accepted_results(
        &fixture.current.portable.accepted_result,
        &alternative.portable.accepted_result,
        Some(&eutheto_domain_ir::ComparisonContext {
            locks: &[],
            manifests: Some(eutheto_domain_ir::ComparisonRunManifests {
                base: &original_result.base_run_manifest,
                candidate: &forged_manifest,
            }),
        }),
    )?;
    let resealed_result = CounterfactualResult::new(
        original_result.request.clone(),
        original_result.base_run_input.clone(),
        original_result.base_run_manifest.clone(),
        original_result.compilation.clone(),
        original_result.run_input.clone(),
        forged_manifest.clone(),
        conclusion,
    )?;
    assert_ne!(resealed_result.checksum, original_result.checksum);

    assert_ne!(
        forged_manifest.checksum,
        original_result.run_manifest.checksum
    );
    let resealed_job = CounterfactualJobRecordV1::new(
        completed.request.clone(),
        completed.state,
        completed.started_at,
        completed.finished_at,
        completed.cancel_request_id,
        completed.cancel_requested_at,
        Some(resealed_result),
        completed.error,
        completed.terminal_authority.clone(),
    )?;
    let connection = Connection::open(directory.path().join("eutheto.sqlite"))?;
    assert_eq!(
        connection.execute(
            "UPDATE solve_runs SET run_manifest_json = ?1 WHERE id = ?2",
            rusqlite::params![
                serde_json::to_string(&forged_manifest)?,
                original_result.run_input.run_id.to_string(),
            ],
        )?,
        1
    );
    assert_eq!(
        connection.execute(
            "UPDATE counterfactual_jobs SET result_json = ?1 WHERE id = ?2",
            rusqlite::params![
                serde_json::to_string(
                    resealed_job
                        .result
                        .as_ref()
                        .ok_or("missing resealed result")?,
                )?,
                started.job.request.job_id.to_string(),
            ],
        )?,
        1
    );
    drop(connection);

    // The Store accepts the structurally valid descriptive carrier; Core must bind its route.
    let stored_job = fixture
        .store
        .load_counterfactual_job(started.job.request.job_id)
        .await?;
    assert_eq!(stored_job, resealed_job);
    let stored_run = fixture
        .store
        .load_solve_run(fixture.scenario_id, original_result.run_input.run_id)
        .await?
        .ok_or("resealed derived run was not retained")?;
    assert_eq!(stored_run.manifest, Some(forged_manifest));
    assert_eq!(
        fixture
            .store
            .load_accepted_result(
                fixture
                    .current
                    .portable
                    .accepted_result
                    .solution
                    .solution_id,
            )
            .await?,
        fixture.current
    );

    let job_id = started.job.request.job_id;
    assert!(matches!(
        app.execute(AppCommand::SolutionStartCounterfactual(request.clone()))
            .await,
        Err(AppError::Protocol(error)) if error.code == "solution.authority_invalid"
    ));
    assert!(matches!(
        app.execute(AppCommand::SolutionCancelCounterfactual(
            SolutionCancelCounterfactualRequestV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                cancel_request_id: request_id()?,
                scenario_id: request.scenario_id,
                expected_revision: request.expected_revision,
                job_id,
            },
        ))
        .await,
        Err(AppError::Protocol(error)) if error.code == "solution.authority_invalid"
    ));
    assert!(matches!(
        app.query(AppQuery::SolutionGetCounterfactual(
            SolutionGetCounterfactualRequestV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                job_id,
            },
        ))
        .await,
        Err(AppError::Protocol(error)) if error.code == "solution.authority_invalid"
    ));
    let base = AcceptedResultRefV1::from_result(&fixture.current.portable.accepted_result)?;
    assert!(matches!(
        app.query(AppQuery::SolutionExplain {
            request: SolutionExplainRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                request: ExplanationRequestV1::new(
                    ExplanationRequestSubjectV1::Counterfactual { job_id, base },
                )?,
            },
            source_revision: request.expected_revision,
            control: eutheto_types::OperationControl::Cancellation(app.setup_cancellation()),
        })
        .await,
        Err(AppError::Protocol(error)) if error.code == "solution.authority_invalid"
    ));
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn counterfactual_runtime_cancels_after_derived_run_without_accepting_a_result()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let (registry, backend) = counterfactual_solver_registry(true)?;
    let app = counterfactual_runtime_app(&directory, &fixture, registry)?;
    let request = counterfactual_runtime_request(&fixture.current)?;
    let condition_checksum = CounterfactualConditionV1::new(request.condition.clone())?.checksum;
    let mut progress = app
        .subscribe(EventTopic::CounterfactualProgress)
        .await
        .boxed()?;

    let AppCommandResult::CounterfactualStarted(started) = app
        .execute(AppCommand::SolutionStartCounterfactual(request.clone()))
        .await
        .boxed()?
    else {
        return Err("expected counterfactual start response".into());
    };
    let entry_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !backend.entered.load(Ordering::SeqCst) && std::time::Instant::now() < entry_deadline {
        tokio::task::yield_now().await;
    }
    let before_cancel = fixture
        .store
        .load_counterfactual_job(started.job.request.job_id)
        .await?;
    assert!(
        backend.entered.load(Ordering::SeqCst),
        "backend did not start before cancellation; job={before_cancel:?}"
    );

    let cancel_request_id = request_id()?;
    let AppCommandResult::CounterfactualCancelled(cancelled) = app
        .execute(AppCommand::SolutionCancelCounterfactual(
            SolutionCancelCounterfactualRequestV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                cancel_request_id,
                scenario_id: request.scenario_id,
                expected_revision: request.expected_revision,
                job_id: started.job.request.job_id,
            },
        ))
        .await
        .boxed()?
    else {
        return Err("expected counterfactual cancellation response".into());
    };
    assert_eq!(cancelled.cancel_request_id, cancel_request_id);
    let events = collect_counterfactual_progress(&mut progress).await?;
    let terminal =
        wait_for_counterfactual_terminal(&fixture.store, started.job.request.job_id).await?;
    assert_eq!(terminal.state, CounterfactualJobState::Cancelled);
    assert_eq!(terminal.cancel_request_id, Some(cancel_request_id));
    assert!(terminal.result.is_none());
    assert!(terminal.error.is_none());
    assert_eq!(
        fixture
            .store
            .list_accepted_results(fixture.scenario_id)
            .await?
            .len(),
        2
    );
    assert!(backend.observed_cancel.load(Ordering::SeqCst));
    assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    let derived_run_id = events
        .iter()
        .find_map(|(phase, context, _)| {
            (*phase == eutheto_types::CounterfactualProgressPhase::Solving)
                .then_some(context.solve_run_id)
                .flatten()
        })
        .ok_or("cancellation fixture never reached its derived run")?;
    let derived = fixture.store.load_solve_input(derived_run_id).await?;
    assert_eq!(
        derived.input.temporary_condition_hash,
        Some(condition_checksum)
    );
    assert_eq!(
        events.last().map(|(phase, _, _)| *phase),
        Some(eutheto_types::CounterfactualProgressPhase::Cancelled)
    );
    #[cfg(debug_assertions)]
    {
        let retained = fixture
            .store
            .load_solve_run(fixture.scenario_id, derived_run_id)
            .await?
            .ok_or("cancelled derived run was not retained")?;
        let manifest = retained
            .manifest
            .as_ref()
            .ok_or("cancelled derived run omitted its terminal manifest")?;
        assert_eq!(
            manifest.outcome,
            RunTerminalOutcomeV1::NoResult {
                status: SolveStatus::Cancelled,
            }
        );
        let (forged_manifest, _) = reseal_native_solve_fingerprint(manifest)?;
        let connection = Connection::open(directory.path().join("eutheto.sqlite"))?;
        assert_eq!(
            connection.execute(
                "UPDATE solve_runs SET run_manifest_json = ?1 WHERE id = ?2",
                rusqlite::params![
                    serde_json::to_string(&forged_manifest)?,
                    derived_run_id.to_string(),
                ],
            )?,
            1
        );
        drop(connection);
        let stored = fixture
            .store
            .load_solve_run(fixture.scenario_id, derived_run_id)
            .await?
            .ok_or("resealed cancelled run was not retained")?;
        assert_eq!(stored.manifest, Some(forged_manifest.clone()));
        assert_eq!(
            fixture
                .store
                .load_accepted_result(
                    fixture
                        .current
                        .portable
                        .accepted_result
                        .solution
                        .solution_id,
                )
                .await?,
            fixture.current
        );
        assert!(matches!(
            app.query(AppQuery::SolutionExplain {
                request: SolutionExplainRequestV1 {
                    schema_version: SOLUTION_API_SCHEMA_VERSION,
                    scenario_id: request.scenario_id,
                    request: ExplanationRequestV1::new(
                        ExplanationRequestSubjectV1::OptimalityStatus {
                            solve_run_id: derived_run_id,
                            run_manifest_checksum: forged_manifest.checksum,
                            result: None,
                        },
                    )?,
                },
                source_revision: request.expected_revision,
                control: eutheto_types::OperationControl::Cancellation(app.setup_cancellation()),
            })
            .await,
            Err(AppError::Protocol(error)) if error.code == "solution.authority_invalid"
        ));
        assert_eq!(backend.invocations.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

#[tokio::test]
async fn queued_counterfactual_cancel_replay_does_not_republish_terminal_progress()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let fixture = Box::pin(solution_application_fixture(&directory)).await?;
    let request = counterfactual_runtime_request(&fixture.current)?;
    let condition = CounterfactualConditionV1::new(request.condition.clone())?;
    let base = &fixture.current.portable;
    let semantics = CounterfactualRequestSemanticsV1 {
        schema_version: COUNTERFACTUAL_REQUEST_SEMANTICS_SCHEMA_VERSION,
        scenario_id: request.scenario_id,
        scenario_revision: request.expected_revision.value(),
        snapshot_id: base.run_input.snapshot_id,
        snapshot_document_hash: base.run_input.snapshot_document_hash.clone(),
        base: AcceptedResultRefV1 {
            solution_id: base.accepted_result.solution.solution_id,
            result_checksum: base.accepted_result.checksum.clone(),
        },
        base_run_id: base.run_input.run_id,
        base_run_input_checksum: base.run_input.checksum.clone(),
        base_model_hash: base.run_input.model_hash.clone(),
        objective_policy_hash: base.run_input.objective_policy_hash.clone(),
        condition_checksum: condition.checksum.clone(),
        total_budget_milliseconds: request.total_budget_milliseconds,
    };
    let job_id = CounterfactualJobId::new(&SystemIdGenerator)?;
    fixture
        .store
        .start_counterfactual_job(
            CounterfactualJobRequestV1::new(
                job_id,
                request.request_id,
                semantics,
                condition,
                timestamp("2026-01-10T12:00:00Z")?,
            )?,
            fixture.current.clone(),
        )
        .await?;
    let mut progress = fixture
        .app
        .subscribe(EventTopic::CounterfactualProgress)
        .await
        .boxed()?;
    let cancel = SolutionCancelCounterfactualRequestV1 {
        schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
        cancel_request_id: request_id()?,
        scenario_id: request.scenario_id,
        expected_revision: request.expected_revision,
        job_id,
    };

    let AppCommandResult::CounterfactualCancelled(first) = fixture
        .app
        .execute(AppCommand::SolutionCancelCounterfactual(cancel.clone()))
        .await
        .boxed()?
    else {
        return Err("expected first queued cancellation response".into());
    };
    assert_eq!(first.job.state, CounterfactualJobState::Cancelled);
    let event = progress
        .try_recv()
        .boxed()?
        .ok_or("first queued cancellation did not publish its terminal progress")?;
    assert!(matches!(
        event.payload,
        EventPayload::CounterfactualProgress {
            phase: eutheto_types::CounterfactualProgressPhase::Cancelled,
            ..
        }
    ));

    let AppCommandResult::CounterfactualCancelled(replayed) = fixture
        .app
        .execute(AppCommand::SolutionCancelCounterfactual(cancel))
        .await
        .boxed()?
    else {
        return Err("expected replayed queued cancellation response".into());
    };
    assert_eq!(replayed.job, first.job);
    assert!(progress.try_recv().boxed()?.is_none());
    Ok(())
}

#[cfg(debug_assertions)]
async fn portable_commit_boundary(
    phase: CommandCommitTestPhase,
    abandon: bool,
) -> Result<(), Box<dyn Error>> {
    let (backup, restored_id) = create_inspected_backup().await?;
    let directory = private_tempdir()?;
    let deps = dependencies(&directory)?;
    let hook = CommandCommitTestHook::new(phase);
    let (store, initialized) = SqliteScenarioStore::open_with_options(
        &deps.paths.database,
        OpenOptions::new(SnapshotPolicy::default()).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store(Arc::clone(&store), initialized, deps.clone())
        .boxed()?;
    let (preview_id, _) = preview_add_backup(&app, backup).await?;
    let mut events = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let request_id = request_id()?;
    let cancellation = app.setup_cancellation();
    let command = AppCommand::ApplyRestore {
        cancellation: cancellation.clone(),
        request_id,
        preview_id,
        collision_plan: CollisionPlan::default(),
        authorization: RestoreAuthorization {
            destructive_action_confirmed: false,
            safety_backup: SafetyBackupEvidence::NotRequired,
            prospective_failure_receipt_token: None,
            collision_plan_sha256: None,
        },
    };
    let caller_app = app.clone();
    let caller = tokio::spawn(async move { caller_app.execute(command).await });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    let caller = if abandon {
        caller.abort();
        assert!(caller.await.is_err_and(|error| error.is_cancelled()));
        None
    } else {
        cancellation.cancel();
        Some(caller)
    };
    tokio::task::spawn_blocking(move || hook.release()).await?;
    let committed = phase == CommandCommitTestPhase::AfterFinalCancellationCheck;
    if let Some(caller) = caller {
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), caller).await??;
        if committed {
            let revision = app
                .application_settings_snapshot()
                .await
                .boxed()?
                .library_revision;
            assert!(matches!(
                result.boxed()?,
                AppCommandResult::PortableApplied {
                    library_revision,
                    safety_backup: SafetyBackupOutcome::NotRequired,
                    ..
                } if library_revision == revision
            ));
        } else {
            assert!(matches!(
                result,
                Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled"
            ));
        }
    }
    if committed {
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
            .await?
            .boxed()?;
        assert!(matches!(
            event.payload,
            EventPayload::ScenarioChanged { context, .. }
                if context.request_id == Some(request_id)
                    && context.scenario_id == Some(restored_id)
        ));
    }
    assert_portable_boundary_state(&app, restored_id, committed).await?;
    drop(app);
    drop(store);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_portable_boundary_state(&reopened, restored_id, committed).await
}

#[cfg(debug_assertions)]
async fn assert_portable_boundary_state(
    app: &EuthetoApp,
    restored_id: ScenarioId,
    committed: bool,
) -> Result<(), Box<dyn Error>> {
    match app.query(AppQuery::ProjectMetadata(restored_id)).await {
        Ok(AppQueryResult::Project(project)) if committed => {
            assert_eq!(project.scenario_id, restored_id);
        }
        Err(AppError::NotFound(_)) if !committed => {}
        other => return Err(format!("unexpected portable boundary project: {other:?}").into()),
    }
    match app
        .query(AppQuery::Setting("appearance".to_owned()))
        .await
        .boxed()?
    {
        AppQueryResult::Setting(Some(setting)) if committed => {
            assert_eq!(
                setting.value,
                json!({"theme": "dark", "reducedMotion": true})
            );
        }
        AppQueryResult::Setting(None) if !committed => {}
        other => return Err(format!("unexpected portable boundary settings: {other:?}").into()),
    }
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn portable_child_cancel_before_final_check_rolls_back() -> Result<(), Box<dyn Error>> {
    portable_commit_boundary(CommandCommitTestPhase::BeforeFinalCancellationCheck, false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn portable_child_cancel_after_final_check_preserves_commit() -> Result<(), Box<dyn Error>> {
    portable_commit_boundary(CommandCommitTestPhase::AfterFinalCancellationCheck, false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn portable_abandoned_caller_before_final_check_rolls_back() -> Result<(), Box<dyn Error>> {
    portable_commit_boundary(CommandCommitTestPhase::BeforeFinalCancellationCheck, true).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn portable_abandoned_caller_after_final_check_publishes_commit() -> Result<(), Box<dyn Error>>
{
    portable_commit_boundary(CommandCommitTestPhase::AfterFinalCancellationCheck, true).await
}

// A bundle-wide result/source map must not rewrite foreign ancestry in an un-copied scenario.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn workforce_mixed_import_preserves_foreign_current_and_historical_ancestry()
-> Result<(), Box<dyn Error>> {
    let decode = |wire: &eutheto_export::PortableScenario| {
        portable_decode::decode_fixture_domain_with_pack(wire, &eutheto_workforce::WorkforcePack)
    };
    let source_directory = private_tempdir()?;
    let source_dependencies = dependencies(&source_directory)?;
    let (source_store, initialization) =
        SqliteScenarioStore::open(&source_dependencies.paths.database).await?;
    let source_store = Arc::new(source_store);
    let document = workforce_adoption_document()?;
    let source_id = document.scenario_id;
    source_store.create_project(NewProject { document }).await?;
    let source = EuthetoApp::from_initialized_store(
        Arc::clone(&source_store),
        initialization,
        source_dependencies,
    )
    .boxed()?;
    let accepted =
        persist_workforce_accepted_result(&source_store, source_id, Revision::INITIAL, 71).await?;
    let accepted_ref = AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?;
    source
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            source_id,
            Revision::INITIAL,
            accepted_ref.clone(),
            false,
        )?))
        .await
        .boxed()?;
    let source_project = source_store.get_project(source_id).await?;
    let source_base = stored_workforce_base(&source_project.document)?;
    let foreign_id = match source
        .execute(AppCommand::DuplicateProject {
            request_id: request_id()?,
            source_id,
            expected_revision: source_project.summary.revision,
            title: "Foreign ancestry".to_owned(),
        })
        .await
        .boxed()?
    {
        AppCommandResult::Project(project) => project.scenario_id,
        other => return Err(format!("unexpected duplicate: {other:?}").into()),
    };
    let bytes = match source
        .query(AppQuery::ExportBackup {
            cancellation: source.setup_cancellation(),
            title: "Mixed Workforce ancestry".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected backup: {other:?}").into()),
    };
    let mut inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &decode,
    )?;
    let foreign = inspected
        .scenarios
        .iter_mut()
        .find(|snapshot| snapshot.document.scenario_id == foreign_id)
        .ok_or("foreign scenario missing")?;
    let foreign_base = stored_workforce_base(&foreign.document)?;
    // This imported foreign claim has no local admission receipt or native acceptance authority.
    let mut manual = eutheto_workforce::model::ManualSelection {
        schema_version: 1,
        id: EntityId::from_uuid(Uuid::now_v7()),
        original: eutheto_domain_ir::RunBaselineOriginal {
            entity: eutheto_workforce::assignment_rules::workforce_baseline_reference(
                foreign_base.id,
                false,
            )?,
            pair_content_hash: eutheto_workforce::assignment_rules::workforce_pair_content_hash(
                &source_base.assignments,
                None,
            )?,
            accepted_result: accepted_ref.clone(),
            source_scenario_id: source_id,
            source_revision: accepted.portable.run_input.scenario_revision,
            source_document_hash: accepted.portable.run_input.snapshot_document_hash.clone(),
            source_run_id: accepted.portable.run_input.run_id,
            source_run_input_checksum: accepted.portable.run_input.checksum.clone(),
            source_run_manifest_checksum: accepted.portable.run_manifest.checksum.clone(),
        },
        admission_source_scenario_id: source_id,
        admission_source_revision: Revision::INITIAL,
        admission_source_document_hash: accepted.portable.run_input.snapshot_document_hash.clone(),
        admission_command_id: CommandId::from_uuid(solution_test_id(0xaaaa, 79)?),
        assignments: foreign_base.assignments.clone(),
        checksum: String::new(),
    };
    // Inert historical endpoints may name another family's person, shift or result UUID.
    let ancestral_pair = *source_base.assignments.first().ok_or("source base pair")?;
    manual.assignments.extend([
        ancestral_pair,
        eutheto_workforce::model::AssignmentPair {
            person_id: eutheto_types::PersonId::from_uuid(accepted_ref.solution_id.as_uuid()),
            shift_id: ancestral_pair.shift_id,
        },
    ]);
    manual.assignments.sort_unstable();
    manual.checksum =
        eutheto_workforce::assignment_rules::workforce_manual_selection_checksum(&manual, None)?;
    let mut manual_raw = serde_json::to_value(
        eutheto_workforce::model::WorkforceEntity::ManualSelection(manual.clone()),
    )?;
    manual_raw["admissionCommandId"] =
        json!(manual.admission_command_id.to_string().to_uppercase());
    foreign
        .document
        .domain
        .entities
        .insert(manual.id, manual_raw.clone());
    let mut historical_foreign = foreign.clone();
    historical_foreign.project = None;
    foreign.revision = Revision::new(1);
    foreign.document.metadata.title = "Current foreign ancestry".to_owned();
    inspected.scenario_revisions.push(historical_foreign);
    let mut sections = BackupSections {
        results: inspected
            .additional_entries
            .iter()
            .filter_map(|(path, bytes)| {
                path.strip_prefix("results/")
                    .and_then(|key| key.strip_suffix(".json"))
                    .map(|key| serde_json::from_slice(bytes).map(|value| (key.to_owned(), value)))
            })
            .collect::<Result<_, serde_json::Error>>()?,
        ..BackupSections::default()
    };
    // Existing opaque retained-result format supplies history dependency only, never acceptance.
    let history_reference_id = Uuid::now_v7();
    sections.results.insert(
        history_reference_id.to_string(),
        json!({
            "resultId": history_reference_id,
            "scenarioId": foreign_id,
            "scenarioRevision": Revision::INITIAL,
            "result": {},
        }),
    );
    let bundle = assemble_full_backup(
        &FullBackupSnapshot {
            bundle_id: inspected.manifest.bundle_id,
            created_at: inspected.manifest.created_at,
            application: inspected.manifest.application,
            title: "Mixed current/historical ancestry".to_owned(),
            scenarios: inspected.scenarios,
            scenario_revisions: inspected.scenario_revisions,
            sections,
            nonsemantic_extensions: inspected.manifest.nonsemantic_extensions,
            manifest_extensions: complete_full_backup_extensions()?,
        },
        &|document| {
            eutheto_domain_api::DomainPack::export_portable(
                &eutheto_workforce::WorkforcePack,
                document,
            )
            .map_err(|error| eutheto_export::ExportError::InvalidModel(error.to_string()))
        },
    )?;
    let target_directory = private_tempdir()?;
    let target_dependencies = dependencies(&target_directory)?;
    let (target_store, initialization) =
        SqliteScenarioStore::open(&target_dependencies.paths.database).await?;
    let target_store = Arc::new(target_store);
    target_store
        .create_project(NewProject {
            document: source_project.document,
        })
        .await?;
    let target = EuthetoApp::from_initialized_store(
        Arc::clone(&target_store),
        initialization,
        target_dependencies,
    )
    .boxed()?;
    let (preview_id, _) = preview_add_backup(&target, bundle).await?;
    apply_restore_with_authorization(
        &target,
        preview_id,
        CollisionPlan {
            scenarios: BTreeMap::from([(source_id, CollisionAction::CreateCopy)]),
            ..CollisionPlan::default()
        },
        SafetyBackupEvidence::NotRequired,
        None,
    )
    .await?
    .boxed()?;
    let current_foreign = target_store.get_project(foreign_id).await?;
    assert_eq!(
        stored_workforce_manual(&current_foreign.document)?.1,
        manual
    );
    assert_eq!(
        current_foreign.document.domain.entities.get(&manual.id),
        Some(&manual_raw)
    );
    assert_eq!(
        stored_workforce_base(&current_foreign.document)?.source_solution_id,
        accepted_ref.solution_id,
    );
    let exported = match target
        .query(AppQuery::ExportBackup {
            cancellation: target.setup_cancellation(),
            title: "Imported ancestry".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::BackupBundle { bytes, .. } => bytes,
        other => return Err(format!("unexpected imported backup: {other:?}").into()),
    };
    let reopened = inspect_bundle(
        &exported,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &decode,
    )?;
    let historical = reopened
        .scenario_revisions
        .iter()
        .find(|snapshot| {
            snapshot.document.scenario_id == foreign_id && snapshot.revision == Revision::INITIAL
        })
        .ok_or("imported foreign historical revision missing")?;
    assert_eq!(stored_workforce_manual(&historical.document)?.1, manual);
    assert_eq!(
        historical.document.domain.entities.get(&manual.id),
        Some(&manual_raw)
    );
    assert_eq!(
        stored_workforce_base(&historical.document)?.source_solution_id,
        accepted_ref.solution_id,
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn workforce_fresh_restore_preserves_portable_sources_without_promoting_results()
-> Result<(), Box<dyn Error>> {
    let decode = |wire: &eutheto_export::PortableScenario| {
        portable_decode::decode_fixture_domain_with_pack(wire, &eutheto_workforce::WorkforcePack)
    };
    let source_directory = private_tempdir()?;
    let source_dependencies = dependencies(&source_directory)?;
    let (source_store, initialization) =
        SqliteScenarioStore::open(&source_dependencies.paths.database).await?;
    let source_store = Arc::new(source_store);
    let original = workforce_adoption_document()?;
    let scenario_id = original.scenario_id;
    source_store
        .create_project(NewProject {
            document: original.clone(),
        })
        .await?;
    let source = EuthetoApp::from_initialized_store(
        Arc::clone(&source_store),
        initialization,
        source_dependencies,
    )
    .boxed()?;
    let accepted =
        persist_workforce_accepted_result(&source_store, scenario_id, Revision::INITIAL, 81)
            .await?;
    let accepted_id = accepted.portable.accepted_result.solution.solution_id;
    source
        .execute(AppCommand::AdoptBaseSchedule(adoption_request(
            scenario_id,
            Revision::INITIAL,
            AcceptedResultRefV1::from_result(&accepted.portable.accepted_result)?,
            false,
        )?))
        .await
        .boxed()?;
    let external_id: EntityId = workforce_fixture::id(50).parse()?;
    let external = json!({
        "kind":"externalStartingPlan", "schemaVersion":1, "id":external_id,
        "name":"Unverified imported plan",
        "assignments":[{"personId":workforce_fixture::id(12),"shiftId":workforce_fixture::id(8)}],
        "originalImport":{"sourceBlake3":"a".repeat(64),"pairContentHash":"b".repeat(64)}
    });
    source
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::new(1),
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Portable integration test".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::ADD_ENTITY.to_owned(),
                    payload: json!({"entity":external}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .boxed()?;
    let AppQueryResult::BackupBundle { bytes, .. } = source
        .query(AppQuery::ExportBackup {
            cancellation: source.setup_cancellation(),
            title: "Workforce portable sources".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    else {
        return Err("expected Workforce backup".into());
    };
    let mut inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &decode,
    )?;
    let current = inspected
        .scenarios
        .first_mut()
        .ok_or("missing current source")?;
    assert_eq!(
        current.document,
        source_store.get_project(scenario_id).await?.document
    );
    current.extensions.insert(
        "vendor.asset-reference".to_owned(),
        json!({"assetKey":"portable-note.txt"}),
    );
    let expected_current = current.clone();
    let accepted_value = serde_json::to_value(&accepted.portable)?;
    let sections = BackupSections {
        results: BTreeMap::from([(accepted_id.to_string(), accepted_value.clone())]),
        assets: BTreeMap::from([(
            "portable-note.txt".to_owned(),
            PortableAsset {
                bytes: b"Permitted portable attachment".to_vec(),
                media_type: "text/plain".to_owned(),
                redistribution_permitted: true,
            },
        )]),
        ..BackupSections::default()
    };
    let bundle = assemble_full_backup(
        &FullBackupSnapshot {
            bundle_id: inspected.manifest.bundle_id,
            created_at: inspected.manifest.created_at,
            application: inspected.manifest.application,
            title: "Workforce retained sources".to_owned(),
            scenarios: inspected.scenarios,
            scenario_revisions: inspected.scenario_revisions,
            sections,
            nonsemantic_extensions: inspected.manifest.nonsemantic_extensions,
            manifest_extensions: complete_full_backup_extensions()?,
        },
        &|document| {
            eutheto_domain_api::DomainPack::export_portable(
                &eutheto_workforce::WorkforcePack,
                document,
            )
            .map_err(|error| eutheto_export::ExportError::InvalidModel(error.to_string()))
        },
    )?;
    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let (preview_id, _) = preview_add_backup(&target, bundle).await?;
    apply_restore_with_authorization(
        &target,
        preview_id,
        CollisionPlan::default(),
        SafetyBackupEvidence::NotRequired,
        None,
    )
    .await?
    .boxed()?;
    drop(target);
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let (reopened_store, _) =
        SqliteScenarioStore::open(target_directory.path().join("eutheto.sqlite")).await?;
    let restored = reopened_store
        .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
        .await?;
    assert!(restored.canonical_result_ids.is_empty());
    assert!(matches!(
        reopened_store.load_accepted_result(accepted_id).await,
        Err(eutheto_store::StoreError::AcceptedResultNotFound(id)) if id == accepted_id
    ));
    for query in [
        AppQuery::ExportScenario {
            scenario_id,
            cancellation: target.setup_cancellation(),
        },
        AppQuery::ExportBackup {
            cancellation: target.setup_cancellation(),
            title: "Restored Workforce backup".to_owned(),
            selection: BackupSelection::default(),
        },
    ] {
        let bytes = match target.query(query).await.boxed()? {
            AppQueryResult::Bundle { bytes, .. } | AppQueryResult::BackupBundle { bytes, .. } => {
                bytes
            }
            other => return Err(format!("unexpected restored export: {other:?}").into()),
        };
        let roundtrip = inspect_bundle(
            &bytes,
            &InspectionPolicy::default(),
            &MigrationRegistries::default(),
            &decode,
        )?;
        let current = roundtrip
            .scenarios
            .first()
            .ok_or("missing restored source")?;
        assert_eq!(current.document, expected_current.document);
        assert_eq!(current.revision, Revision::new(2));
        assert_eq!(current.extensions, expected_current.extensions);
        assert_eq!(
            current.document.domain.entities.get(&external_id),
            Some(&external)
        );
        assert_eq!(
            stored_workforce_base(&current.document)?.source_solution_id,
            accepted_id
        );
        let historical = roundtrip
            .scenario_revisions
            .iter()
            .find(|snapshot| {
                snapshot.document.scenario_id == scenario_id
                    && snapshot.revision == Revision::INITIAL
            })
            .ok_or("missing original accepted source")?;
        assert_eq!(historical.document, original);
        let bytes = roundtrip
            .additional_entries
            .get(&format!("results/{accepted_id}.json"))
            .ok_or("missing archived result")?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(bytes)?,
            accepted_value
        );
        assert_eq!(
            roundtrip.additional_entries.get("assets/portable-note.txt"),
            Some(&b"Permitted portable attachment".to_vec())
        );
    }
    Ok(())
}

#[cfg(debug_assertions)]
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn migrated_workforce_unaccepted_result_is_preserved_in_backup() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let deps = dependencies(&directory)?;
    std::fs::write(
        &deps.paths.database,
        include_bytes!("../../../tests/migration/fixtures/database_v1.sqlite3"),
    )?;
    let document = workforce_adoption_document()?;
    let scenario_id = document.scenario_id;
    let run_id = SolveRunId::new(&SystemIdGenerator)?;
    let result_id = SolutionId::new(&SystemIdGenerator)?;
    let created_at = "2026-08-28T23:00:00Z";
    let raw_solution = json!({
        "legacy":true,
        "scenarioId":scenario_id,
        "entityId":workforce_fixture::id(1),
    });
    let connection = Connection::open(&deps.paths.database)?;
    connection.execute(
        "INSERT INTO scenarios (id, domain_pack_id, domain_schema_version, title, description, revision, document_json, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4, 0, ?5, ?6, ?6)",
        rusqlite::params![
            scenario_id.to_string(),
            document.domain_pack.id.to_string(),
            document.metadata.title,
            document.metadata.description,
            serde_json::to_string(&document)?,
            created_at,
        ],
    )?;
    connection.execute(
        "INSERT INTO scenario_history_state (scenario_id, cursor_sequence, branch_generation) VALUES (?1, 0, 0)",
        [scenario_id.to_string()],
    )?;
    connection.execute(
        "INSERT INTO solve_runs (id, scenario_id, scenario_revision, input_hash, backend_id, backend_version, status, options_json, started_at) VALUES (?1, ?2, 0, 'legacy-hash', 'legacy.backend', '1', 'completed', '{\"legacy\":true}', ?3)",
        rusqlite::params![run_id.to_string(), scenario_id.to_string(), created_at],
    )?;
    connection.execute(
        "INSERT INTO solutions (id, solve_run_id, scenario_id, scenario_revision, status, accepted, normalized_solution_json, score_json, verification_report_json, explanation_index_json, created_at) VALUES (?1, ?2, ?3, 0, 'accepted', 1, ?4, '{\"legacy\":true}', '{\"legacy\":true}', 'null', ?5)",
        rusqlite::params![
            result_id.to_string(),
            run_id.to_string(),
            scenario_id.to_string(),
            serde_json::to_string(&raw_solution)?,
            created_at,
        ],
    )?;
    drop(connection);
    let app = EuthetoApp::open(deps).await.boxed()?;
    let connection = Connection::open(directory.path().join("eutheto.sqlite"))?;
    let recorded: (String, bool) = connection.query_row(
        "SELECT status, accepted FROM solutions WHERE id = ?1",
        [result_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(recorded, ("legacy_unverified".to_owned(), false));
    drop(connection);
    let AppQueryResult::BackupBundle { bytes, .. } = app
        .query(AppQuery::ExportBackup {
            cancellation: app.setup_cancellation(),
            title: "Migrated Workforce result".to_owned(),
            selection: BackupSelection::default(),
        })
        .await
        .boxed()?
    else {
        return Err("expected migrated Workforce backup".into());
    };
    let inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy::default(),
        &MigrationRegistries::default(),
        &|wire| {
            portable_decode::decode_fixture_domain_with_pack(
                wire,
                &eutheto_workforce::WorkforcePack,
            )
        },
    )?;
    let result_bytes = inspected
        .additional_entries
        .get(&format!("results/{result_id}.json"))
        .ok_or("results-inclusive backup omitted the migrated unaccepted result")?;
    let archived: serde_json::Value = serde_json::from_slice(result_bytes)?;
    assert_eq!(archived["archive"]["status"], recorded.0);
    assert_eq!(archived["archive"]["accepted"], recorded.1);
    assert_eq!(archived["archive"]["normalizedSolution"], raw_solution);
    assert!(
        archived["archive"]
            .as_object()
            .ok_or("archive object")?
            .contains_key("explanationIndex")
    );
    assert_eq!(
        archived["archive"]["explanationIndex"],
        serde_json::Value::Null
    );
    assert!(
        !archived["archive"]
            .as_object()
            .ok_or("archive object")?
            .contains_key("evidence")
    );
    for action in [
        CollisionAction::Skip,
        CollisionAction::Replace,
        CollisionAction::CreateCopy,
    ] {
        let (preview_id, preview) = preview_add_backup(&app, bytes.clone()).await?;
        let mut plan = CollisionPlan {
            scenarios: BTreeMap::from([(scenario_id, action)]),
            ..CollisionPlan::default()
        };
        for identity in preview.supplemental_collisions {
            plan.supplemental.insert(
                identity,
                if action == CollisionAction::Skip {
                    eutheto_import::SupplementalCollisionAction::Skip
                } else {
                    eutheto_import::SupplementalCollisionAction::Replace
                },
            );
        }
        apply_restore_with_authorization(
            &app,
            preview_id,
            plan,
            SafetyBackupEvidence::NotRequired,
            None,
        )
        .await?
        .map_err(|error| format!("{action:?}: {error:?}"))?;
    }
    let (source_store, _) =
        SqliteScenarioStore::open(directory.path().join("eutheto.sqlite")).await?;
    let same_library = source_store
        .library_snapshot(eutheto_store::NativeArchivalSelection::All)
        .await?;
    assert_eq!(same_library.sections.results.len(), 2);
    let mut copied = None;
    for result in same_library.sections.results.values() {
        let result: serde_json::Value = serde_json::from_slice(result)?;
        assert_eq!(result["archive"], archived["archive"]);
        if result["resultId"] != archived["resultId"] {
            copied = Some(result);
        }
    }
    let copied = copied.ok_or("same-library copy omitted archived result")?;
    assert_ne!(copied["scenarioId"], archived["scenarioId"]);
    let connection = Connection::open(directory.path().join("eutheto.sqlite"))?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM solutions", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    drop(connection);
    let target_directory = private_tempdir()?;
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let (preview_id, _) = preview_add_backup(&target, bytes).await?;
    apply_restore_with_authorization(
        &target,
        preview_id,
        CollisionPlan::default(),
        SafetyBackupEvidence::NotRequired,
        None,
    )
    .await?
    .boxed()?;
    drop(target);
    let target = EuthetoApp::open(dependencies(&target_directory)?)
        .await
        .boxed()?;
    let (store, _) =
        SqliteScenarioStore::open(target_directory.path().join("eutheto.sqlite")).await?;
    assert!(matches!(
        store.load_accepted_result(result_id).await,
        Err(eutheto_store::StoreError::AcceptedResultNotFound(id)) if id == result_id
    ));
    for query in [
        AppQuery::ExportScenario {
            scenario_id,
            cancellation: target.setup_cancellation(),
        },
        AppQuery::ExportBackup {
            title: "Restored legacy Workforce result".to_owned(),
            selection: BackupSelection::default(),
            cancellation: target.setup_cancellation(),
        },
    ] {
        let bytes = match target.query(query).await.boxed()? {
            AppQueryResult::Bundle { bytes, .. } | AppQueryResult::BackupBundle { bytes, .. } => {
                bytes
            }
            other => return Err(format!("unexpected restored export: {other:?}").into()),
        };
        let roundtrip = inspect_bundle(
            &bytes,
            &InspectionPolicy::default(),
            &MigrationRegistries::default(),
            &|wire| {
                portable_decode::decode_fixture_domain_with_pack(
                    wire,
                    &eutheto_workforce::WorkforcePack,
                )
            },
        )?;
        assert_eq!(roundtrip.scenarios[0].document, document);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                roundtrip
                    .additional_entries
                    .get(&format!("results/{result_id}.json"))
                    .ok_or("restored legacy result omitted")?
            )?,
            archived,
        );
    }
    Ok(())
}
