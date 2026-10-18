use crate::counterfactual::{
    COUNTERFACTUAL_API_SCHEMA_VERSION, CounterfactualRuntime, CounterfactualRuntimeServices,
    SolutionCancelCounterfactualDtoV1, SolutionCancelCounterfactualRequestV1,
    SolutionStartCounterfactualDtoV1, SolutionStartCounterfactualRequestV1,
};
use eutheto_command::{CommandError, apply_command_with_registry, validate_document_shape};
use eutheto_domain_api::{
    DomainCatalog, DomainPackDescriptor, DomainPackError, DomainPackRegistry,
    DomainSetupQueryV1 as AcceptedDomainQueryV1, DomainView, PortableImportContext,
    SetupContinuationV1 as AcceptedViewContinuationV1, SetupViewContext,
    bounded_json_size as measure_accepted_view_json_size,
};
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, AssignmentEvidenceV1, EvidenceRenderRequestV2,
    ExplanationEvidencePayloadV2, ExplanationEvidenceV2, ExplanationRequestSubjectV1,
    ExplanationRequestV1, ExplanationResultV2, ExplanationValidationSeverity,
    OptimalityStatusEvidence, PortableAcceptedResult, RepairCausalityV1, RepairEvidenceV2,
    RunBaselineOriginal, RunInput, ScoreVector, SolutionComparisonV2, ValidationIssueEvidenceV1,
    VerificationContextV1, VerificationReport, VerifiedCheckpoint, blake3_hex,
    prepare_comparison_v2, validation_issue_identity,
};
use eutheto_export::{
    ApplicationMetadata, BACKUP_SELECTION_EXTENSION, BACKUP_SELECTION_VERSION, BackupSections,
    BackupSelection as PortableBackupSelectionMetadata, BackupSelectionScope, BundleKind,
    ExportError, FixedExclusion, FullBackupSnapshot, OmittedAssetReason,
    PortableBackupAssetSelection, PortableScenario, ScenarioExportSnapshot, assemble_full_backup,
    assemble_scenario_export, backup_selection_extension_value, omitted_asset_placeholder,
    parse_omitted_asset_placeholder, prepare_bundle_atomic_cancellable,
    write_bundle_atomic_cancellable,
};
use eutheto_import::{
    AppliedMigration, CollisionPlan, DecodedPortableDomain, ImportOptions, ImportPreview,
    InspectedBundle, InspectionPolicy, LocalLibrarySnapshot, LocalScenarioSnapshot,
    MigrationFailure, MigrationRegistries, MigrationRegistryKind, MigrationSubject,
    PackMigrationVersionSpace, PortableMigrationOutcome, PortableMigrationStep,
    PreservedBundleMetadata, RestoreAuthorization, RestoreMode, SafetyBackupEvidence,
    StagedDisposition, UnopenedBundle, inspect_bundle, inspect_unopened_bundle_for_exact_reexport,
    preflight_bundle_metadata,
};
pub use eutheto_solver_api::{
    BackendSupportColumn, CapabilityMatrix, SupportCell, SupportFeature, SupportFeatureCategory,
    SupportFeatureGate, SupportFeatureId,
};
use eutheto_solver_api::{DeferredBackendCandidate, SolverDescriptor, SolverRegistry};
use eutheto_store::{
    AppSetting, CommandWrite, CounterfactualJobListCursorV1, CounterfactualJobListPageV1,
    HistoryCommand, HistoryEntry, InitializationOutcome, JournalWrite, NewProject, NewSolveRunV1,
    ProjectListScope, RedoBranchPolicy, SafetyBackupFailureReceipt, SqliteScenarioStore,
    StagedLibraryApply, StoreError, StoredAcceptedResultSummary, StoredAcceptedResultV2,
    StoredProject,
};
pub use eutheto_store::{ExistingSolveRunV1, MAX_SOLVE_RUN_LIST_LIMIT, SolveRunListRequestV1};
use eutheto_types::{
    ActorRef, AppError, ApplicationSettingsWriteResultV1, BackendId, BundleId, CancellationToken,
    Change, ChangeKind, ChangeSet, Clock, CommandEnvelope, CommandId, CommandResult, CommandSource,
    CounterfactualJobId, DirectoryAvailabilityLabel, DomainCommandEnvelope, DomainPackRef,
    EntityId, EventContext, EventPayload, EventTopic, IdGenerator, KnownVerificationAlarmV1,
    MonotonicClock, OperationControl, OperationInterruption, PROJECT_LIST_SCHEMA_VERSION, PackId,
    PortableAsset, PortableDomainDocument, PortableProjectMetadata, ProjectListItemV1,
    ProjectMetadataDto, ProtocolFailure, RequestId, ResourceRef, Revision, Rfc3339Timestamp,
    SCENARIO_FORMAT_VERSION, SUPPORT_PREVIEW_SCHEMA_VERSION, ScenarioCommand, ScenarioDocument,
    ScenarioDomain, ScenarioId, ScenarioMetadata, ScenarioSettings, ScenarioSnapshotV1,
    ScenarioViewDto, SolutionId, SolvePhase, SolveRunId, SolveStatus, StorageFailure,
    SupportApplicationMetadataDto, SupportDirectoryMetadataDto, SupportLibraryMetadataDto,
    SupportPreviewDto, SupportSchemaMetadataDto, UnsupportedFeature, ValidationIssue,
    ValidationReport, ValidationSeverity, VerificationAlarmTerminalization, VerificationFailure,
    collect_scenario_owned_uuids, extract_asset_references, extract_result_dependency,
    extract_result_id, extract_scenario_references,
};
use eutheto_workforce::{
    assignment_rules::decode_workforce_assignment,
    commands::{ADD_ENTITY, EntityPayload, UPDATE_ENTITY},
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{AssignmentPair, BaseSchedule, WorkforceEntity},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, broadcast};
use uuid::Uuid;

#[path = "application_settings.rs"]
mod application_settings;
use application_settings::PendingSettingsPreview;
pub use application_settings::*;

#[path = "people_csv.rs"]
mod people_csv;
pub use people_csv::*;

#[path = "headless.rs"]
mod headless;
pub use headless::*;

#[path = "model_estimate.rs"]
mod model_estimate;
pub use model_estimate::*;

#[path = "setup.rs"]
mod setup;
pub use setup::*;

#[path = "history.rs"]
mod history;
pub use history::*;
#[path = "manual.rs"]
mod manual;
pub use manual::*;
#[path = "recipient_pdf.rs"]
mod recipient_pdf;
#[path = "recipient_report.rs"]
mod recipient_report;
pub use recipient_pdf::*;
pub use recipient_report::*;

#[path = "result_publication.rs"]
mod result_publication;

#[path = "schedule_export.rs"]
mod schedule_export;
pub use schedule_export::*;

#[path = "recipient_memory.rs"]
pub(crate) mod recipient_memory;

#[path = "diagnostic_actions.rs"]
mod diagnostic_actions;
pub use diagnostic_actions::{
    CapturedDiagnosticExport, DIAGNOSTIC_COPY_API_SCHEMA_VERSION,
    DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION, DIAGNOSTIC_REPORT_SCHEMA_VERSION,
    DiagnosticCopyRequestV1, DiagnosticCopyResultV1, DiagnosticExportMetadataV1,
    DiagnosticExportRequestV1, DiagnosticReport, MAX_DIAGNOSTIC_REPORT_BYTES,
    PreparedDiagnosticExport,
};

const EVENT_VERSION: u32 = 1;
/// Current application solution-read wire schema.
pub const SOLUTION_API_SCHEMA_VERSION: u32 = 1;
/// Current explicit accepted-result base-adoption request wire schema.
pub const SOLUTION_ADOPT_BASE_SCHEMA_VERSION: u32 = 1;
/// Current complete comparison wire schema; older general requests are refused.
pub const SOLUTION_COMPARE_SCHEMA_VERSION: u32 = 3;
const MAX_PENDING_PREVIEWS: usize = 3;
const MAX_ID_ALLOCATION_ATTEMPTS: usize = 64;
const MAX_PENDING_PREVIEW_BYTES: usize = 64 * 1024 * 1024;
const PORTABLE_SETTINGS_SECTION: &str = "application-settings";
const REPLACE_WITHOUT_BACKUP_PHRASE: &str = "REPLACE WITHOUT BACKUP";
const COLLISION_PLAN_SHA256_DOMAIN: &[u8] = b"eutheto/collision-plan-sha256/v1\0";
type ScenarioMutex = Arc<Mutex<()>>;
type ScenarioLocks = Arc<Mutex<BTreeMap<ScenarioId, ScenarioMutex>>>;
fn collision_plan_sha256(collision_plan: &CollisionPlan) -> Result<String, AppError> {
    let canonical =
        eutheto_export::canonical_json(collision_plan).map_err(|error| export_error(&error))?;
    let mut domain_separated =
        Vec::with_capacity(COLLISION_PLAN_SHA256_DOMAIN.len() + canonical.len());
    domain_separated.extend_from_slice(COLLISION_PLAN_SHA256_DOMAIN);
    domain_separated.extend_from_slice(&canonical);
    Ok(eutheto_export::sha256_hex(&domain_separated))
}
fn validated_static_pack_registry() -> Result<DomainPackRegistry, AppError> {
    let builder = DomainPackRegistry::builder().register(eutheto_workforce::WorkforcePack);
    #[cfg(debug_assertions)]
    let builder = builder.register(eutheto_command::OfficialTestPack);
    builder.build().map_err(|_| {
        protocol_error(
            "application.pack_registry_invalid",
            "Compiled domain-pack metadata failed its startup invariant.",
            false,
        )
    })
}

fn validated_static_registries() -> Result<(Arc<DomainPackRegistry>, Arc<SolverRegistry>), AppError>
{
    let packs = validated_static_pack_registry()?;
    let solvers = SolverRegistry::production().map_err(|_| {
        protocol_error(
            "application.solver_registry_invalid",
            "Compiled solver metadata failed its startup invariant.",
            false,
        )
    })?;
    Ok((Arc::new(packs), Arc::new(solvers)))
}

/// Host-resolved filesystem locations. Core never discovers platform paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    pub database: PathBuf,
    pub safety_backups: PathBuf,
}

/// Explicit dependencies used by the platform-neutral application service.
#[derive(Clone)]
pub struct AppDependencies {
    pub paths: AppPaths,
    pub clock: Arc<dyn Clock>,
    pub monotonic_clock: Arc<dyn MonotonicClock>,
    pub ids: Arc<dyn IdGenerator>,
    pub cancellation: CancellationToken,
}

/// Scope used by the project-list application query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectScope {
    Active,
    Archived,
    All,
}
/// Asset inclusion policy for a portable full backup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupAssetSelection {
    ExcludeAll,
    IncludeAll,
    IncludeUnderThreshold,
}

/// Explicit full-backup section selection. Full backup is the default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackupSelection {
    pub include_results: bool,
    pub assets: BackupAssetSelection,
    pub include_audit: bool,
}

impl Default for BackupSelection {
    fn default() -> Self {
        Self {
            include_results: true,
            assets: BackupAssetSelection::IncludeAll,
            include_audit: false,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupSummary {
    pub include_results: bool,
    pub asset_selection: BackupAssetSelection,
    pub fixed_exclusions: BTreeSet<FixedExclusion>,
    pub excluded_asset_count: u64,
    pub excluded_asset_ids: Vec<String>,
    pub exclusion_scope: Option<String>,
}

/// Safety-backup evidence for a successfully committed portable restore.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SafetyBackupOutcome {
    NotRequired,
    CreatedAndVerified { artifact_name: String },
    ConfirmedBypass,
}

struct CreatedPortableSafetyBackup {
    sha256: String,
    artifact_name: String,
}

/// Deferred capability families with stable unavailable responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferredCapability {
    Solve,
    ArtificialIntelligence,
}

/// Revision and bundle-kind identity bound to reviewed portable bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedPortableBinding {
    Scenario {
        scenario_id: ScenarioId,
        expected_revision: Revision,
        expected_library_revision: Revision,
    },
    Backup {
        expected_library_revision: Revision,
    },
}
/// Complete data-only metadata for one compiled-in pack.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainPackMetadata {
    pub descriptor: DomainPackDescriptor,
    pub catalog: DomainCatalog,
}

/// Runtime-visible generated solver matrix metadata. Feature and backend order is stable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolverSupportMatrixMetadata {
    pub schema_version: u32,
    pub planning_ir_schema_version: u32,
    pub features: Vec<SupportFeature>,
    pub production_backend_ids: Vec<BackendId>,
    pub backend_columns: Vec<BackendSupportColumn>,
}

/// Lists locally accepted solutions for one scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionListRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
}
/// Reads one durable counterfactual job owned by the requested scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionGetCounterfactualRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub job_id: CounterfactualJobId,
}

/// Pages durable diagnostic jobs for one scenario, newest first.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionListCounterfactualsRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub before: Option<CounterfactualJobListCursorV1>,
    pub limit: u32,
}

/// Requests one locally accepted solution's summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionSummaryRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
}

/// Changes only the selected accepted-result viewing state for one scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionSelectRequestV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub solution_id: SolutionId,
}
/// Explicitly adopts one independently verified accepted result as the Workforce base schedule.
///
/// Selection is a read-only viewing state and never implies adoption. Adoption is an ordinary
/// revisioned scenario command and therefore participates in the normal journal/undo/redo path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionAdoptBaseRequestV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub command_id: CommandId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub result: AcceptedResultRefV1,
    pub actor: ActorRef,
    pub truncate_redo: bool,
}

/// Selects one pack-owned result view from an accepted solution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionViewRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
    pub view_id: String,
    #[serde(default = "default_solution_view_parameters")]
    pub parameters: Value,
    #[serde(default)]
    pub continuation: Option<AcceptedViewContinuationV1>,
}

fn default_solution_view_parameters() -> Value {
    Value::Object(serde_json::Map::new())
}
/// Requests current independent re-verification of one accepted solution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionVerifyRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
}

/// Selects two independently accepted sources for the complete current comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionCompareRequestV3 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub base_solution_id: SolutionId,
    pub candidate_solution_id: SolutionId,
}

/// Requests one typed explanation over retained local authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionExplainRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub request: ExplanationRequestV1,
}

/// Compact accepted-solution projection used by solution lists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionSummaryDtoV1 {
    pub schema_version: u32,
    pub result: AcceptedResultRefV1,
    pub run_id: SolveRunId,
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub current_revision: Revision,
    pub stale: bool,
    pub selected: bool,
    pub status: SolveStatus,
    pub score: ScoreVector,
    pub verification_checksum: String,
    pub finished_at: Rfc3339Timestamp,
}

/// Versioned list envelope for one scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionListDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    /// Scores and checksums are retained metadata, not freshly independently verified.
    pub freshly_verified: bool,
    pub solutions: Vec<SolutionSummaryDtoV1>,
}

/// Original scenario, revision and durable diagnostic ownership are always returned together.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionGetCounterfactualDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub job: eutheto_domain_ir::CounterfactualJobRecordV1,
}

/// Bounded persisted diagnostic page; events never substitute for this authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionListCounterfactualsDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub page: CounterfactualJobListPageV1,
}

/// Complete accepted-only result detail.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionDetailDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub scenario_revision: Revision,
    pub selected: bool,
    pub result: AcceptedResult,
}

/// Pack-owned view bound to an exact accepted scenario revision.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionViewDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub scenario_revision: Revision,
    pub result: AcceptedResultRefV1,
    pub view: DomainView,
}

/// Fresh independent verification bound to the selected stored result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionVerificationDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub scenario_revision: Revision,
    pub result: AcceptedResultRefV1,
    pub verification: VerificationReport,
}

/// Complete source-bound comparison with independently gated aggregate scores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionComparisonDtoV3 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub comparison: SolutionComparisonV2,
}

/// Rendered typed evidence and all selected recorded revisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionExplanationDtoV2 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub scenario_revisions: Vec<Revision>,
    pub explanation: ExplanationResultV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infeasibility_context: Option<InfeasibilityExplanationContextV1>,
}

/// Native custody references for actions on one retained diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InfeasibilityExplanationContextV1 {
    pub run_id: SolveRunId,
    pub source_revision: Revision,
    pub run_manifest_checksum: String,
    pub evidence_checksum: String,
    pub conflict_id: eutheto_domain_ir::DomainEvidenceId,
}

/// State-changing application operations.
#[derive(Clone, Debug)]
pub enum AppCommand {
    /// Create a project under a request correlation identity.
    CreateProject {
        request_id: RequestId,
        title: String,
        description: String,
        domain_pack: DomainPackRef,
        settings: ScenarioSettings,
    },
    DuplicateProject {
        request_id: RequestId,
        source_id: ScenarioId,
        expected_revision: Revision,
        title: String,
    },
    ArchiveProject {
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    },
    UnarchiveProject {
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    },
    DeleteProject {
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    },
    ApplyScenario {
        request_id: RequestId,
        envelope: CommandEnvelope,
        truncate_redo: bool,
    },
    CreateDiagnosticCopy {
        request: DiagnosticCopyRequestV1,
        control: OperationControl,
    },
    Undo {
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    },
    Redo {
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
    },
    SetSetting {
        request_id: RequestId,
        expected_library_revision: Revision,
        key: String,
        value: Value,
    },
    DeleteSetting {
        request_id: RequestId,
        expected_library_revision: Revision,
        key: String,
    },
    SolutionSelect(SolutionSelectRequestV1),
    AdoptBaseSchedule(SolutionAdoptBaseRequestV1),
    SolutionStartCounterfactual(SolutionStartCounterfactualRequestV1),
    SolutionCancelCounterfactual(SolutionCancelCounterfactualRequestV1),
    ExportScenario {
        scenario_id: ScenarioId,
        destination: PathBuf,
        cancellation: CancellationToken,
    },
    CreateBackup {
        title: String,
        destination: PathBuf,
        selection: BackupSelection,
        cancellation: CancellationToken,
    },
    PublishPreparedPortable {
        destination: PathBuf,
        bytes: Vec<u8>,
        expected_sha256: String,
        binding: PreparedPortableBinding,
        cancellation: CancellationToken,
    },
    ApplyImport {
        request_id: RequestId,
        preview_id: RequestId,
        collision_plan: CollisionPlan,
        cancellation: CancellationToken,
    },
    ApplyRestore {
        request_id: RequestId,
        preview_id: RequestId,
        collision_plan: CollisionPlan,
        authorization: RestoreAuthorization,
        cancellation: CancellationToken,
    },
    CancelPortablePreview {
        preview_id: RequestId,
    },
    /// Atomically writes and consumes one previously inspected unopened bundle.
    ExactReexportUnopenedBundle {
        preview_id: RequestId,
        destination: PathBuf,
        cancellation: CancellationToken,
    },
    Deferred(DeferredCapability),
}

/// Read-only application operations.
#[derive(Clone, Debug)]
pub enum AppQuery {
    ListProjects(ProjectScope),
    ProjectMetadata(ScenarioId),
    OpenProject(ScenarioId),
    ScenarioView(ScenarioId),
    ValidateScenario(ScenarioId),
    History(ScenarioId),
    HistoryPage(HistoryPageRequestV1),
    Setting(String),
    SolutionGetCounterfactual(SolutionGetCounterfactualRequestV1),
    SolutionListCounterfactuals(SolutionListCounterfactualsRequestV1),
    SolutionList(SolutionListRequestV1),
    SolutionGetSummary(SolutionSummaryRequestV1),
    SolutionGetView(SolutionViewRequestV1),
    SolutionVerify(SolutionVerifyRequestV1),
    SolutionCompareV3(SolutionCompareRequestV3),
    SolutionExplain {
        request: SolutionExplainRequestV1,
        source_revision: Revision,
        control: OperationControl,
    },
    PreviewImport {
        bytes: Vec<u8>,
        options: ImportOptions,
        cancellation: CancellationToken,
    },
    PreviewRestore {
        bytes: Vec<u8>,
        options: ImportOptions,
        cancellation: CancellationToken,
    },
    ExportScenario {
        scenario_id: ScenarioId,
        cancellation: CancellationToken,
    },
    ExportBackup {
        title: String,
        selection: BackupSelection,
        cancellation: CancellationToken,
    },
    SupportPreview,
    Deferred(DeferredCapability),
    ListDomainPacks,
    DescribeDomainPack(PackId),
    ListSolvers,
    DescribeSolver(BackendId),
    SolverSupportMatrix,
    DeferredSolverGates,
    /// Performs bounded archive/checksum inspection only and retains exact original bytes.
    InspectUnopenedBundle {
        bytes: Vec<u8>,
        cancellation: CancellationToken,
    },
}

/// Source and persisted identities for one applied portable scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppliedPortableScenario {
    pub source_scenario_id: ScenarioId,
    pub scenario_id: ScenarioId,
}

/// Results from state-changing application operations.
#[derive(Clone, Debug)]
pub enum AppCommandResult {
    Project(ProjectMetadataDto),
    Deleted,
    ScenarioCommand(CommandResult),
    DiagnosticCopy(DiagnosticCopyResultV1),
    SettingsWritten(ApplicationSettingsWriteResultV1),
    SolutionSelected(SolutionSummaryDtoV1),
    CounterfactualStarted(SolutionStartCounterfactualDtoV1),
    CounterfactualCancelled(SolutionCancelCounterfactualDtoV1),
    BundleWritten,
    BackupWritten(BackupSummary),
    PortableApplied {
        scenarios: Vec<AppliedPortableScenario>,
        library_revision: Revision,
        safety_backup: SafetyBackupOutcome,
    },
    PortablePreviewCancelled,
    UnopenedBundleReexported,
}

/// Results from read-only application operations.
#[derive(Clone, Debug)]
pub enum AppQueryResult {
    Projects(Vec<ProjectListItemV1>),
    Project(ProjectMetadataDto),
    Scenario(Box<ScenarioViewDto>),
    Validation(ValidationReport),
    History(Vec<HistoryEntry>),
    HistoryPage(HistoryPageDtoV1),
    Setting(Option<AppSetting<Value>>),
    SolutionGetCounterfactual(Box<SolutionGetCounterfactualDtoV1>),
    SolutionListCounterfactuals(Box<SolutionListCounterfactualsDtoV1>),
    SolutionList(SolutionListDtoV1),
    SolutionSummary(Box<SolutionDetailDtoV1>),
    SolutionView(Box<SolutionViewDtoV1>),
    SolutionVerification(Box<SolutionVerificationDtoV1>),
    SolutionComparisonV3(Box<SolutionComparisonDtoV3>),
    SolutionExplanation(Box<SolutionExplanationDtoV2>),
    PortablePreview {
        preview_id: RequestId,
        preview: Box<ImportPreview>,
    },
    SupportPreview(SupportPreviewDto),
    Bundle {
        bytes: Vec<u8>,
        scenario_revision: Revision,
        library_revision: Revision,
    },
    BackupBundle {
        bytes: Vec<u8>,
        summary: BackupSummary,
        library_revision: Revision,
    },
    DomainPacks(Vec<DomainPackDescriptor>),
    DomainPack(Box<DomainPackMetadata>),
    Solvers(Vec<SolverDescriptor>),
    Solver(SolverDescriptor),
    SolverSupportMatrix(SolverSupportMatrixMetadata),
    DeferredSolverGates(Vec<DeferredBackendCandidate>),
    /// Opaque capability plus allowlisted metadata; this is not an import or authority claim.
    UnopenedBundlePreview {
        preview_id: RequestId,
        metadata: PreservedBundleMetadata,
    },
}

/// One strongly typed event delivered by an application subscription.
#[derive(Clone, Debug, PartialEq)]
pub struct AppEvent {
    pub topic: EventTopic,
    pub payload: EventPayload,
}

/// Topic-filtered receiver. Lag is reported safely instead of silently losing events.
pub struct EventSubscription {
    topic: EventTopic,
    receiver: broadcast::Receiver<AppEvent>,
}

impl EventSubscription {
    /// Receives the next event for this subscription's topic.
    ///
    /// # Errors
    ///
    /// Returns a protocol error when the receiver lags or the publisher closes.
    pub async fn recv(&mut self) -> Result<AppEvent, AppError> {
        loop {
            match self.receiver.recv().await {
                Ok(event) if event.topic == self.topic => return Ok(event),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    return Err(protocol_error(
                        "event.subscription_lagged",
                        "The event subscriber fell behind; refresh authoritative state.",
                        true,
                    ));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(protocol_error(
                        "event.subscription_closed",
                        "The event subscription is closed.",
                        false,
                    ));
                }
            }
        }
    }

    /// Returns the next already-buffered event for this topic without waiting.
    ///
    /// # Errors
    ///
    /// Returns a protocol error when the receiver lagged or the publisher closed.
    pub fn try_recv(&mut self) -> Result<Option<AppEvent>, AppError> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) if event.topic == self.topic => return Ok(Some(event)),
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Empty) => return Ok(None),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    return Err(protocol_error(
                        "event.subscription_lagged",
                        "The event subscriber fell behind; refresh authoritative state.",
                        true,
                    ));
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    return Err(protocol_error(
                        "event.subscription_closed",
                        "The event subscription is closed.",
                        false,
                    ));
                }
            }
        }
    }
}

#[derive(Clone)]
struct PendingImportPreview {
    inspected: InspectedBundle,
    preview: ImportPreview,
    options: ImportOptions,
    portable_settings: BTreeMap<String, AppSetting<Value>>,
    safety_backup_failure: Option<PendingSafetyBackupFailure>,
    retained_bytes: usize,
}

impl PendingImportPreview {
    fn retained_memory_charge(&self) -> Option<usize> {
        let mut charge = self
            .inspected
            .retained_memory_bytes
            .checked_add(size_of::<Self>())?
            .checked_add(import_preview_records_memory_charge(&self.preview)?)?
            .checked_add(import_preview_metadata_memory_charge(&self.preview)?)?
            .checked_add(pending_tree_memory_charge(
                1,
                size_of::<(RequestId, PendingPortablePreview)>(),
            )?)?;
        charge = charge.checked_add(pending_tree_memory_charge(
            self.portable_settings.len(),
            size_of::<(String, AppSetting<Value>)>(),
        )?)?;
        for (key, setting) in &self.portable_settings {
            charge = charge
                .checked_add(key.capacity())?
                .checked_add(32)?
                .checked_add(eutheto_import::retained_value_memory_charge(
                    &setting.value,
                )?)?;
        }
        let receipt_charge = self
            .safety_backup_failure
            .as_ref()
            .map_or(Some(0), |failure| {
                failure
                    .proof
                    .capacity()
                    .checked_add(failure.collision_plan_sha256.capacity())?
                    .checked_add(64)
            })?;
        // Reserve room before any backup attempt for its canonical UUID proof and
        // SHA-256 digest, including allocation overhead. Reinsertion also checks
        // actual capacities, so retaining a failure cannot bypass the ceiling.
        charge.checked_add(receipt_charge.max(256))
    }
}

fn import_preview_records_memory_charge(preview: &ImportPreview) -> Option<usize> {
    let mut charge = 0_usize;
    for string in [
        &preview.binding.file_sha256,
        &preview.binding.options_sha256,
        &preview.title,
        &preview.created_at,
        &preview.source_application.name,
        &preview.source_application.version,
    ] {
        charge = charge.checked_add(string.capacity())?.checked_add(32)?;
    }
    for (capacity, item_size) in [
        (
            preview.scenarios.capacity(),
            size_of::<eutheto_import::ScenarioPreview>(),
        ),
        (
            preview.removed_scenarios.capacity(),
            size_of::<LocalScenarioSnapshot>(),
        ),
        (
            preview.supplemental_collisions.capacity(),
            size_of::<eutheto_types::SupplementalIdentity>(),
        ),
        (
            preview.removed_supplemental.capacity(),
            size_of::<eutheto_types::SupplementalIdentity>(),
        ),
        (
            preview.applied_migrations.capacity(),
            size_of::<AppliedMigration>(),
        ),
        (preview.settings_changed.capacity(), size_of::<String>()),
        (preview.settings_removed.capacity(), size_of::<String>()),
    ] {
        charge = charge
            .checked_add(capacity.checked_mul(item_size)?)?
            .checked_add(32)?;
    }
    for scenario in &preview.scenarios {
        charge = charge
            .checked_add(scenario.title.capacity())?
            .checked_add(32)?;
        if let Some(warning) = &scenario.same_identity_revision_warning {
            charge = charge.checked_add(warning.capacity())?.checked_add(32)?;
        }
    }
    for scenario in &preview.removed_scenarios {
        charge = charge
            .checked_add(scenario.title.capacity())?
            .checked_add(32)?
            .checked_add(pending_tree_memory_charge(
                scenario.owned_uuids.len(),
                size_of::<Uuid>(),
            )?)?;
    }
    for identity in preview
        .supplemental_collisions
        .iter()
        .chain(&preview.removed_supplemental)
    {
        charge = charge
            .checked_add(identity.key.capacity())?
            .checked_add(32)?;
    }
    for key in preview
        .settings_changed
        .iter()
        .chain(&preview.settings_removed)
    {
        charge = charge.checked_add(key.capacity())?.checked_add(32)?;
    }
    for migration in &preview.applied_migrations {
        charge = charge
            .checked_add(migration.name.capacity())?
            .checked_add(32)?;
        if let Some(subject) = &migration.subject {
            // PackId owns an exact-length copy made by its validated constructor.
            charge = charge
                .checked_add(subject.pack_id.as_str().len())?
                .checked_add(32)?;
        }
    }
    Some(charge)
}

fn import_preview_metadata_memory_charge(preview: &ImportPreview) -> Option<usize> {
    let mut charge = 0_usize;
    charge = charge.checked_add(pending_tree_memory_charge(
        preview.required_capabilities.len(),
        size_of::<eutheto_types::SemanticCapability>(),
    )?)?;
    for capability in &preview.required_capabilities {
        charge = charge
            .checked_add(capability.id.capacity())?
            .checked_add(32)?;
    }
    for strings in [
        &preview.preserved_extensions,
        &preview.included_sections,
        &preview.excluded_sections,
    ] {
        charge = charge.checked_add(pending_tree_memory_charge(
            strings.len(),
            size_of::<String>(),
        )?)?;
        for string in strings {
            charge = charge.checked_add(string.capacity())?.checked_add(32)?;
        }
    }
    if let Some(selection) = &preview.source_backup_selection {
        charge = charge
            .checked_add(pending_tree_memory_charge(
                selection.fixed_exclusions.len(),
                size_of::<FixedExclusion>(),
            )?)?
            .checked_add(pending_tree_memory_charge(
                selection.excluded_asset_ids.len(),
                size_of::<String>(),
            )?)?;
        for key in &selection.excluded_asset_ids {
            charge = charge.checked_add(key.capacity())?.checked_add(32)?;
        }
    }
    charge = charge.checked_add(pending_tree_memory_charge(
        preview.omitted_assets.len(),
        size_of::<(String, eutheto_export::OmittedAssetPlaceholder)>(),
    )?)?;
    for (key, asset) in &preview.omitted_assets {
        for string in [
            key,
            &asset.format,
            &asset.original_media_type,
            &asset.content_sha256,
        ] {
            charge = charge.checked_add(string.capacity())?.checked_add(32)?;
        }
    }
    Some(charge)
}

fn pending_tree_memory_charge(items: usize, item_size: usize) -> Option<usize> {
    // Charge a complete 11-slot BTree node per item, even for sparse roots,
    // plus links and allocator overhead. No temporary serialization is needed.
    items.checked_mul(item_size.checked_mul(11)?.checked_add(256)?)
}

#[derive(Clone)]
enum PendingPortablePreview {
    Import(Box<PendingImportPreview>),
    Unopened(UnopenedBundle),
    Csv(PendingCsvPreview),
    Settings(PendingSettingsPreview),
}

impl PendingPortablePreview {
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Import(preview) => preview.retained_bytes,
            Self::Unopened(bundle) => bundle.retained_memory_bytes().saturating_add(
                pending_tree_memory_charge(1, size_of::<(RequestId, Self)>()).unwrap_or(usize::MAX),
            ),
            Self::Csv(preview) => preview.retained_bytes(),
            Self::Settings(preview) => preview.retained_bytes(),
        }
    }
}

#[derive(Clone)]
struct PendingSafetyBackupFailure {
    proof: String,
    collision_plan_sha256: String,
}
struct StagedPortableApply {
    import: eutheto_import::StagedImport,
    scenarios: Vec<AppliedPortableScenario>,
    events: Vec<(ScenarioId, Revision, ChangeKind)>,
    remove_scenario_ids: BTreeSet<ScenarioId>,
}
struct CommittedPortableApply {
    scenarios: Vec<AppliedPortableScenario>,
    events: Vec<(ScenarioId, Revision, ChangeKind)>,
    library_changed: bool,
    library_revision: Revision,
    safety_backup: SafetyBackupOutcome,
}

#[derive(Clone, Copy)]
enum ProjectLifecycleAction {
    Archive,
    Unarchive,
    Delete,
}

const ACCEPTED_VERIFICATION_CACHE_CAPACITY: usize = 4;
const ACCEPTED_VERIFICATION_MAX_IN_FLIGHT: usize = 4;
const ACCEPTED_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(30);
type AcceptedContentHash = [u8; 32];

// An accepted read owns its source snapshot and query slot until the store actor finishes,
// even if the requesting window times out or detaches.
#[allow(clippy::large_enum_variant)]
enum AcceptedQuerySource {
    Terminal(StoredAcceptedResultV2),
    Checkpoint(ScenarioDocument, VerifiedCheckpoint),
}

impl AcceptedQuerySource {
    fn run_id(&self) -> SolveRunId {
        match self {
            Self::Terminal(stored) => stored.portable.run_input.run_id,
            Self::Checkpoint(_, checkpoint) => checkpoint.run_input.run_id,
        }
    }
}

// This is only a bounded change detector for freshly loaded tuples, never a signature.
struct AcceptedVerificationCache {
    completed: Mutex<VecDeque<AcceptedContentHash>>,
    slots: Arc<Semaphore>,
}

impl AcceptedVerificationCache {
    fn new() -> Self {
        Self {
            completed: Mutex::new(VecDeque::with_capacity(
                ACCEPTED_VERIFICATION_CACHE_CAPACITY,
            )),
            slots: Arc::new(Semaphore::new(ACCEPTED_VERIFICATION_MAX_IN_FLIGHT)),
        }
    }
}

#[derive(Clone)]
struct AcceptedVerificationState {
    cache: Arc<AcceptedVerificationCache>,
}

impl AcceptedVerificationState {
    fn new() -> Self {
        Self {
            cache: Arc::new(AcceptedVerificationCache::new()),
        }
    }
}

struct CapturedOptimalityStatus {
    document: ScenarioDocument,
    run_input: RunInput,
    run_manifest: eutheto_domain_ir::RunManifest,
    stored: Option<Box<StoredAcceptedResultV2>>,
    result: Option<AcceptedResultRefV1>,
}
enum CapturedExplanation {
    Validation {
        document: Box<ScenarioDocument>,
        revision: Revision,
        issue_id: eutheto_domain_ir::VerificationIssueId,
    },
    Assignment {
        stored: Box<StoredAcceptedResultV2>,
        assignment_id: eutheto_domain_ir::DomainAssignmentId,
    },
    Counterfactual {
        stored: Box<StoredAcceptedResultV2>,
        result: Box<eutheto_domain_ir::CounterfactualResult>,
        alternative: Option<Box<StoredAcceptedResultV2>>,
    },
    SolutionDifference {
        base: Box<StoredAcceptedResultV2>,
        candidate: Box<StoredAcceptedResultV2>,
    },
    Repair {
        current: Box<StoredAcceptedResultV2>,
        base: Box<StoredAcceptedResultV2>,
    },
    OptimalityStatus(Box<CapturedOptimalityStatus>),
    Infeasibility(Box<eutheto_store::StoredInfeasibilitySourceV1>),
}

const MAX_ORDINARY_SOLVE_CUSTODY: usize = 16;
type OrdinarySolveCustody = Arc<StdMutex<BTreeMap<SolveRunId, Arc<StdMutex<OrdinarySolveState>>>>>;

struct OrdinaryRunBinding {
    run_id: SolveRunId,
    request_id: RequestId,
    scenario_id: ScenarioId,
    revision: Revision,
    input_checksum: String,
    started_at: Rfc3339Timestamp,
}

struct OrdinaryFirstAccepted {
    solution_id: SolutionId,
    checksum: String,
    accepted_at: Rfc3339Timestamp,
}

// These are independent producer/checkpoint/confirmation facts, not one exclusive state.
#[allow(clippy::struct_excessive_bools)]
struct OrdinarySolveState {
    pending_start: Option<NewSolveRunV1>,
    binding: Option<OrdinaryRunBinding>,
    first_accepted: Option<OrdinaryFirstAccepted>,
    alarm: Option<eutheto_verify::CorrectnessAlarm>,
    secondary_cause: Option<eutheto_types::KnownVerificationSecondaryCauseCode>,
    confirmed: bool,
    producer_done: bool,
    checkpoint_claimed: bool,
    checkpoint_done: bool,
    observations: OrdinaryObservationBounds,
    native_observations: Option<Arc<eutheto_domain_ir::RunNativeObservations>>,
}

#[derive(Clone, Copy)]
pub(crate) struct OrdinaryObservationBounds {
    pub(crate) latest_wall: Option<Rfc3339Timestamp>,
    pub(crate) latest_elapsed: Option<Duration>,
    pub(crate) unreliable: bool,
}

fn lock_ordinary_state(
    state: &StdMutex<OrdinarySolveState>,
) -> std::sync::MutexGuard<'_, OrdinarySolveState> {
    state.lock().unwrap_or_else(|poisoned| {
        // Closed binding/scalar updates survive unwinding; timing no longer does.
        let mut state = poisoned.into_inner();
        state.observations.unreliable = true;
        state
    })
}

impl OrdinarySolveState {
    fn observe_wall(&mut self, timestamp: Rfc3339Timestamp) {
        if self
            .observations
            .latest_wall
            .is_some_and(|previous| timestamp < previous)
        {
            self.observations.unreliable = true;
        } else {
            self.observations.latest_wall = Some(timestamp);
        }
    }

    fn observe_elapsed(&mut self, elapsed: Duration) {
        let representable = u64::try_from(elapsed.as_millis())
            .ok()
            .and_then(|value| eutheto_types::DurationMillis::new(value).ok())
            .is_some();
        if !representable
            || self
                .observations
                .latest_elapsed
                .is_some_and(|previous| elapsed < previous)
        {
            self.observations.unreliable = true;
        } else {
            self.observations.latest_elapsed = Some(elapsed);
        }
    }

    fn validate_input(&self, input: &RunInput) -> Result<(), AppError> {
        if self.binding.as_ref().is_some_and(|binding| {
            binding.run_id != input.run_id
                || binding.request_id != input.request_id
                || binding.scenario_id != input.scenario_id
                || binding.revision != Revision::new(input.scenario_revision)
                || binding.input_checksum != input.checksum
        }) {
            return Err(solution_contract_error());
        }
        Ok(())
    }

    fn admit(&self) -> Result<(), AppError> {
        let Some(alarm) = &self.alarm else {
            return Ok(());
        };
        let binding = self.binding.as_ref().ok_or_else(solution_contract_error)?;
        let known = KnownVerificationAlarmV1::new(
            binding.run_id,
            &binding.input_checksum,
            &alarm.diagnostic_code,
            if self.confirmed {
                VerificationAlarmTerminalization::Confirmed
            } else {
                VerificationAlarmTerminalization::Unconfirmed
            },
            self.secondary_cause
                .map(eutheto_types::KnownVerificationSecondaryCauseCode::as_str),
        )
        .map_err(|_| solution_contract_error())?;
        Err(AppError::Verification(
            VerificationFailure::from_known_alarm(known, None),
        ))
    }
}

/// One native producer owns admission; publication handles never own this lease.
pub(crate) struct OrdinarySolveLease {
    custody: OrdinarySolveCustody,
    run_id: SolveRunId,
    state: Arc<StdMutex<OrdinarySolveState>>,
}

/// Independent checkpoint completion is required even when its producer is dropped.
pub(crate) struct OrdinaryCheckpointWork {
    custody: OrdinarySolveCustody,
    run_id: SolveRunId,
    state: Arc<StdMutex<OrdinarySolveState>>,
}

#[derive(Clone)]
struct OrdinaryPublicationHandle(Arc<StdMutex<OrdinarySolveState>>);

fn matches_ordinary_start(
    pending: &NewSolveRunV1,
    input: &RunInput,
    started_at: Rfc3339Timestamp,
) -> bool {
    pending.run_id == input.run_id
        && pending.request_id == input.request_id
        && pending.scenario_id == input.scenario_id
        && pending.expected_revision == Revision::new(input.scenario_revision)
        && pending.started_at == started_at
        && pending.planning_ir_schema_version == input.planning_ir_schema_version
        && pending.compiler_version == input.compiler_version
        && pending.application_version == input.application_version
        && pending.backend_id == input.backend_id
        && pending.backend_version == input.backend_version
        && pending.adapter_version == input.adapter_version
        && pending.worker_version == input.worker_version
        && pending.solver_version == input.solver_version
        && pending.protocol_major == input.protocol_major
        && pending.protocol_minor == input.protocol_minor
        && pending.model_hash == input.model_hash
        && pending.objective_policy_hash == input.objective_policy_hash
        && pending.solve_options == input.solve_options
        && pending.temporary_condition_hash == input.temporary_condition_hash
        && pending.objective_context == input.objective_context
}

impl OrdinarySolveLease {
    pub(crate) fn bind_started(
        &self,
        input: &RunInput,
        started_at: Rfc3339Timestamp,
    ) -> Result<(), AppError> {
        let mut state = lock_ordinary_state(&self.state);
        let pending = state
            .pending_start
            .as_ref()
            .ok_or_else(solution_contract_error)?;
        if !matches_ordinary_start(pending, input, started_at) {
            return Err(solution_contract_error());
        }
        state.binding = Some(OrdinaryRunBinding {
            run_id: input.run_id,
            request_id: input.request_id,
            scenario_id: input.scenario_id,
            revision: Revision::new(input.scenario_revision),
            input_checksum: input.checksum.clone(),
            started_at,
        });
        state.pending_start = None;
        state.observe_wall(started_at);
        Ok(())
    }

    pub(crate) fn retain_native_observations(
        &self,
        input: &RunInput,
        observations: Arc<eutheto_domain_ir::RunNativeObservations>,
    ) -> Result<(), AppError> {
        let mut state = lock_ordinary_state(&self.state);
        if state.binding.as_ref().is_none_or(|binding| {
            binding.run_id != input.run_id || binding.input_checksum != input.checksum
        }) || state.native_observations.is_some()
        {
            return Err(solution_contract_error());
        }
        state.native_observations = Some(observations);
        Ok(())
    }

    pub(crate) fn native_observations(
        &self,
    ) -> Option<Arc<eutheto_domain_ir::RunNativeObservations>> {
        lock_ordinary_state(&self.state).native_observations.clone()
    }

    /// Only a native no-start/reused result may settle a provisional loser.
    pub(crate) fn release_unowned_start(&self) -> Result<(), AppError> {
        let mut state = lock_ordinary_state(&self.state);
        if state.binding.is_some() {
            return Err(solution_contract_error());
        }
        state.pending_start = None;
        Ok(())
    }

    pub(crate) fn capture_alarm(&self, alarm: &eutheto_verify::CorrectnessAlarm) {
        if alarm.category == eutheto_verify::CorrectnessAlarmCategory::ClockFailed {
            return;
        }
        lock_ordinary_state(&self.state)
            .alarm
            .get_or_insert_with(|| alarm.clone());
    }

    pub(crate) fn observe_wall(&self, timestamp: Rfc3339Timestamp) {
        lock_ordinary_state(&self.state).observe_wall(timestamp);
    }

    pub(crate) fn observe_elapsed(&self, elapsed: Duration) {
        lock_ordinary_state(&self.state).observe_elapsed(elapsed);
    }

    pub(crate) fn observation_bounds(&self) -> Result<OrdinaryObservationBounds, AppError> {
        let state = lock_ordinary_state(&self.state);
        if state.binding.is_none() {
            return Err(solution_contract_error());
        }
        Ok(state.observations)
    }

    pub(crate) fn retain_secondary_cause(
        &self,
        cause: Option<eutheto_types::KnownVerificationSecondaryCauseCode>,
    ) {
        let mut state = lock_ordinary_state(&self.state);
        if state.secondary_cause.is_none() {
            state.secondary_cause = cause;
        }
    }

    pub(crate) fn record_first_accepted(
        &self,
        solution_id: SolutionId,
        checksum: &str,
        accepted_at: Rfc3339Timestamp,
    ) -> Result<(), AppError> {
        let mut state = lock_ordinary_state(&self.state);
        state.admit()?;
        if state.binding.is_none() || state.first_accepted.is_some() {
            return Err(solution_contract_error());
        }
        state.observe_wall(accepted_at);
        state.first_accepted = Some(OrdinaryFirstAccepted {
            solution_id,
            checksum: checksum.to_owned(),
            accepted_at,
        });
        Ok(())
    }

    pub(crate) fn checkpoint_work(&self) -> Result<OrdinaryCheckpointWork, AppError> {
        let mut state = lock_ordinary_state(&self.state);
        if state.binding.is_none() || state.checkpoint_claimed {
            return Err(solution_contract_error());
        }
        state.checkpoint_claimed = true;
        state.checkpoint_done = false;
        Ok(OrdinaryCheckpointWork {
            custody: Arc::clone(&self.custody),
            run_id: self.run_id,
            state: Arc::clone(&self.state),
        })
    }
}

fn retire_ordinary_solve(
    custody: &OrdinarySolveCustody,
    run_id: SolveRunId,
    state: &Arc<StdMutex<OrdinarySolveState>>,
) {
    let Ok(mut entries) = custody.lock() else {
        return;
    };
    let current = lock_ordinary_state(state);
    if current.producer_done
        && current.checkpoint_done
        && current.pending_start.is_none()
        && (current.alarm.is_none() || current.confirmed)
        && entries
            .get(&run_id)
            .is_some_and(|entry| Arc::ptr_eq(entry, state))
    {
        entries.remove(&run_id);
    }
}

impl Drop for OrdinarySolveLease {
    fn drop(&mut self) {
        lock_ordinary_state(&self.state).producer_done = true;
        retire_ordinary_solve(&self.custody, self.run_id, &self.state);
    }
}

impl Drop for OrdinaryCheckpointWork {
    fn drop(&mut self) {
        lock_ordinary_state(&self.state).checkpoint_done = true;
        retire_ordinary_solve(&self.custody, self.run_id, &self.state);
    }
}

impl OrdinaryPublicationHandle {
    fn recipient_retained_bytes(&self, control: &OperationControl) -> Result<usize, AppError> {
        let mut bytes = recipient_memory::SourceBytes::new(control)?;
        let state = lock_ordinary_state(&self.0);
        let OrdinarySolveState {
            pending_start,
            binding,
            first_accepted,
            alarm,
            secondary_cause: _,
            confirmed: _,
            producer_done: _,
            checkpoint_claimed: _,
            checkpoint_done: _,
            observations: _,
            native_observations,
        } = &*state;
        // Recipient custody binds only a started source; native observations can still be
        // pending for an early verified checkpoint. No request-sized pending start may remain.
        if pending_start.is_some() {
            return Err(solution_contract_error());
        }
        let binding = binding.as_ref().ok_or_else(solution_contract_error)?;
        bytes.add(size_of::<Self>())?;
        bytes.arc::<StdMutex<OrdinarySolveState>>()?;
        let OrdinaryRunBinding {
            run_id: _,
            request_id: _,
            scenario_id: _,
            revision: _,
            input_checksum,
            started_at: _,
        } = binding;
        bytes.strings([input_checksum])?;
        // A verified checkpoint can precede the first terminal acceptance; its later checksum
        // is copied from a canonical 64-hex string. All other late fields are inline scalars.
        if let Some(accepted) = first_accepted {
            let OrdinaryFirstAccepted {
                solution_id: _,
                checksum,
                accepted_at: _,
            } = accepted;
            bytes.strings([checksum])?;
        } else {
            bytes.add(64)?;
        }
        if let Some(alarm) = alarm {
            let eutheto_verify::CorrectnessAlarm {
                category: _,
                diagnostic_code,
            } = alarm;
            bytes.strings([diagnostic_code])?;
        } else {
            // capture_alarm clones a closed native verifier code, never a raw diagnostic.
            bytes.add(eutheto_domain_ir::MAX_RUN_DIAGNOSTIC_CODE_BYTES)?;
        }
        if let Some(observations) = native_observations {
            bytes.observations(observations)?;
        } else {
            bytes.pending_observations()?;
        }
        Ok(bytes.finish())
    }

    fn admit(&self) -> Result<(), AppError> {
        lock_ordinary_state(&self.0).admit()
    }

    fn validate_diagnostic_scope(
        &self,
        scenario_id: ScenarioId,
        source_revision: Revision,
        run_id: SolveRunId,
    ) -> Result<(), AppError> {
        let state = lock_ordinary_state(&self.0);
        let (native_scenario, native_revision, native_run) = if let Some(binding) = &state.binding {
            (binding.scenario_id, binding.revision, binding.run_id)
        } else if let Some(start) = &state.pending_start {
            (start.scenario_id, start.expected_revision, start.run_id)
        } else {
            return Err(solution_contract_error());
        };
        if native_scenario != scenario_id || native_run != run_id {
            return Err(solution_reference_error("/request"));
        }
        if native_revision != source_revision {
            return Err(solution_reference_error("/sourceRevision"));
        }
        Ok(())
    }

    fn validate_input(&self, input: &RunInput) -> Result<(), AppError> {
        lock_ordinary_state(&self.0).validate_input(input)
    }

    fn validate_summary(&self, summary: &StoredAcceptedResultSummary) -> Result<(), AppError> {
        let state = lock_ordinary_state(&self.0);
        if let Some(binding) = &state.binding
            && (binding.run_id != summary.run_id
                || binding.scenario_id != summary.scenario_id
                || binding.revision != summary.scenario_revision
                || binding.input_checksum != summary.run_input_checksum)
        {
            return Err(solution_contract_error());
        }
        Ok(())
    }

    fn admit_input(&self, input: &RunInput) -> Result<(), AppError> {
        let state = lock_ordinary_state(&self.0);
        state.validate_input(input)?;
        state.admit()
    }
}

/// Reusable application service with no Tauri or CLI dependency.
#[derive(Clone)]
pub struct EuthetoApp {
    store: Arc<SqliteScenarioStore>,
    initialization: InitializationOutcome,
    pack_registry: Arc<DomainPackRegistry>,
    solver_registry: Arc<SolverRegistry>,
    paths: AppPaths,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    monotonic_clock: Arc<dyn MonotonicClock>,
    cancellation: CancellationToken,
    accepted_verification: AcceptedVerificationState,
    /// Bounds complete accepted tuples and projections outside scenario mutation locks.
    accepted_query_slots: Arc<Semaphore>,
    mutation_locks: ScenarioLocks,
    validation_readiness: Arc<ValidationReadiness>,
    previews: Arc<Mutex<BTreeMap<RequestId, PendingPortablePreview>>>,
    events: broadcast::Sender<AppEvent>,
    counterfactual: CounterfactualRuntime,
    ordinary_solve_custody: OrdinarySolveCustody,
}

impl EuthetoApp {
    /// Opens the injected database path, runs startup initialization, and constructs services.
    ///
    /// # Errors
    ///
    /// Returns a typed storage or protocol failure when directories, startup,
    /// migration, integrity recovery, or the database actor cannot initialize.
    pub async fn open(dependencies: AppDependencies) -> Result<Self, AppError> {
        let (pack_registry, solver_registry) = validated_static_registries()?;
        let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database)
            .await
            .map_err(store_error)?;
        Ok(Self::from_initialized_store_with_registries(
            Arc::new(store),
            initialization,
            dependencies,
            pack_registry,
            solver_registry,
        ))
    }

    /// Opens the application with a complete host-supplied static solver registry.
    ///
    /// Platform adapters use this boundary to add manifest-verified bundled backends while the
    /// default [`Self::open`] path retains the generated metadata-only registry.
    ///
    /// # Errors
    ///
    /// Returns a typed storage or protocol failure under the same conditions as [`Self::open`].
    pub async fn open_with_solver_registry(
        dependencies: AppDependencies,
        solver_registry: SolverRegistry,
    ) -> Result<Self, AppError> {
        let (pack_registry, _) = validated_static_registries()?;
        let (store, initialization) = SqliteScenarioStore::open(&dependencies.paths.database)
            .await
            .map_err(store_error)?;
        Ok(Self::from_initialized_store_with_registries(
            Arc::new(store),
            initialization,
            dependencies,
            pack_registry,
            Arc::new(solver_registry),
        ))
    }

    /// Constructs the service from an explicitly initialized store.
    ///
    /// # Errors
    ///
    /// Returns a safe protocol failure if compiled pack or solver metadata has drifted.
    pub fn from_initialized_store(
        store: Arc<SqliteScenarioStore>,
        initialization: InitializationOutcome,
        dependencies: AppDependencies,
    ) -> Result<Self, AppError> {
        let (pack_registry, solver_registry) = validated_static_registries()?;
        Ok(Self::from_initialized_store_with_registries(
            store,
            initialization,
            dependencies,
            pack_registry,
            solver_registry,
        ))
    }

    /// Constructs the service with an explicitly supplied compiled-in pack registry.
    ///
    /// This is the same initialization boundary as [`Self::from_initialized_store`], but lets
    /// embedders supply their complete static pack set.
    ///
    /// # Errors
    ///
    /// Returns a safe protocol failure if compiled solver metadata has drifted.
    pub fn from_initialized_store_with_pack_registry(
        store: Arc<SqliteScenarioStore>,
        initialization: InitializationOutcome,
        dependencies: AppDependencies,
        pack_registry: DomainPackRegistry,
    ) -> Result<Self, AppError> {
        let (_, solver_registry) = validated_static_registries()?;
        Ok(Self::from_initialized_store_with_registries(
            store,
            initialization,
            dependencies,
            Arc::new(pack_registry),
            solver_registry,
        ))
    }

    /// Constructs the service with explicit validated pack and solver registries.
    ///
    /// This initialization boundary is intended for embedders that supply a complete static
    /// backend set rather than the default compiled registry.
    #[must_use]
    pub fn from_initialized_store_with_registries(
        store: Arc<SqliteScenarioStore>,
        initialization: InitializationOutcome,
        dependencies: AppDependencies,
        pack_registry: Arc<DomainPackRegistry>,
        solver_registry: Arc<SolverRegistry>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        let counterfactual = CounterfactualRuntime::new(
            Arc::clone(&store),
            Arc::clone(&pack_registry),
            Arc::clone(&solver_registry),
            CounterfactualRuntimeServices {
                clock: Arc::clone(&dependencies.clock),
                ids: Arc::clone(&dependencies.ids),
                cancellation: dependencies.cancellation.clone(),
                events: events.clone(),
            },
        );
        Self {
            store,
            initialization,
            pack_registry,
            solver_registry,
            paths: dependencies.paths,
            clock: dependencies.clock,
            monotonic_clock: dependencies.monotonic_clock,
            ids: dependencies.ids,
            cancellation: dependencies.cancellation,
            accepted_verification: AcceptedVerificationState::new(),
            accepted_query_slots: Arc::new(Semaphore::new(ACCEPTED_VERIFICATION_MAX_IN_FLIGHT)),
            mutation_locks: Arc::new(Mutex::new(BTreeMap::new())),
            validation_readiness: Arc::default(),
            previews: Arc::new(Mutex::new(BTreeMap::new())),
            events,
            counterfactual,
            ordinary_solve_custody: Arc::default(),
        }
    }

    /// Returns the exact initialization outcome produced while opening the store.
    ///
    /// Startup recovery is complete before the application is constructed; reading this
    /// outcome reports that work without replaying it.
    #[must_use]
    pub fn initialization(&self) -> &InitializationOutcome {
        &self.initialization
    }

    /// Creates a topic-filtered typed event subscription.
    ///
    /// # Errors
    ///
    /// Returns a typed application error when subscription setup is unavailable.
    pub async fn subscribe(&self, topic: EventTopic) -> Result<EventSubscription, AppError> {
        std::future::ready(()).await;
        Ok(EventSubscription {
            topic,
            receiver: self.events.subscribe(),
        })
    }

    /// Whether this process has an actual registered executable solver runtime.
    ///
    /// Generated support metadata alone is not an execution capability.
    #[must_use]
    pub fn solve_runtime_available(&self) -> bool {
        !self.solver_registry.is_empty()
    }

    /// Returns descriptors for executable backends only.
    #[must_use]
    pub fn solve_backend_descriptors(&self) -> Vec<SolverDescriptor> {
        self.solver_registry.descriptors().cloned().collect()
    }

    /// Reads one scenario-bound retained solve run without starting work.
    ///
    /// # Errors
    ///
    /// Returns cancellation or a store read/recovery error; a run belonging
    /// to another scenario is absent rather than disclosed.
    pub async fn solve_get_job(
        &self,
        scenario_id: ScenarioId,
        run_id: SolveRunId,
    ) -> Result<Option<ExistingSolveRunV1>, AppError> {
        let ordinary = self.ordinary_run_handle(scenario_id, run_id)?;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        self.check_cancelled()?;
        let retained = self.store.load_solve_run(scenario_id, run_id).await;
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        if let Some(ordinary) = &ordinary {
            if let Ok(Some(run)) = &retained {
                ordinary.admit_input(&run.input)?;
            } else {
                ordinary.admit()?;
            }
        }
        if let Ok(Some(run)) = &retained
            && let Some(manifest) = &run.manifest
        {
            crate::native_observations::validate_fingerprint(&run.input, manifest)
                .map_err(|_| solution_contract_error())?;
        }
        retained.map_err(store_error)
    }

    /// Reads one durable first-verified candidate only after the parent run is bound to
    /// the requested scenario. A non-alarm terminal failure does not revoke that checkpoint.
    ///
    /// # Errors
    ///
    /// Returns cancellation or invalid persisted authority without disclosing another
    /// scenario's candidate.
    pub async fn solve_get_verified_checkpoint(
        &self,
        scenario_id: ScenarioId,
        run_id: SolveRunId,
    ) -> Result<Option<VerifiedCheckpoint>, AppError> {
        let ordinary = self.ordinary_run_handle(scenario_id, run_id)?;
        let Some(run) = self.solve_get_job(scenario_id, run_id).await? else {
            return Ok(None);
        };
        let checkpoint = self.store.load_verified_checkpoint(run_id).await;
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        let checkpoint = checkpoint.map_err(store_error)?;
        if checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.run_input != run.input)
        {
            return Err(solution_contract_error());
        }
        if let Some(ordinary) = ordinary {
            ordinary.admit_input(&run.input)?;
        }
        Ok(checkpoint)
    }

    /// Reads a bounded scenario/revision run list without taking execution
    /// ownership of any running row.
    ///
    /// # Errors
    ///
    /// Returns cancellation, an invalid list limit, or a store read/recovery
    /// error.
    pub async fn solve_list_runs(
        &self,
        request: SolveRunListRequestV1,
    ) -> Result<Vec<ExistingSolveRunV1>, AppError> {
        self.check_cancelled()?;
        let scenario_id = request.scenario_id;
        let mut ordinary = self.ordinary_scenario_handles(scenario_id)?;
        let mut runs = self
            .store
            .list_solve_runs(request)
            .await
            .map_err(store_error)?;
        for run in &mut runs {
            if ordinary
                .iter()
                .any(|(run_id, _)| *run_id == run.input.run_id)
            {
                continue;
            }
            if let Some(handle) = self.ordinary_run_handle(scenario_id, run.input.run_id)? {
                ordinary.push((run.input.run_id, handle));
            } else if run.manifest.is_none() {
                // A newly listed owner may already have drained and retired after exact
                // quarantine. Absence is not permission to publish its older Running row.
                let fresh = self
                    .store
                    .load_solve_run(scenario_id, run.input.run_id)
                    .await
                    .map_err(store_error)?
                    .ok_or_else(solution_contract_error)?;
                if fresh.input != run.input || fresh.started_at != run.started_at {
                    return Err(solution_contract_error());
                }
                *run = fresh;
                if let Some(handle) = self.ordinary_run_handle(scenario_id, run.input.run_id)? {
                    ordinary.push((run.input.run_id, handle));
                }
            }
        }
        for run in &runs {
            self.counterfactual
                .guard_run_publication(scenario_id, run.input.run_id)
                .await?;
        }
        for run in &runs {
            if let Some((_, handle)) = ordinary
                .iter()
                .find(|(run_id, _)| *run_id == run.input.run_id)
            {
                handle.validate_input(&run.input)?;
            }
        }
        for run in &runs {
            if let Some((_, handle)) = ordinary
                .iter()
                .find(|(run_id, _)| *run_id == run.input.run_id)
            {
                handle.admit()?;
            }
        }
        for run in &runs {
            if let Some(manifest) = &run.manifest {
                crate::native_observations::validate_fingerprint(&run.input, manifest)
                    .map_err(|_| solution_contract_error())?;
            }
        }
        Ok(runs)
    }

    /// Publishes advisory progress after the caller has already bound the
    /// immutable scenario/revision/run identity.
    pub fn publish_solve_progress(
        &self,
        request_id: RequestId,
        scenario_id: ScenarioId,
        revision: Revision,
        run_id: SolveRunId,
        phase: SolvePhase,
        percent: Option<u8>,
    ) {
        let _ = self.events.send(AppEvent {
            topic: EventTopic::SolveProgress,
            payload: EventPayload::SolveProgress {
                context: EventContext {
                    event_version: EVENT_VERSION,
                    timestamp: self.clock.now(),
                    request_id: Some(request_id),
                    scenario_id: Some(scenario_id),
                    revision: Some(revision),
                    solve_run_id: Some(run_id),
                },
                phase,
                percent,
            },
        });
    }

    fn ordinary_scenario_handles(
        &self,
        scenario_id: ScenarioId,
    ) -> Result<Vec<(SolveRunId, OrdinaryPublicationHandle)>, AppError> {
        let entries = self
            .ordinary_solve_custody
            .lock()
            .map_err(|_| solution_contract_error())?;
        let mut handles = Vec::new();
        for (run_id, entry) in entries.iter() {
            let state = lock_ordinary_state(entry);
            let owner = state
                .binding
                .as_ref()
                .map(|binding| binding.scenario_id)
                .or_else(|| {
                    state
                        .pending_start
                        .as_ref()
                        .map(|pending| pending.scenario_id)
                });
            if owner == Some(scenario_id) {
                handles.push((*run_id, OrdinaryPublicationHandle(Arc::clone(entry))));
            }
        }
        Ok(handles)
    }

    fn ordinary_request_handle(
        &self,
        scenario_id: ScenarioId,
        request_id: RequestId,
    ) -> Result<Option<OrdinaryPublicationHandle>, AppError> {
        let entries = self
            .ordinary_solve_custody
            .lock()
            .map_err(|_| solution_contract_error())?;
        let mut pending = None;
        for entry in entries.values() {
            let state = lock_ordinary_state(entry);
            if state.binding.as_ref().is_some_and(|binding| {
                binding.scenario_id == scenario_id && binding.request_id == request_id
            }) {
                return Ok(Some(OrdinaryPublicationHandle(Arc::clone(entry))));
            }
            if pending.is_none()
                && state.pending_start.as_ref().is_some_and(|start| {
                    start.scenario_id == scenario_id && start.request_id == request_id
                })
            {
                pending = Some(OrdinaryPublicationHandle(Arc::clone(entry)));
            }
        }
        Ok(pending)
    }

    fn ordinary_run_handle(
        &self,
        scenario_id: ScenarioId,
        run_id: SolveRunId,
    ) -> Result<Option<OrdinaryPublicationHandle>, AppError> {
        let entries = self
            .ordinary_solve_custody
            .lock()
            .map_err(|_| solution_contract_error())?;
        let Some(entry) = entries.get(&run_id) else {
            return Ok(None);
        };
        let state = lock_ordinary_state(entry);
        let owner = state
            .binding
            .as_ref()
            .map(|binding| binding.scenario_id)
            .or_else(|| {
                state
                    .pending_start
                    .as_ref()
                    .map(|pending| pending.scenario_id)
            });
        Ok((owner == Some(scenario_id)).then(|| OrdinaryPublicationHandle(Arc::clone(entry))))
    }

    fn ordinary_reference_handle(
        &self,
        scenario_id: ScenarioId,
        reference: &AcceptedResultRefV1,
    ) -> Result<Option<OrdinaryPublicationHandle>, AppError> {
        let Some(handle) = self.ordinary_solution_handle(scenario_id, reference.solution_id)?
        else {
            return Ok(None);
        };
        let matches = lock_ordinary_state(&handle.0)
            .first_accepted
            .as_ref()
            .is_some_and(|first| first.checksum == reference.result_checksum);
        Ok(matches.then_some(handle))
    }

    fn ordinary_explanation_handles(
        &self,
        request: &SolutionExplainRequestV1,
    ) -> Result<[Option<OrdinaryPublicationHandle>; 2], AppError> {
        let (first, second) = match &request.request.subject {
            ExplanationRequestSubjectV1::Assignment { result, .. } => (result, None),
            ExplanationRequestSubjectV1::Counterfactual { base, .. } => (base, None),
            ExplanationRequestSubjectV1::SolutionDifference { left, right } => (left, Some(right)),
            ExplanationRequestSubjectV1::Repair { current, base } => (current, Some(base)),
            ExplanationRequestSubjectV1::Infeasibility { solve_run_id, .. }
            | ExplanationRequestSubjectV1::OptimalityStatus { solve_run_id, .. } => {
                return Ok([
                    self.ordinary_run_handle(request.scenario_id, *solve_run_id)?,
                    None,
                ]);
            }
            ExplanationRequestSubjectV1::Validation { .. } => return Ok([None, None]),
        };
        Ok([
            self.ordinary_reference_handle(request.scenario_id, first)?,
            second
                .map(|reference| self.ordinary_reference_handle(request.scenario_id, reference))
                .transpose()?
                .flatten(),
        ])
    }

    fn ordinary_explanation_scope_admitted(
        request: &SolutionExplainRequestV1,
        source_revision: Revision,
        handles: &[Option<OrdinaryPublicationHandle>; 2],
    ) -> Result<bool, AppError> {
        let anchor = match &request.request.subject {
            ExplanationRequestSubjectV1::Assignment { .. }
            | ExplanationRequestSubjectV1::Counterfactual { .. }
            | ExplanationRequestSubjectV1::Repair { .. } => 0,
            ExplanationRequestSubjectV1::SolutionDifference { .. } => 1,
            _ => return Ok(false),
        };
        let Some(handle) = handles[anchor].as_ref() else {
            return Ok(false);
        };
        let state = lock_ordinary_state(&handle.0);
        let Some(binding) = state.binding.as_ref() else {
            return Ok(false);
        };
        if binding.revision != source_revision {
            return Err(solution_reference_error("/sourceRevision"));
        }
        Ok(true)
    }

    fn ordinary_solution_handle(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
    ) -> Result<Option<OrdinaryPublicationHandle>, AppError> {
        let entries = self
            .ordinary_solve_custody
            .lock()
            .map_err(|_| solution_contract_error())?;
        for entry in entries.values() {
            let state = lock_ordinary_state(entry);
            if state
                .binding
                .as_ref()
                .is_some_and(|binding| binding.scenario_id == scenario_id)
                && state
                    .first_accepted
                    .as_ref()
                    .is_some_and(|first| first.solution_id == solution_id)
            {
                return Ok(Some(OrdinaryPublicationHandle(Arc::clone(entry))));
            }
        }
        Ok(None)
    }

    /// Reader validation also proves absence of solutions, checkpoints and proof payloads.
    pub(crate) async fn reconcile_ordinary_solve_custody(&self) -> Result<(), AppError> {
        let entries: Vec<_> = self
            .ordinary_solve_custody
            .lock()
            .map_err(|_| solution_contract_error())?
            .iter()
            .map(|(run_id, entry)| (*run_id, Arc::clone(entry)))
            .collect();
        let mut first_error = None;
        for (run_id, entry) in entries {
            let pending = {
                let state = lock_ordinary_state(&entry);
                if state.producer_done && state.checkpoint_done {
                    state.pending_start.clone()
                } else {
                    None
                }
            };
            if let Some(pending) = pending {
                let existing = match self
                    .store
                    .load_solve_run_by_request(pending.request_id)
                    .await
                {
                    Ok(existing) => existing,
                    Err(error) => {
                        first_error.get_or_insert_with(|| store_error(error));
                        continue;
                    }
                };
                let resolved = existing.as_ref().is_none_or(|existing| {
                    existing.input.run_id != run_id
                        || matches_ordinary_start(&pending, &existing.input, existing.started_at)
                });
                if resolved {
                    let mut state = lock_ordinary_state(&entry);
                    if state.producer_done && state.checkpoint_done {
                        state.pending_start = None;
                    }
                }
            }
            let alarm_binding = {
                let state = lock_ordinary_state(&entry);
                state
                    .binding
                    .as_ref()
                    .zip(state.alarm.as_ref())
                    .map(|(binding, alarm)| {
                        (
                            binding.scenario_id,
                            binding.request_id,
                            binding.revision,
                            binding.started_at,
                            binding.input_checksum.clone(),
                            alarm.diagnostic_code.clone(),
                        )
                    })
            };
            if let Some((scenario_id, request_id, revision, started_at, checksum, code)) =
                alarm_binding
            {
                let existing = match self.store.load_solve_run(scenario_id, run_id).await {
                    Ok(existing) => existing,
                    Err(error) => {
                        first_error.get_or_insert_with(|| store_error(error));
                        continue;
                    }
                };
                let exact = existing.as_ref().is_some_and(|existing| {
                    existing.input.run_id == run_id
                        && existing.input.request_id == request_id
                        && Revision::new(existing.input.scenario_revision) == revision
                        && existing.input.checksum == checksum
                        && existing.started_at == started_at
                        && existing.manifest.as_ref().is_some_and(|manifest| {
                            manifest.run_id == run_id
                                && manifest.run_input_checksum == checksum
                                && matches!(&manifest.outcome,
                                    eutheto_domain_ir::RunTerminalOutcomeV1::VerificationAlarm {
                                        diagnostic_code
                                    } if diagnostic_code == &code)
                        })
                });
                if exact {
                    lock_ordinary_state(&entry).confirmed = true;
                }
            }
            retire_ordinary_solve(&self.ordinary_solve_custody, run_id, &entry);
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn reserve_ordinary_solve_custody(
        &self,
        request: &NewSolveRunV1,
    ) -> Result<OrdinarySolveLease, AppError> {
        let mut entries = self
            .ordinary_solve_custody
            .lock()
            .map_err(|_| solution_contract_error())?;
        if entries.len() >= MAX_ORDINARY_SOLVE_CUSTODY || entries.contains_key(&request.run_id) {
            return Err(resource_limit_error());
        }
        let state = Arc::new(StdMutex::new(OrdinarySolveState {
            pending_start: Some(request.clone()),
            binding: None,
            first_accepted: None,
            alarm: None,
            secondary_cause: None,
            confirmed: false,
            producer_done: false,
            checkpoint_claimed: false,
            checkpoint_done: true,
            native_observations: None,
            observations: OrdinaryObservationBounds {
                latest_wall: None,
                latest_elapsed: None,
                unreliable: false,
            },
        }));
        entries.insert(request.run_id, Arc::clone(&state));
        Ok(OrdinarySolveLease {
            custody: Arc::clone(&self.ordinary_solve_custody),
            run_id: request.run_id,
            state,
        })
    }

    /// Announces an independently accepted result only after its exact local checkpoint
    /// has committed; listeners must still read store authority after this advisory event.
    pub(crate) fn publish_solve_verified(
        &self,
        checkpoint: &VerifiedCheckpoint,
        work: &OrdinaryCheckpointWork,
    ) -> Result<(), AppError> {
        if !Arc::ptr_eq(&self.ordinary_solve_custody, &work.custody) {
            return Err(solution_contract_error());
        }
        let input = &checkpoint.run_input;
        let mut state = lock_ordinary_state(&work.state);
        let binding = state.binding.as_ref().ok_or_else(solution_contract_error)?;
        let first = state
            .first_accepted
            .as_ref()
            .ok_or_else(solution_contract_error)?;
        if work.run_id != input.run_id
            || binding.run_id != input.run_id
            || binding.scenario_id != input.scenario_id
            || binding.request_id != input.request_id
            || binding.revision != Revision::new(input.scenario_revision)
            || binding.input_checksum != input.checksum
            || first.solution_id != checkpoint.accepted_result.solution.solution_id
            || first.checksum != checkpoint.accepted_result.checksum
            || first.accepted_at != checkpoint.accepted_at
        {
            return Err(solution_contract_error());
        }
        state.admit()?;
        let timestamp = self.clock.now();
        state.observe_wall(timestamp);
        if state.observations.unreliable {
            return Err(timing_error());
        }
        let _ = self.events.send(AppEvent {
            topic: EventTopic::SolveVerified,
            payload: EventPayload::SolveVerified {
                context: EventContext {
                    event_version: EVENT_VERSION,
                    timestamp,
                    request_id: Some(input.request_id),
                    scenario_id: Some(input.scenario_id),
                    revision: Some(Revision::new(input.scenario_revision)),
                    solve_run_id: Some(input.run_id),
                },
                solution_id: checkpoint.accepted_result.solution.solution_id,
                result_checksum: checkpoint.accepted_result.checksum.clone(),
                checkpoint_checksum: checkpoint.checksum.clone(),
            },
        });
        Ok(())
    }

    /// Publishes terminal state only after durable run finalization succeeds.
    pub fn publish_solve_completed(
        &self,
        request_id: RequestId,
        scenario_id: ScenarioId,
        revision: Revision,
        run_id: SolveRunId,
        status: SolveStatus,
        solution_id: Option<SolutionId>,
    ) {
        let _ = self.events.send(AppEvent {
            topic: EventTopic::SolveCompleted,
            payload: EventPayload::SolveCompleted {
                context: EventContext {
                    event_version: EVENT_VERSION,
                    timestamp: self.clock.now(),
                    request_id: Some(request_id),
                    scenario_id: Some(scenario_id),
                    revision: Some(revision),
                    solve_run_id: Some(run_id),
                },
                status,
                solution_id,
            },
        });
    }

    /// Executes one application mutation.
    /// # Errors
    ///
    /// Returns validation, conflict, unsupported, storage, or protocol errors
    /// without exposing backend diagnostics.
    pub async fn execute(&self, command: AppCommand) -> Result<AppCommandResult, AppError> {
        if !matches!(command, AppCommand::CancelPortablePreview { .. }) {
            self.check_cancelled()?;
        }
        match command {
            command @ (AppCommand::CreateProject { .. } | AppCommand::DuplicateProject { .. }) => {
                self.execute_project_creation_command(command).await
            }
            command @ (AppCommand::ArchiveProject { .. }
            | AppCommand::UnarchiveProject { .. }
            | AppCommand::DeleteProject { .. }) => {
                self.execute_project_lifecycle_command(command).await
            }
            AppCommand::ApplyScenario {
                request_id,
                envelope,
                truncate_redo,
            } => self
                .apply_scenario(
                    request_id,
                    envelope,
                    truncate_redo,
                    self.cancellation.clone(),
                )
                .await
                .map(AppCommandResult::ScenarioCommand),
            AppCommand::CreateDiagnosticCopy { request, control } => {
                self.create_diagnostic_copy(request, control).await
            }
            AppCommand::Undo {
                request_id,
                scenario_id,
                expected_revision,
            } => {
                self.move_history(request_id, scenario_id, expected_revision, true)
                    .await
            }
            AppCommand::Redo {
                request_id,
                scenario_id,
                expected_revision,
            } => {
                self.move_history(request_id, scenario_id, expected_revision, false)
                    .await
            }
            command @ (AppCommand::SetSetting { .. } | AppCommand::DeleteSetting { .. }) => {
                self.execute_setting_command(command).await
            }
            AppCommand::SolutionSelect(request) => self.select_solution(request).await,
            AppCommand::AdoptBaseSchedule(request) => self.adopt_base_schedule(request).await,
            AppCommand::SolutionStartCounterfactual(request) => {
                self.start_counterfactual(request).await
            }
            AppCommand::SolutionCancelCounterfactual(request) => {
                self.cancel_counterfactual(request).await
            }
            command @ (AppCommand::ApplyImport { .. }
            | AppCommand::ApplyRestore { .. }
            | AppCommand::CancelPortablePreview { .. }
            | AppCommand::ExportScenario { .. }
            | AppCommand::CreateBackup { .. }
            | AppCommand::PublishPreparedPortable { .. }
            | AppCommand::ExactReexportUnopenedBundle { .. }) => {
                self.execute_portable_command(command).await
            }
            AppCommand::Deferred(capability) => Err(unsupported(capability)),
        }
    }
    async fn execute_project_creation_command(
        &self,
        command: AppCommand,
    ) -> Result<AppCommandResult, AppError> {
        match command {
            AppCommand::CreateProject {
                request_id,
                title,
                description,
                domain_pack,
                settings,
            } => {
                self.create_project(request_id, title, description, domain_pack, settings)
                    .await
            }
            AppCommand::DuplicateProject {
                request_id,
                source_id,
                expected_revision,
                title,
            } => {
                self.duplicate_project(request_id, source_id, expected_revision, title)
                    .await
            }
            _ => unreachable!("dispatcher passes only project creation commands"),
        }
    }

    async fn execute_portable_command(
        &self,
        command: AppCommand,
    ) -> Result<AppCommandResult, AppError> {
        match command {
            AppCommand::ExportScenario {
                scenario_id,
                destination,
                cancellation,
            } => {
                let (bytes, _, _) = self.export_scenario(scenario_id, &cancellation).await?;
                self.write_bundle(destination, bytes, &cancellation).await?;
                Ok(AppCommandResult::BundleWritten)
            }
            AppCommand::CreateBackup {
                title,
                destination,
                selection,
                cancellation,
            } => {
                let (bytes, summary, _) =
                    self.export_backup(title, selection, &cancellation).await?;
                self.write_bundle(destination, bytes, &cancellation).await?;
                Ok(AppCommandResult::BackupWritten(summary))
            }
            AppCommand::PublishPreparedPortable {
                destination,
                bytes,
                expected_sha256,
                binding,
                cancellation,
            } => {
                self.publish_prepared_portable(
                    destination,
                    bytes,
                    &expected_sha256,
                    binding,
                    &cancellation,
                )
                .await
            }
            AppCommand::ExactReexportUnopenedBundle {
                preview_id,
                destination,
                cancellation,
            } => {
                self.exact_reexport_unopened_bundle(preview_id, destination, &cancellation)
                    .await
            }
            AppCommand::ApplyImport {
                request_id,
                preview_id,
                collision_plan,
                cancellation,
            } => {
                self.apply_portable(request_id, preview_id, collision_plan, None, cancellation)
                    .await
            }
            AppCommand::ApplyRestore {
                request_id,
                preview_id,
                collision_plan,
                authorization,
                cancellation,
            } => {
                self.apply_portable(
                    request_id,
                    preview_id,
                    collision_plan,
                    Some(authorization),
                    cancellation,
                )
                .await
            }
            AppCommand::CancelPortablePreview { preview_id } => {
                self.cancel_portable_preview(preview_id).await
            }
            _ => unreachable!("dispatcher passes only portable commands"),
        }
    }

    async fn duplicate_project(
        &self,
        request_id: RequestId,
        source_id: ScenarioId,
        expected_revision: Revision,
        title: String,
    ) -> Result<AppCommandResult, AppError> {
        let mutation = self.scenario_lock(source_id).await;
        let _guard = mutation.lock().await;
        let library = self
            .store
            .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
            .await
            .map_err(store_error)?;
        let source = library
            .projects
            .iter()
            .find(|project| project.document.scenario_id == source_id)
            .ok_or(AppError::NotFound(ResourceRef::Scenario(source_id)))?;
        if source.summary.revision != expected_revision {
            return Err(AppError::Conflict {
                expected_revision,
                actual_revision: source.summary.revision,
            });
        }
        let source_owned = collect_scenario_owned_uuids(&portable_scenario(source, true));
        let LibraryUuidClosure { occupied, .. } = library_uuid_closure(&library);
        let mut allocated = BTreeSet::new();
        let target_uuid = allocate_unique_uuid(self.ids.as_ref(), &occupied, &mut allocated)?;
        let id = ScenarioId::from_uuid(target_uuid);
        let mut id_remap = BTreeMap::from([(source_id.as_uuid(), target_uuid)]);
        for old_id in source_owned {
            if old_id != source_id.as_uuid() {
                let replacement =
                    allocate_unique_uuid(self.ids.as_ref(), &occupied, &mut allocated)?;
                id_remap.insert(old_id, replacement);
            }
        }
        let project = self
            .store
            .duplicate_project(
                source_id,
                expected_revision,
                id,
                id_remap,
                title,
                self.clock.now(),
                |original, copied, mapping| {
                    normalize_workforce_manual_copy(original, copied, mapping, mapping, None)
                },
            )
            .await
            .map_err(store_error)?;
        self.publish_changed_event(
            project.document.scenario_id,
            request_id,
            project.summary.revision,
            ChangeSet {
                changes: vec![Change {
                    kind: ChangeKind::Added,
                    path: "/project".to_owned(),
                    before: None,
                    after: Some(Value::Bool(true)),
                }],
            },
        );
        Ok(AppCommandResult::Project(project_metadata(&project)))
    }

    async fn execute_project_lifecycle_command(
        &self,
        command: AppCommand,
    ) -> Result<AppCommandResult, AppError> {
        let (request_id, scenario_id, expected_revision, action) = match command {
            AppCommand::ArchiveProject {
                request_id,
                scenario_id,
                expected_revision,
            } => (
                request_id,
                scenario_id,
                expected_revision,
                ProjectLifecycleAction::Archive,
            ),
            AppCommand::UnarchiveProject {
                request_id,
                scenario_id,
                expected_revision,
            } => (
                request_id,
                scenario_id,
                expected_revision,
                ProjectLifecycleAction::Unarchive,
            ),
            AppCommand::DeleteProject {
                request_id,
                scenario_id,
                expected_revision,
            } => (
                request_id,
                scenario_id,
                expected_revision,
                ProjectLifecycleAction::Delete,
            ),
            _ => {
                return Err(protocol_error(
                    "protocol.command_dispatch",
                    "The application received an invalid lifecycle command.",
                    false,
                ));
            }
        };
        self.execute_project_lifecycle(request_id, scenario_id, expected_revision, action)
            .await
    }

    async fn execute_project_lifecycle(
        &self,
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        action: ProjectLifecycleAction,
    ) -> Result<AppCommandResult, AppError> {
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let (kind, before, after) = match action {
            ProjectLifecycleAction::Archive => {
                self.store
                    .archive_project(scenario_id, expected_revision, self.clock.now())
                    .await
                    .map_err(store_error)?;
                (
                    ChangeKind::Updated,
                    Some(Value::Bool(false)),
                    Some(Value::Bool(true)),
                )
            }
            ProjectLifecycleAction::Unarchive => {
                self.store
                    .unarchive_project(scenario_id, expected_revision)
                    .await
                    .map_err(store_error)?;
                (
                    ChangeKind::Updated,
                    Some(Value::Bool(true)),
                    Some(Value::Bool(false)),
                )
            }
            ProjectLifecycleAction::Delete => {
                self.store
                    .delete_project(scenario_id, expected_revision)
                    .await
                    .map_err(store_error)?;
                (ChangeKind::Removed, Some(Value::Bool(true)), None)
            }
        };
        self.publish_changed_event(
            scenario_id,
            request_id,
            expected_revision,
            ChangeSet {
                changes: vec![Change {
                    kind,
                    path: if matches!(action, ProjectLifecycleAction::Delete) {
                        "/project".to_owned()
                    } else {
                        "/archived".to_owned()
                    },
                    before,
                    after,
                }],
            },
        );
        Ok(AppCommandResult::Deleted)
    }

    /// Executes one read-only application query.
    ///
    /// # Errors
    ///
    /// Returns a typed not-found, unsupported, storage, or protocol failure.
    pub async fn query(&self, query: AppQuery) -> Result<AppQueryResult, AppError> {
        self.check_cancelled()?;
        match query {
            AppQuery::ListProjects(scope) => {
                let projects = self
                    .store
                    .list_projects(match scope {
                        ProjectScope::Active => ProjectListScope::Active,
                        ProjectScope::Archived => ProjectListScope::Archived,
                        ProjectScope::All => ProjectListScope::All,
                    })
                    .await
                    .map_err(store_error)?;
                Ok(AppQueryResult::Projects(
                    projects.into_iter().map(project_summary).collect(),
                ))
            }
            AppQuery::ProjectMetadata(id) => {
                let project = self.store.get_project(id).await.map_err(store_error)?;
                Ok(AppQueryResult::Project(project_metadata(&project)))
            }
            AppQuery::OpenProject(id) => self.query_open_project(id).await,
            AppQuery::ScenarioView(id) => self.query_scenario_view(id).await,
            AppQuery::ValidateScenario(id) => self.query_scenario_validation(id).await,
            AppQuery::History(id) => self
                .store
                .history(id)
                .await
                .map(AppQueryResult::History)
                .map_err(store_error),
            AppQuery::HistoryPage(request) => self
                .history_page(request)
                .await
                .map(AppQueryResult::HistoryPage),
            AppQuery::Setting(key) => {
                validate_app_setting_key(&key)?;
                self.store
                    .get_setting(key)
                    .await
                    .map(AppQueryResult::Setting)
                    .map_err(store_error)
            }
            AppQuery::SolutionGetCounterfactual(request) => {
                self.query_counterfactual_job(request).await
            }
            AppQuery::SolutionListCounterfactuals(request) => {
                self.query_counterfactual_jobs(request).await
            }
            AppQuery::SolutionList(request) => self.query_solution_list(request).await,
            AppQuery::SolutionGetSummary(request) => self.query_solution_summary(request).await,
            AppQuery::SolutionGetView(request) => self
                .solution_view_with_cancellation(request, self.cancellation.child())
                .await
                .map(|view| AppQueryResult::SolutionView(Box::new(view))),
            AppQuery::SolutionVerify(request) => self.query_solution_verify(request).await,
            AppQuery::SolutionCompareV3(request) => self.query_solution_compare(request).await,
            AppQuery::SolutionExplain {
                request,
                source_revision,
                control,
            } => {
                self.query_solution_explain(request, source_revision, control)
                    .await
            }
            query @ (AppQuery::PreviewImport { .. }
            | AppQuery::PreviewRestore { .. }
            | AppQuery::ExportScenario { .. }
            | AppQuery::ExportBackup { .. }
            | AppQuery::InspectUnopenedBundle { .. }) => self.query_portable(query).await,
            AppQuery::SupportPreview => self
                .support_preview()
                .await
                .map(AppQueryResult::SupportPreview),
            AppQuery::Deferred(capability) => Err(unsupported(capability)),
            AppQuery::ListDomainPacks => Ok(AppQueryResult::DomainPacks(
                self.pack_registry.descriptors().cloned().collect(),
            )),
            AppQuery::DescribeDomainPack(id) => self.query_domain_pack_metadata(id),
            AppQuery::ListSolvers => Ok(AppQueryResult::Solvers(
                self.solver_registry.descriptors().cloned().collect(),
            )),
            AppQuery::DescribeSolver(id) => self
                .solver_registry
                .get(&id)
                .map(|backend| backend.descriptor().clone())
                .map(AppQueryResult::Solver)
                .ok_or(AppError::NotFound(ResourceRef::Backend(id))),
            AppQuery::SolverSupportMatrix => Ok(self.query_solver_support_matrix()),
            AppQuery::DeferredSolverGates => {
                let mut candidates = self.solver_registry.matrix().deferred_candidates().to_vec();
                candidates.sort_by(|left, right| left.backend_id.cmp(&right.backend_id));
                Ok(AppQueryResult::DeferredSolverGates(candidates))
            }
        }
    }

    async fn query_portable(&self, query: AppQuery) -> Result<AppQueryResult, AppError> {
        match query {
            AppQuery::PreviewImport {
                bytes,
                options,
                cancellation,
            } => {
                self.query_preview_import(bytes, options, &cancellation)
                    .await
            }
            AppQuery::PreviewRestore {
                bytes,
                options,
                cancellation,
            } => {
                self.query_preview_restore(bytes, options, &cancellation)
                    .await
            }
            AppQuery::ExportScenario {
                scenario_id,
                cancellation,
            } => {
                let (bytes, scenario_revision, library_revision) =
                    self.export_scenario(scenario_id, &cancellation).await?;
                Ok(AppQueryResult::Bundle {
                    bytes,
                    scenario_revision,
                    library_revision,
                })
            }
            AppQuery::ExportBackup {
                title,
                selection,
                cancellation,
            } => {
                let (bytes, summary, library_revision) =
                    self.export_backup(title, selection, &cancellation).await?;
                Ok(AppQueryResult::BackupBundle {
                    bytes,
                    summary,
                    library_revision,
                })
            }
            AppQuery::InspectUnopenedBundle {
                bytes,
                cancellation,
            } => self.inspect_unopened_bundle(bytes, &cancellation).await,
            _ => unreachable!("dispatcher passes only portable queries"),
        }
    }

    /// Records an explicit project opening and returns only bounded list metadata.
    ///
    /// # Errors
    ///
    /// Rejects cancelled requests, missing or malformed projects, and storage failures.
    pub async fn open_project_metadata(
        &self,
        id: ScenarioId,
    ) -> Result<ProjectListItemV1, AppError> {
        self.check_cancelled()?;
        let project = self
            .store
            .open_project(id, self.clock.now())
            .await
            .map_err(store_error)?;
        Ok(project_summary(project.summary))
    }

    async fn query_open_project(&self, id: ScenarioId) -> Result<AppQueryResult, AppError> {
        let project = self
            .store
            .open_project(id, self.clock.now())
            .await
            .map_err(store_error)?;
        Ok(AppQueryResult::Scenario(Box::new(scenario_view(
            project,
            &self.pack_registry,
        ))))
    }

    async fn query_scenario_view(&self, id: ScenarioId) -> Result<AppQueryResult, AppError> {
        let project = self.store.get_project(id).await.map_err(store_error)?;
        Ok(AppQueryResult::Scenario(Box::new(scenario_view(
            project,
            &self.pack_registry,
        ))))
    }

    async fn query_scenario_validation(&self, id: ScenarioId) -> Result<AppQueryResult, AppError> {
        let project = self.store.get_project(id).await.map_err(store_error)?;
        Ok(AppQueryResult::Validation(validate(
            &project.document,
            &self.pack_registry,
        )))
    }

    async fn select_solution(
        &self,
        request: SolutionSelectRequestV1,
    ) -> Result<AppCommandResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let scenario_id = request.scenario_id;
        let ordinary = self.ordinary_solution_handle(scenario_id, request.solution_id)?;
        let mut source_admitted = false;
        let result = async {
            let cancellation = self.cancellation.child();
            let _query_slot = self.acquire_accepted_query_slot(&cancellation).await?;
            let stored = {
                let mutation = self.scenario_lock(scenario_id).await;
                let _guard = mutation.lock().await;
                let actual_revision = self.current_scenario_revision(scenario_id).await?;
                if request.expected_revision != actual_revision {
                    return Err(AppError::Conflict {
                        expected_revision: request.expected_revision,
                        actual_revision,
                    });
                }
                let stored = self
                    .store
                    .load_accepted_result(request.solution_id)
                    .await
                    .map_err(store_error)?;
                if stored.portable.scenario_id != scenario_id
                    || stored.document.scenario_id != scenario_id
                    || stored.portable.accepted_result.solution.scenario_id != scenario_id
                {
                    return Err(AppError::NotFound(ResourceRef::Solution(
                        request.solution_id,
                    )));
                }
                stored
            };
            if let Some(handle) = &ordinary {
                handle.admit_input(&stored.portable.run_input)?;
            }
            source_admitted = true;
            let verified = self.ensure_fresh_accepted(stored, cancellation).await?;
            let mutation = self.scenario_lock(scenario_id).await;
            let _guard = mutation.lock().await;
            ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
            let selected = self
                .store
                .select_accepted_result(
                    scenario_id,
                    request.expected_revision,
                    request.solution_id,
                    verified,
                )
                .await
                .map_err(store_error)?;
            Ok(AppCommandResult::SolutionSelected(solution_summary(
                &selected,
            )))
        }
        .await;
        if source_admitted {
            ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        }
        result
    }
    // Keep source admission, verified preparation and the final revision CAS in order.
    #[allow(clippy::too_many_lines)]
    async fn adopt_base_schedule(
        &self,
        request: SolutionAdoptBaseRequestV1,
    ) -> Result<AppCommandResult, AppError> {
        ensure_adopt_base_schema(request.schema_version)?;
        request
            .result
            .validate()
            .map_err(|_| solution_reference_error("/result"))?;

        let scenario_id = request.scenario_id;
        let expected_revision = request.expected_revision;
        let ordinary = self.ordinary_reference_handle(scenario_id, &request.result)?;
        let mut source_admitted = false;
        let result = async {
            let cancellation = self.cancellation.child();
            let _query_slot = self.acquire_accepted_query_slot(&cancellation).await?;

            // Capture and independently verify the immutable accepted source without holding the
            // per-scenario mutation mutex. The final ordinary scenario command below is the CAS and
            // validates person/shift references against the current document in the same transaction.
            let actual_revision = self.current_scenario_revision(scenario_id).await?;
            if expected_revision != actual_revision {
                return Err(AppError::Conflict {
                    expected_revision,
                    actual_revision,
                });
            }
            let stored = self
                .load_referenced_solution(scenario_id, &request.result)
                .await?;
            if let Some(handle) = &ordinary {
                handle.admit_input(&stored.portable.run_input)?;
            }
            source_admitted = true;
            let verified = self.ensure_fresh_accepted(stored, cancellation).await?;
            if verified.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
                return Err(unsupported_project_pack(&verified.document.domain_pack));
            }
            // This read is only used to prepare the typed pack payload. apply_scenario performs the
            // authoritative expected-revision CAS and retries no part of this preparation.
            let project = self
                .store
                .get_project(scenario_id)
                .await
                .map_err(store_error)?;
            if project.summary.revision != expected_revision {
                return Err(AppError::Conflict {
                    expected_revision,
                    actual_revision: project.summary.revision,
                });
            }
            if project.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
                return Err(unsupported_project_pack(&project.document.domain_pack));
            }
            // A previously accepted source may be adopted historically; retain its source revision
            // while the current document reference check remains authoritative at commit.
            let source_revision = solution_revision(&verified)?;
            let assignments = selected_workforce_assignments(&verified.portable.accepted_result)?;
            let source_solution_id = verified.portable.accepted_result.solution.solution_id;
            let existing = workforce_base_schedule(&project.document)?;
            let (entity_id, command_type) = if let Some(base) = &existing {
                // An exact repeat is an explicit no-op: do not manufacture a revision or journal row.
                if base.source_solution_id == source_solution_id
                    && base.source_revision == source_revision
                    && base.assignments == assignments
                {
                    return Err(validation_error(
                        "solution.base_adoption_noop",
                        "/result",
                        "The accepted result is already the current base schedule.",
                    ));
                }
                (base.id, UPDATE_ENTITY)
            } else {
                let occupied = collect_scenario_owned_uuids(&portable_scenario(&project, true));
                (
                    EntityId::from_uuid(allocate_unique_uuid(
                        self.ids.as_ref(),
                        &occupied,
                        &mut BTreeSet::new(),
                    )?),
                    ADD_ENTITY,
                )
            };

            let payload = serde_json::to_value(EntityPayload {
                entity: WorkforceEntity::BaseSchedule(BaseSchedule {
                    id: entity_id,
                    source_solution_id,
                    source_revision,
                    assignments,
                }),
            })
            .map_err(|_| solution_contract_error())?;
            let envelope = CommandEnvelope {
                command_id: request.command_id,
                scenario_id,
                expected_revision,
                actor: request.actor,
                source: CommandSource::Desktop,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: command_type.to_owned(),
                    payload,
                }),
            };
            ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
            self.apply_scenario(
                request.request_id,
                envelope,
                request.truncate_redo,
                self.cancellation.clone(),
            )
            .await
            .map(AppCommandResult::ScenarioCommand)
        }
        .await;
        if source_admitted {
            ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        }
        result
    }

    async fn start_counterfactual(
        &self,
        request: SolutionStartCounterfactualRequestV1,
    ) -> Result<AppCommandResult, AppError> {
        let request_id = request.request_id;
        let scenario_id = request.scenario_id;
        let ordinary = self.ordinary_solution_handle(scenario_id, request.base_solution_id)?;
        let mut source_admitted = false;
        let result = async {
            let cancellation = self.cancellation.child();
            let _query_slot = self.acquire_accepted_query_slot(&cancellation).await?;
            let stored = {
                let mutation = self.scenario_lock(scenario_id).await;
                let _guard = mutation.lock().await;
                self.load_solution(scenario_id, request.base_solution_id)
                    .await?
            };
            crate::counterfactual::validate_counterfactual_source(&request, &stored)?;
            if let Some(handle) = &ordinary {
                handle.admit_input(&stored.portable.run_input)?;
            }
            source_admitted = true;
            let verified = self.ensure_fresh_accepted(stored, cancellation).await?;
            let mutation = self.scenario_lock(scenario_id).await;
            let _guard = mutation.lock().await;
            ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
            let job = Box::pin(self.counterfactual.start(
                request,
                verified,
                Arc::clone(&self.monotonic_clock),
            ))
            .await?;
            let current_revision = self.current_scenario_revision(scenario_id).await;
            self.counterfactual
                .guard_job_publication(scenario_id, job.request.job_id)
                .await?;
            let current_revision = current_revision?;
            if let Some(result) = &job.result {
                crate::native_observations::validate_counterfactual_fingerprints(result)
                    .map_err(|_| solution_contract_error())?;
            }
            Ok(AppCommandResult::CounterfactualStarted(
                SolutionStartCounterfactualDtoV1 {
                    schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                    request_id,
                    current_revision,
                    job,
                },
            ))
        }
        .await;
        if source_admitted {
            ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        }
        result
    }

    async fn cancel_counterfactual(
        &self,
        request: SolutionCancelCounterfactualRequestV1,
    ) -> Result<AppCommandResult, AppError> {
        let cancel_request_id = request.cancel_request_id;
        let scenario_id = request.scenario_id;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let job = self.counterfactual.cancel(&request).await?;
        let current_revision = self.current_scenario_revision(scenario_id).await;
        self.counterfactual
            .guard_job_publication(scenario_id, job.request.job_id)
            .await?;
        let current_revision = current_revision?;
        if let Some(result) = &job.result {
            crate::native_observations::validate_counterfactual_fingerprints(result)
                .map_err(|_| solution_contract_error())?;
        }
        Ok(AppCommandResult::CounterfactualCancelled(
            SolutionCancelCounterfactualDtoV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                cancel_request_id,
                current_revision,
                job,
            },
        ))
    }

    async fn query_counterfactual_job(
        &self,
        request: SolutionGetCounterfactualRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_counterfactual_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        self.counterfactual
            .guard_job_publication(request.scenario_id, request.job_id)
            .await?;
        let job = self.store.load_counterfactual_job(request.job_id).await;
        self.counterfactual
            .guard_job_publication(request.scenario_id, request.job_id)
            .await?;
        let job = job.map_err(store_error)?;
        if job.request.semantics.scenario_id != request.scenario_id {
            return Err(validation_error(
                "solution.counterfactual_not_found",
                "/jobId",
                "The selected counterfactual was not found in this scenario.",
            ));
        }
        let current_revision = self.current_scenario_revision(request.scenario_id).await;
        self.counterfactual
            .guard_job_publication(request.scenario_id, request.job_id)
            .await?;
        let current_revision = current_revision?;
        if let Some(result) = &job.result {
            crate::native_observations::validate_counterfactual_fingerprints(result)
                .map_err(|_| solution_contract_error())?;
        }
        Ok(AppQueryResult::SolutionGetCounterfactual(Box::new(
            SolutionGetCounterfactualDtoV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                current_revision,
                job,
            },
        )))
    }

    async fn query_counterfactual_jobs(
        &self,
        request: SolutionListCounterfactualsRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_counterfactual_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let page = self
            .store
            .list_counterfactual_jobs(request.scenario_id, request.before, request.limit)
            .await
            .map_err(store_error)?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await;
        for job in &page.jobs {
            self.counterfactual
                .guard_job_publication(request.scenario_id, job.job_id)
                .await?;
        }
        let current_revision = current_revision?;
        Ok(AppQueryResult::SolutionListCounterfactuals(Box::new(
            SolutionListCounterfactualsDtoV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                current_revision,
                page,
            },
        )))
    }

    async fn query_solution_list(
        &self,
        request: SolutionListRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let mut ordinary = self.ordinary_scenario_handles(request.scenario_id)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let summaries = self
            .store
            .list_accepted_results(request.scenario_id)
            .await
            .map_err(store_error)?;
        for summary in &summaries {
            if !ordinary.iter().any(|(run_id, _)| *run_id == summary.run_id)
                && let Some(handle) =
                    self.ordinary_run_handle(request.scenario_id, summary.run_id)?
            {
                ordinary.push((summary.run_id, handle));
            }
        }
        let current_revision = self.current_scenario_revision(request.scenario_id).await;
        for summary in &summaries {
            self.counterfactual
                .guard_run_publication(request.scenario_id, summary.run_id)
                .await?;
        }
        let current_revision = current_revision?;
        for summary in &summaries {
            if let Some((_, handle)) = ordinary
                .iter()
                .find(|(run_id, _)| *run_id == summary.run_id)
            {
                handle.validate_summary(summary)?;
            }
        }
        for summary in &summaries {
            if let Some((_, handle)) = ordinary
                .iter()
                .find(|(run_id, _)| *run_id == summary.run_id)
            {
                handle.admit()?;
            }
        }
        Ok(AppQueryResult::SolutionList(SolutionListDtoV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            current_revision,
            freshly_verified: false,
            solutions: summaries.iter().map(solution_summary).collect(),
        }))
    }

    async fn query_solution_summary(
        &self,
        request: SolutionSummaryRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let ordinary = self.ordinary_solution_handle(request.scenario_id, request.solution_id)?;
        let result = async {
            let cancellation = self.cancellation.child();
            let _query_slot = self.acquire_accepted_query_slot(&cancellation).await?;
            let (stored, selected, current_revision) = {
                let mutation = self.scenario_lock(request.scenario_id).await;
                let _guard = mutation.lock().await;
                let stored = self
                    .load_solution(request.scenario_id, request.solution_id)
                    .await?;
                let selected = self
                    .store
                    .selected_accepted_result(request.scenario_id)
                    .await
                    .map_err(store_error)?
                    .is_some_and(|summary| summary.result.solution_id == request.solution_id);
                let current_revision = self.current_scenario_revision(request.scenario_id).await?;
                (stored, selected, current_revision)
            };
            if let Some(handle) = &ordinary {
                handle.admit_input(&stored.portable.run_input)?;
            }
            let stored = self.ensure_fresh_accepted(stored, cancellation).await?;
            Ok(AppQueryResult::SolutionSummary(Box::new(solution_detail(
                &stored,
                current_revision,
                selected,
            )?)))
        }
        .await;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        result
    }

    /// Captures an exact accepted result/document pair before projecting off the async executor.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported requests, unavailable accepted results, storage or
    /// projection failures, cancellation, or exhausted response limits.
    // Snapshot capture, authority refresh and off-executor projection share one query-slot owner.
    #[allow(clippy::too_many_lines)]
    pub async fn solution_view_with_cancellation(
        &self,
        request: SolutionViewRequestV1,
        cancellation: CancellationToken,
    ) -> Result<SolutionViewDtoV1, AppError> {
        const QUERY_BYTES: usize = 64 * 1024;
        let deadline = tokio::time::Instant::now() + ACCEPTED_VERIFICATION_TIMEOUT;
        let _cancel_on_drop = CancelOnDrop::new(cancellation.clone());
        let cancellation_for_gate = cancellation.clone();
        let control = OperationControl::Cancellation(cancellation);
        self.check_cancelled()?;
        control.check().map_err(operation_interrupted)?;
        ensure_solution_schema(request.schema_version)?;
        measure_accepted_view_json_size(&request, QUERY_BYTES)
            .map_err(|error| solution_view_error(&error))?;
        let query = AcceptedDomainQueryV1 {
            schema_version: 1,
            view_id: request.view_id.clone(),
            parameters: request.parameters.clone(),
            continuation: request.continuation.clone(),
        };
        let (source, current_revision, query_slot, ordinary) = self
            .capture_accepted_source(
                request.scenario_id,
                request.solution_id,
                &cancellation_for_gate,
                deadline,
            )
            .await?;
        let result = async {
            let source = match source {
                AcceptedQuerySource::Terminal(stored) => AcceptedQuerySource::Terminal(
                    self.ensure_fresh_accepted_until(
                        stored,
                        OperationControl::Cancellation(cancellation_for_gate),
                        deadline,
                        None,
                    )
                    .await?,
                ),
                checkpoint @ AcceptedQuerySource::Checkpoint(..) => checkpoint,
            };
            let run_id = source.run_id();
            self.counterfactual
                .guard_run_publication(request.scenario_id, run_id)
                .await?;
            control.check().map_err(operation_interrupted)?;
            let registry = Arc::clone(&self.pack_registry);
            let view = tokio::task::spawn_blocking(move || {
                let _query_slot = query_slot;
                control.check().map_err(operation_interrupted)?;
                let (document, accepted, scenario_revision, objective_context) = match source {
                    AcceptedQuerySource::Terminal(stored) => {
                        let revision = solution_revision(&stored)?;
                        (
                            stored.document,
                            stored.portable.accepted_result,
                            revision,
                            stored.portable.run_input.objective_context,
                        )
                    }
                    AcceptedQuerySource::Checkpoint(document, checkpoint) => {
                        reverify_checkpoint_solution(&document, &checkpoint, &registry, &control)?;
                        let revision = Revision::try_new(checkpoint.run_input.scenario_revision)
                            .map_err(|_| solution_contract_error())?;
                        (
                            document,
                            checkpoint.accepted_result,
                            revision,
                            checkpoint.run_input.objective_context,
                        )
                    }
                };
                let pack = available_pack(&document, &registry)
                    .ok_or_else(|| unsupported_project_pack(&document.domain_pack))?;
                let context = SetupViewContext {
                    revision: scenario_revision,
                    query_fingerprint: accepted_view_fingerprint(&accepted, &query)?,
                };
                let output = pack
                    .build_view(
                        eutheto_domain_api::DomainViewInput::AcceptedSolution {
                            document: &document,
                            solution: &accepted.solution,
                            objective_context: objective_context.as_ref(),
                            view_id: &query.view_id,
                            query: &query,
                            context,
                        },
                        &control,
                    )
                    .map_err(|error| solution_view_error(&error))?;
                control.check().map_err(operation_interrupted)?;
                if output.reconciliation.is_some() {
                    return Err(solution_view_error(&DomainPackError::Contract(
                        "accepted-solution view returned command reconciliation".to_owned(),
                    )));
                }
                Ok(SolutionViewDtoV1 {
                    schema_version: SOLUTION_API_SCHEMA_VERSION,
                    scenario_id: request.scenario_id,
                    current_revision,
                    scenario_revision,
                    result: accepted_reference(&accepted)?,
                    view: output.view,
                })
            });
            let projected = tokio::time::timeout_at(deadline, view).await;
            self.counterfactual
                .guard_run_publication(request.scenario_id, run_id)
                .await?;
            projected
                .map_err(|_| operation_interrupted(OperationInterruption::DeadlineExceeded))?
                .map_err(join_error)?
        }
        .await;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        result
    }

    async fn query_solution_verify(
        &self,
        request: SolutionVerifyRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        let deadline = tokio::time::Instant::now() + ACCEPTED_VERIFICATION_TIMEOUT;
        ensure_solution_schema(request.schema_version)?;
        let cancellation = self.cancellation.child();
        let _cancel_on_drop = CancelOnDrop::new(cancellation.clone());
        let (source, current_revision, _query_slot, ordinary) = self
            .capture_accepted_source(
                request.scenario_id,
                request.solution_id,
                &cancellation,
                deadline,
            )
            .await?;
        let result = async {
            let permit = self
                .acquire_verification_slot(
                    &OperationControl::Cancellation(cancellation.clone()),
                    deadline,
                )
                .await?;
            let (accepted, scenario_revision, report) = match source {
                AcceptedQuerySource::Terminal(stored) => {
                    let (stored, report) = self
                        .verify_accepted_off_lock(
                            stored,
                            OperationControl::Cancellation(cancellation.child()),
                            permit,
                            deadline,
                            None,
                        )
                        .await?;
                    let scenario_revision = solution_revision(&stored)?;
                    (stored.portable.accepted_result, scenario_revision, report)
                }
                AcceptedQuerySource::Checkpoint(document, checkpoint) => {
                    let (checkpoint, report) = self
                        .verify_checkpoint_off_lock(
                            document,
                            checkpoint,
                            cancellation.clone(),
                            permit,
                            deadline,
                        )
                        .await?;
                    let scenario_revision =
                        Revision::try_new(checkpoint.run_input.scenario_revision)
                            .map_err(|_| solution_contract_error())?;
                    (checkpoint.accepted_result, scenario_revision, report)
                }
            };
            Ok(AppQueryResult::SolutionVerification(Box::new(
                SolutionVerificationDtoV1 {
                    schema_version: SOLUTION_API_SCHEMA_VERSION,
                    scenario_id: request.scenario_id,
                    current_revision,
                    scenario_revision,
                    result: accepted_reference(&accepted)?,
                    verification: report,
                },
            )))
        }
        .await;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        result
    }

    async fn query_solution_compare(
        &self,
        request: SolutionCompareRequestV3,
    ) -> Result<AppQueryResult, AppError> {
        if request.schema_version != SOLUTION_COMPARE_SCHEMA_VERSION {
            return Err(unsupported_solution_schema());
        }
        let (base, candidate, current_revision, query_slot, control, deadline, ordinary) = self
            .comparison_sources(
                request.scenario_id,
                request.base_solution_id,
                request.candidate_solution_id,
            )
            .await?;
        let app = self.clone();
        let comparison_control = control.clone();
        let mut comparison = tokio::spawn(async move {
            app.compare_source_records(base, candidate, comparison_control, query_slot)
                .await
                .map(|(_, comparison)| comparison)
        });
        let result = if let Ok(joined) = tokio::time::timeout_at(deadline, &mut comparison).await {
            joined.map_err(join_error).and_then(|result| result)
        } else {
            control.cancellation().cancel();
            let _ = comparison.await;
            Err(operation_interrupted(
                OperationInterruption::DeadlineExceeded,
            ))
        };
        for handle in ordinary.iter().flatten() {
            handle.admit()?;
        }
        result.map(|comparison| {
            AppQueryResult::SolutionComparisonV3(Box::new(SolutionComparisonDtoV3 {
                schema_version: SOLUTION_COMPARE_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                current_revision,
                comparison,
            }))
        })
    }

    async fn comparison_sources(
        &self,
        scenario_id: ScenarioId,
        base_solution_id: SolutionId,
        candidate_solution_id: SolutionId,
    ) -> Result<
        (
            StoredAcceptedResultV2,
            StoredAcceptedResultV2,
            Revision,
            Arc<tokio::sync::OwnedSemaphorePermit>,
            OperationControl,
            tokio::time::Instant,
            [Option<OrdinaryPublicationHandle>; 2],
        ),
        AppError,
    > {
        let ordinary = [
            self.ordinary_solution_handle(scenario_id, base_solution_id)?,
            self.ordinary_solution_handle(scenario_id, candidate_solution_id)?,
        ];
        for handle in ordinary.iter().flatten() {
            handle.admit()?;
        }
        let result = async {
            let cancellation = self.cancellation.child();
            let query_slot = Arc::new(self.acquire_accepted_query_slot(&cancellation).await?);
            let (base, candidate, current_revision) = {
                let mutation = self.scenario_lock(scenario_id).await;
                let _guard = mutation.lock().await;
                let base = self.load_solution(scenario_id, base_solution_id).await?;
                let candidate = self
                    .load_solution(scenario_id, candidate_solution_id)
                    .await?;
                for (handle, input) in ordinary
                    .iter()
                    .zip([&base.portable.run_input, &candidate.portable.run_input])
                {
                    if let Some(handle) = handle {
                        handle.validate_input(input)?;
                    }
                }
                for handle in ordinary.iter().flatten() {
                    handle.admit()?;
                }
                let current_revision = self.current_scenario_revision(scenario_id).await?;
                (base, candidate, current_revision)
            };
            let budget = eutheto_types::ParentSolveBudget::new(
                eutheto_types::DurationMillis::new(30_000).map_err(|_| resource_limit_error())?,
                Arc::clone(&self.monotonic_clock),
                cancellation,
            )
            .map_err(|_| resource_limit_error())?;
            let control = OperationControl::Solve(budget.phase_view());
            let deadline = tokio::time::Instant::now() + ACCEPTED_VERIFICATION_TIMEOUT;
            let base = self
                .ensure_fresh_accepted_until(
                    base,
                    control.clone(),
                    deadline,
                    Some(Arc::clone(&query_slot)),
                )
                .await?;
            let candidate = self
                .ensure_fresh_accepted_until(
                    candidate,
                    control.clone(),
                    deadline,
                    Some(Arc::clone(&query_slot)),
                )
                .await?;
            Ok::<_, AppError>((
                base,
                candidate,
                current_revision,
                query_slot,
                control,
                deadline,
            ))
        }
        .await;
        for handle in ordinary.iter().flatten() {
            handle.admit()?;
        }
        result.map(|(base, candidate, revision, slot, control, deadline)| {
            (base, candidate, revision, slot, control, deadline, ordinary)
        })
    }

    async fn compare_source_records(
        &self,
        base: StoredAcceptedResultV2,
        candidate: StoredAcceptedResultV2,
        control: OperationControl,
        query_slot: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<(StoredAcceptedResultV2, SolutionComparisonV2), AppError> {
        let registry = Arc::clone(&self.pack_registry);
        tokio::task::spawn_blocking(move || {
            let _query_slot = query_slot;
            let comparison = compare_stored_solutions(&base, &candidate, &registry, &control)?;
            Ok((candidate, comparison))
        })
        .await
        .map_err(join_error)?
    }

    async fn render_repair_sources(
        &self,
        base: Box<StoredAcceptedResultV2>,
        current: Box<StoredAcceptedResultV2>,
        control: OperationControl,
        query_slot: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<
        (
            ScenarioDocument,
            EvidenceRenderRequestV2,
            eutheto_domain_ir::EvidenceRenderResultV1,
        ),
        AppError,
    > {
        let registry = Arc::clone(&self.pack_registry);
        tokio::task::spawn_blocking(move || {
            let _query_slot = query_slot;
            let comparison = compare_stored_solutions(&base, &current, &registry, &control)?;
            let request = explanation_render_request(ExplanationEvidencePayloadV2::Repair {
                repair: Box::new(RepairEvidenceV2 {
                    comparison,
                    causality: RepairCausalityV1::NotEstablished,
                }),
            })?;
            let pack = available_pack(&current.document, &registry)
                .ok_or_else(|| unsupported_project_pack(&current.document.domain_pack))?;
            let rendered = pack
                .render_repair_evidence(
                    &base.document,
                    &base.portable,
                    &current.document,
                    &current.portable,
                    &request,
                    &control,
                )
                .map_err(|error| {
                    domain_interruption(&error).unwrap_or_else(|| explanation_render_error(&error))
                })?;
            control.check().map_err(operation_interrupted)?;
            Ok((current.document, request, rendered))
        })
        .await
        .map_err(join_error)?
    }

    async fn query_solution_explain(
        &self,
        request: SolutionExplainRequestV1,
        source_revision: Revision,
        control: OperationControl,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        request
            .request
            .validate()
            .map_err(|_| solution_reference_error("/request"))?;
        let scenario_id = request.scenario_id;
        let ordinary = self.ordinary_explanation_handles(&request)?;
        let native_scope_admitted =
            Self::ordinary_explanation_scope_admitted(&request, source_revision, &ordinary)?;
        let counterfactual_reference = match &request.request.subject {
            ExplanationRequestSubjectV1::Counterfactual { job_id, base } => {
                Some((*job_id, base.clone()))
            }
            _ => None,
        };
        if let Some((job_id, base)) = &counterfactual_reference {
            self.counterfactual
                .guard_job_explanation_publication(scenario_id, source_revision, *job_id, base)
                .await?;
        }
        if native_scope_admitted {
            for handle in ordinary.iter().flatten() {
                handle.admit()?;
            }
        }
        let result = async {
            let control = match control {
                OperationControl::Cancellation(cancellation) => {
                    let limit = eutheto_types::DurationMillis::new(30_000)
                        .map_err(|_| resource_limit_error())?;
                    let budget = eutheto_types::ParentSolveBudget::new(
                        limit,
                        Arc::clone(&self.monotonic_clock),
                        cancellation,
                    )
                    .map_err(|_| {
                        protocol_error(
                            "operation.budget_invalid",
                            "The operation deadline could not be represented safely.",
                            false,
                        )
                    })?;
                    OperationControl::Solve(budget.phase_view())
                }
                OperationControl::Solve(budget) => {
                    OperationControl::Solve(budget.capped_phase_view(ACCEPTED_VERIFICATION_TIMEOUT))
                }
            };
            control.check().map_err(operation_interrupted)?;
            let remaining = match &control {
                OperationControl::Cancellation(_) => ACCEPTED_VERIFICATION_TIMEOUT,
                OperationControl::Solve(budget) => budget
                    .remaining_duration()
                    .min(ACCEPTED_VERIFICATION_TIMEOUT),
            };
            let deadline = tokio::time::Instant::now() + remaining;
            let mut guard = CancelOnDrop::new(control.cancellation().clone());
            let result = self
                .query_solution_explain_controlled(
                    request,
                    source_revision,
                    control,
                    deadline,
                    counterfactual_reference.as_ref(),
                    &ordinary,
                )
                .await;
            guard.disarm();
            result
        }
        .await;
        if let Some((job_id, base)) = &counterfactual_reference {
            self.counterfactual
                .guard_job_explanation_publication(scenario_id, source_revision, *job_id, base)
                .await?;
        }
        if native_scope_admitted {
            for handle in ordinary.iter().flatten() {
                handle.admit()?;
            }
        }
        result
    }

    #[allow(clippy::too_many_lines)]
    async fn capture_explanation(
        &self,
        request: &SolutionExplainRequestV1,
        source_revision: Revision,
        control: &OperationControl,
        ordinary: &[Option<OrdinaryPublicationHandle>],
    ) -> Result<CapturedExplanation, AppError> {
        control.check().map_err(operation_interrupted)?;
        let captured = {
            let mutation = self.scenario_lock(request.scenario_id).await;
            let _guard = mutation.lock().await;
            match &request.request.subject {
                ExplanationRequestSubjectV1::Validation { issue_id } => {
                    let Some(issue_id) = issue_id else {
                        return Err(explanation_unavailable("validation"));
                    };
                    control.check().map_err(operation_interrupted)?;
                    let project = self
                        .store
                        .get_project(request.scenario_id)
                        .await
                        .map_err(store_error)?;
                    ensure_supported_document(&project.document, &self.pack_registry)
                        .map_err(store_error)?;
                    control.check().map_err(operation_interrupted)?;
                    CapturedExplanation::Validation {
                        document: Box::new(project.document),
                        revision: project.summary.revision,
                        issue_id: issue_id.clone(),
                    }
                }
                ExplanationRequestSubjectV1::Infeasibility {
                    solve_run_id,
                    run_manifest_checksum,
                    conflict_id,
                } => {
                    control.check().map_err(operation_interrupted)?;
                    let retained = self
                        .store
                        .load_run_infeasibility_source(request.scenario_id, *solve_run_id)
                        .await
                        .map_err(store_error)?
                        .ok_or(AppError::NotFound(ResourceRef::SolveRun(*solve_run_id)))?;
                    if &retained.manifest.checksum != run_manifest_checksum {
                        return Err(solution_reference_error("/request/subject"));
                    }
                    crate::native_observations::validate_fingerprint(
                        &retained.input,
                        &retained.manifest,
                    )
                    .map_err(|_| solution_contract_error())?;
                    if let Some(conflict_id) = conflict_id {
                        let evidence = retained
                            .evidence
                            .as_ref()
                            .ok_or_else(|| explanation_unavailable("infeasibility"))?;
                        let expected = eutheto_domain_ir::DomainEvidenceId::new(format!(
                            "infeasibility.{}",
                            evidence.checksum
                        ))
                        .map_err(|_| solution_contract_error())?;
                        if conflict_id != &expected {
                            return Err(solution_reference_error("/request/subject/conflictId"));
                        }
                    }
                    CapturedExplanation::Infeasibility(Box::new(retained))
                }
                ExplanationRequestSubjectV1::Assignment {
                    result,
                    assignment_id,
                } => CapturedExplanation::Assignment {
                    stored: Box::new(
                        self.load_referenced_solution(request.scenario_id, result)
                            .await?,
                    ),
                    assignment_id: assignment_id.clone(),
                },
                ExplanationRequestSubjectV1::Counterfactual { job_id, base } => {
                    self.counterfactual
                        .guard_job_explanation_publication(
                            request.scenario_id,
                            source_revision,
                            *job_id,
                            base,
                        )
                        .await?;
                    let completion = self
                        .store
                        .load_counterfactual_completion_for_evidence(*job_id)
                        .await;
                    self.counterfactual
                        .guard_job_explanation_publication(
                            request.scenario_id,
                            source_revision,
                            *job_id,
                            base,
                        )
                        .await?;
                    let (job, stored, alternative) = completion.map_err(store_error)?;
                    if job.request.semantics.scenario_id != request.scenario_id
                        || job.request.semantics.base != *base
                    {
                        return Err(solution_reference_error("/request/subject"));
                    }
                    let result = job
                        .result
                        .ok_or_else(|| explanation_unavailable("counterfactual"))?;
                    crate::native_observations::validate_counterfactual_fingerprints(&result)
                        .map_err(|_| solution_contract_error())?;
                    let stored = stored.ok_or_else(solution_verification_mismatch)?;
                    let alternative = alternative.map(Box::new);
                    CapturedExplanation::Counterfactual {
                        stored: Box::new(stored),
                        result: Box::new(result),
                        alternative,
                    }
                }
                ExplanationRequestSubjectV1::SolutionDifference { left, right } => {
                    let candidate = self
                        .load_referenced_solution(request.scenario_id, right)
                        .await?;
                    if solution_revision(&candidate)? != source_revision {
                        return Err(solution_reference_error("/sourceRevision"));
                    }
                    for handle in ordinary.iter().flatten() {
                        handle.admit()?;
                    }
                    CapturedExplanation::SolutionDifference {
                        base: Box::new(
                            self.load_referenced_solution(request.scenario_id, left)
                                .await?,
                        ),
                        candidate: Box::new(candidate),
                    }
                }
                ExplanationRequestSubjectV1::Repair { current, base } => {
                    let current = self
                        .load_referenced_solution(request.scenario_id, current)
                        .await?;
                    if solution_revision(&current)? != source_revision {
                        return Err(solution_reference_error("/sourceRevision"));
                    }
                    for handle in ordinary.iter().flatten() {
                        handle.admit()?;
                    }
                    CapturedExplanation::Repair {
                        current: Box::new(current),
                        base: Box::new(
                            self.load_referenced_solution(request.scenario_id, base)
                                .await?,
                        ),
                    }
                }
                ExplanationRequestSubjectV1::OptimalityStatus {
                    solve_run_id,
                    run_manifest_checksum,
                    result,
                } => {
                    let retained = self
                        .store
                        .load_solve_run(request.scenario_id, *solve_run_id)
                        .await
                        .map_err(store_error)?
                        .ok_or(AppError::NotFound(ResourceRef::SolveRun(*solve_run_id)))?;
                    let run_manifest = retained
                        .manifest
                        .ok_or_else(|| explanation_unavailable("optimality_status"))?;
                    if run_manifest.run_id != *solve_run_id
                        || run_manifest.checksum.as_str() != run_manifest_checksum
                        || run_manifest.run_input_checksum.as_str()
                            != retained.input.checksum.as_str()
                    {
                        return Err(solution_reference_error("/request/subject"));
                    }
                    let loaded = self
                        .store
                        .load_solve_input(*solve_run_id)
                        .await
                        .map_err(store_error)?;
                    if loaded.input != retained.input
                        || loaded.input.scenario_id != request.scenario_id
                    {
                        return Err(solution_contract_error());
                    }
                    crate::native_observations::validate_fingerprint(&loaded.input, &run_manifest)
                        .map_err(|_| solution_contract_error())?;
                    let (stored, result) = match (&run_manifest.outcome, result) {
                        (
                            eutheto_domain_ir::RunTerminalOutcomeV1::Accepted {
                                solution_id,
                                accepted_result_checksum,
                                ..
                            },
                            Some(result),
                        ) if solution_id == &result.solution_id
                            && accepted_result_checksum == &result.result_checksum =>
                        {
                            let stored = self
                                .load_referenced_solution(request.scenario_id, result)
                                .await?;
                            if stored.portable.run_input != loaded.input
                                || stored.portable.run_manifest != run_manifest
                                || stored.document != loaded.document
                                || stored.portable.accepted_result.solution.solution_id
                                    != *solution_id
                                || stored.portable.accepted_result.checksum.as_str()
                                    != accepted_result_checksum
                            {
                                return Err(solution_verification_mismatch());
                            }
                            (Some(Box::new(stored)), Some(result.clone()))
                        }
                        (eutheto_domain_ir::RunTerminalOutcomeV1::Accepted { .. }, _)
                        | (_, Some(_)) => {
                            return Err(solution_reference_error("/request/subject"));
                        }
                        (_, None) => (None, None),
                    };
                    CapturedExplanation::OptimalityStatus(Box::new(CapturedOptimalityStatus {
                        document: loaded.document,
                        run_input: loaded.input,
                        run_manifest,
                        stored,
                        result,
                    }))
                }
            }
        };
        Ok(captured)
    }

    // Keeping the seven typed subjects together makes the evidence/reference binding auditable.
    #[allow(clippy::too_many_lines)]
    async fn query_solution_explain_controlled(
        &self,
        request: SolutionExplainRequestV1,
        source_revision: Revision,
        control: OperationControl,
        deadline: tokio::time::Instant,
        counterfactual_reference: Option<&(CounterfactualJobId, AcceptedResultRefV1)>,
        ordinary: &[Option<OrdinaryPublicationHandle>; 2],
    ) -> Result<AppQueryResult, AppError> {
        let mut source_admitted = false;
        let run_reference = match &request.request.subject {
            ExplanationRequestSubjectV1::Infeasibility { solve_run_id, .. }
            | ExplanationRequestSubjectV1::OptimalityStatus { solve_run_id, .. } => {
                Some(*solve_run_id)
            }
            _ => None,
        };
        let result = async {
            let scenario_id = request.scenario_id;
            let cancellation = control.cancellation().clone();
            let query_slot =
                Self::acquire_accepted_slot(&self.accepted_query_slots, &cancellation, deadline)
                    .await;
            if let Some((job_id, base)) = counterfactual_reference {
                self.counterfactual
                    .guard_job_explanation_publication(scenario_id, source_revision, *job_id, base)
                    .await?;
            }
            if let Some(run_id) = run_reference {
                self.counterfactual
                    .guard_run_publication(scenario_id, run_id)
                    .await?;
            }
            let query_slot = Arc::new(query_slot?);
            let app = self.clone();
            let capture_control = control.clone();
            let capture_slot = Arc::clone(&query_slot);
            let capture_ordinary = (*ordinary).clone();
            let mut capture = tokio::spawn(async move {
                let _query_slot = capture_slot;
                let captured = app
                    .capture_explanation(
                        &request,
                        source_revision,
                        &capture_control,
                        &capture_ordinary,
                    )
                    .await?;
                Ok::<_, AppError>((request, captured))
            });
            let Ok(captured) = tokio::time::timeout_at(deadline, &mut capture).await else {
                cancellation.cancel();
                // The native operation retains admission while this source owner drains.
                let _ = capture.await;
                if let Some((job_id, base)) = counterfactual_reference {
                    self.counterfactual
                        .guard_job_explanation_publication(
                            scenario_id,
                            source_revision,
                            *job_id,
                            base,
                        )
                        .await?;
                }
                if let Some(run_id) = run_reference {
                    self.counterfactual
                        .guard_run_publication(scenario_id, run_id)
                        .await?;
                }
                return Err(operation_interrupted(
                    OperationInterruption::DeadlineExceeded,
                ));
            };
            if let Some((job_id, base)) = counterfactual_reference {
                self.counterfactual
                    .guard_job_explanation_publication(scenario_id, source_revision, *job_id, base)
                    .await?;
            }
            if let Some(run_id) = run_reference {
                self.counterfactual
                    .guard_run_publication(scenario_id, run_id)
                    .await?;
            }
            let (request, captured) = captured.map_err(join_error)??;
            let captured_revision = match &captured {
                CapturedExplanation::Validation { revision, .. } => *revision,
                CapturedExplanation::Infeasibility(source) => {
                    Revision::try_new(source.input.scenario_revision)
                        .map_err(|_| solution_contract_error())?
                }
                CapturedExplanation::Assignment { stored, .. }
                | CapturedExplanation::Counterfactual { stored, .. } => solution_revision(stored)?,
                CapturedExplanation::SolutionDifference { candidate, .. } => {
                    solution_revision(candidate)?
                }
                CapturedExplanation::Repair { current, .. } => solution_revision(current)?,
                CapturedExplanation::OptimalityStatus(source) => {
                    Revision::try_new(source.run_input.scenario_revision)
                        .map_err(|_| solution_contract_error())?
                }
            };
            if captured_revision != source_revision {
                return Err(solution_reference_error("/sourceRevision"));
            }
            let inputs = match &captured {
                CapturedExplanation::Validation { .. } => [None, None],
                CapturedExplanation::Infeasibility(source) => [Some(&source.input), None],
                CapturedExplanation::Assignment { stored, .. }
                | CapturedExplanation::Counterfactual { stored, .. } => {
                    [Some(&stored.portable.run_input), None]
                }
                CapturedExplanation::SolutionDifference { base, candidate } => [
                    Some(&base.portable.run_input),
                    Some(&candidate.portable.run_input),
                ],
                CapturedExplanation::Repair { current, base } => [
                    Some(&current.portable.run_input),
                    Some(&base.portable.run_input),
                ],
                CapturedExplanation::OptimalityStatus(source) => [Some(&source.run_input), None],
            };
            for (handle, input) in ordinary.iter().zip(inputs) {
                if let (Some(handle), Some(input)) = (handle, input) {
                    handle.validate_input(input)?;
                }
            }
            source_admitted = true;
            for handle in ordinary.iter().flatten() {
                handle.admit()?;
            }
            control.check().map_err(operation_interrupted)?;
            self.guard_explanation_publication(&request, source_revision)
                .await?;
            let mut infeasibility_context = None;
            let mut rendered_repair = None;
            let (document, revisions, render_request, objective_context) = match captured {
                CapturedExplanation::Validation {
                    document,
                    revision,
                    issue_id,
                } => {
                    let registry = Arc::clone(&self.pack_registry);
                    let validation_control = control.clone();
                    let validation_slot = Arc::clone(&query_slot);
                    let expected_issue_id = issue_id.clone();
                    let (document, selected_issue) = tokio::task::spawn_blocking(move || {
                        let _query_slot = validation_slot;
                        let control = validation_control;
                        control.check().map_err(operation_interrupted)?;
                        let pack = available_pack(&document, &registry)
                            .ok_or_else(|| unsupported_project_pack(&document.domain_pack))?;
                        let validated =
                            pack.validate_full(&document, &control).map_err(|error| {
                                domain_interruption(&error)
                                    .unwrap_or_else(|| explanation_render_error(&error))
                            })?;
                        control.check().map_err(operation_interrupted)?;
                        let mut selected_issue = None;
                        for issue in validated.issues {
                            let actual_issue_id = validation_issue_identity(&issue)
                                .map_err(|_| resource_limit_error())?;
                            if actual_issue_id == expected_issue_id
                                && selected_issue.replace(issue).is_some()
                            {
                                return Err(explanation_unavailable("validation"));
                            }
                        }
                        let selected_issue =
                            selected_issue.ok_or_else(|| explanation_unavailable("validation"))?;
                        Ok::<_, AppError>((document, selected_issue))
                    })
                    .await
                    .map_err(join_error)??;
                    let document = *document;
                    control.check().map_err(operation_interrupted)?;
                    let (current_document, current_revision) =
                        self.current_scenario_snapshot(request.scenario_id).await?;
                    control.check().map_err(operation_interrupted)?;
                    if current_revision != revision || current_document != document {
                        return Err(explanation_unavailable("validation"));
                    }
                    let issue = validation_issue_evidence(&selected_issue, &issue_id)?;
                    (
                        document,
                        vec![revision],
                        explanation_render_request(ExplanationEvidencePayloadV2::Validation {
                            issue,
                        })?,
                        None,
                    )
                }
                CapturedExplanation::Infeasibility(source) => {
                    let source = *source;
                    let infeasibility = if let Some(evidence) = source.evidence {
                        let conflict_id = eutheto_domain_ir::DomainEvidenceId::new(format!(
                            "infeasibility.{}",
                            evidence.checksum
                        ))
                        .map_err(|_| solution_contract_error())?;
                        infeasibility_context = Some(InfeasibilityExplanationContextV1 {
                            run_id: evidence.run_id,
                            source_revision,
                            run_manifest_checksum: evidence.run_manifest_checksum,
                            evidence_checksum: evidence.checksum,
                            conflict_id,
                        });
                        evidence.infeasibility
                    } else {
                        eutheto_domain_ir::InfeasibilityEvidenceV1::Unavailable {
                            reason: eutheto_domain_ir::ConflictUnavailableReason::NotRecorded,
                        }
                    };
                    (
                        source.document,
                        vec![source_revision],
                        explanation_render_request(ExplanationEvidencePayloadV2::Infeasibility {
                            infeasibility,
                            condition: source.condition,
                        })?,
                        source.input.objective_context,
                    )
                }
                CapturedExplanation::Assignment {
                    stored,
                    assignment_id,
                } => {
                    let stored = self
                        .ensure_fresh_accepted_until(
                            *stored,
                            control.clone(),
                            deadline,
                            Some(Arc::clone(&query_slot)),
                        )
                        .await?;
                    let revision = solution_revision(&stored)?;
                    let assignment =
                        assignment_evidence(&stored.portable.accepted_result, &assignment_id)?;
                    (
                        stored.document,
                        vec![revision],
                        explanation_render_request(ExplanationEvidencePayloadV2::Assignment {
                            assignment,
                        })?,
                        stored.portable.run_input.objective_context,
                    )
                }
                CapturedExplanation::Counterfactual {
                    stored,
                    result,
                    alternative,
                } => {
                    let mut stored = stored;
                    let fresh = async {
                        *stored = self
                            .ensure_fresh_accepted_until(
                                *stored,
                                control.clone(),
                                deadline,
                                Some(Arc::clone(&query_slot)),
                            )
                            .await?;
                        Ok::<_, AppError>(stored)
                    }
                    .await;
                    self.guard_explanation_publication(&request, source_revision)
                        .await?;
                    let stored = fresh?;
                    if let Some(alternative) = alternative {
                        let alternative = self
                            .ensure_fresh_accepted_until(
                                *alternative,
                                control.clone(),
                                deadline,
                                Some(Arc::clone(&query_slot)),
                            )
                            .await
                            .map(|_| ());
                        self.guard_explanation_publication(&request, source_revision)
                            .await?;
                        alternative?;
                    }
                    let revision = solution_revision(&stored)?;
                    (
                        stored.document,
                        vec![revision],
                        explanation_render_request(ExplanationEvidencePayloadV2::Counterfactual {
                            result,
                        })?,
                        stored.portable.run_input.objective_context,
                    )
                }
                CapturedExplanation::SolutionDifference { base, candidate } => {
                    // Reuse captured boxes so revalidated records do not inflate the read future.
                    let mut base = base;
                    *base = self
                        .ensure_fresh_accepted_until(
                            *base,
                            control.clone(),
                            deadline,
                            Some(Arc::clone(&query_slot)),
                        )
                        .await?;
                    let mut candidate = candidate;
                    *candidate = self
                        .ensure_fresh_accepted_until(
                            *candidate,
                            control.clone(),
                            deadline,
                            Some(Arc::clone(&query_slot)),
                        )
                        .await?;
                    let base_revision = solution_revision(&base)?;
                    let candidate_revision = solution_revision(&candidate)?;
                    let (candidate, comparison) = self
                        .compare_source_records(
                            *base,
                            *candidate,
                            control.clone(),
                            Arc::clone(&query_slot),
                        )
                        .await?;
                    (
                        candidate.document,
                        vec![base_revision, candidate_revision],
                        explanation_render_request(
                            ExplanationEvidencePayloadV2::SolutionDifference {
                                comparison: Box::new(comparison),
                            },
                        )?,
                        candidate.portable.run_input.objective_context,
                    )
                }
                CapturedExplanation::Repair { current, base } => {
                    let mut current = current;
                    *current = self
                        .ensure_fresh_accepted_until(
                            *current,
                            control.clone(),
                            deadline,
                            Some(Arc::clone(&query_slot)),
                        )
                        .await?;
                    let mut base = base;
                    *base = self
                        .ensure_fresh_accepted_until(
                            *base,
                            control.clone(),
                            deadline,
                            Some(Arc::clone(&query_slot)),
                        )
                        .await?;
                    let current_revision = solution_revision(&current)?;
                    let base_revision = solution_revision(&base)?;
                    let (document, render_request, rendered) = self
                        .render_repair_sources(
                            base,
                            current,
                            control.clone(),
                            Arc::clone(&query_slot),
                        )
                        .await?;
                    rendered_repair = Some(rendered);
                    (
                        document,
                        vec![current_revision, base_revision],
                        render_request,
                        None,
                    )
                }
                CapturedExplanation::OptimalityStatus(captured) => {
                    let mut captured = captured;
                    if let Some(stored) = captured.stored.take() {
                        self.ensure_fresh_accepted_until(
                            *stored,
                            control.clone(),
                            deadline,
                            Some(Arc::clone(&query_slot)),
                        )
                        .await?;
                    }
                    let CapturedOptimalityStatus {
                        document,
                        run_input,
                        run_manifest,
                        result,
                        ..
                    } = *captured;
                    let revision = Revision::try_new(run_input.scenario_revision)
                        .map_err(|_| solution_contract_error())?;
                    (
                        document,
                        vec![revision],
                        explanation_render_request(
                            ExplanationEvidencePayloadV2::OptimalityStatus {
                                status: Box::new(OptimalityStatusEvidence {
                                    schema_version: Some(2),
                                    run_input,
                                    run_manifest,
                                    result,
                                }),
                            },
                        )?,
                        None,
                    )
                }
            };
            let rendered = if let Some(rendered) = rendered_repair {
                Ok((document, render_request, rendered))
            } else {
                let registry = Arc::clone(&self.pack_registry);
                let render_control = control.clone();
                let render_slot = Arc::clone(&query_slot);
                tokio::task::spawn_blocking(move || {
                    let _query_slot = render_slot;
                    render_control.check().map_err(operation_interrupted)?;
                    let pack = available_pack(&document, &registry)
                        .ok_or_else(|| unsupported_project_pack(&document.domain_pack))?;
                    let rendered = pack
                        .render_evidence(
                            &document,
                            &render_request,
                            match &render_request.evidence.evidence {
                                ExplanationEvidencePayloadV2::OptimalityStatus { status } => {
                                    status.run_input.objective_context.as_ref()
                                }
                                _ => objective_context.as_ref(),
                            },
                            &render_control,
                        )
                        .map_err(|error| {
                            domain_interruption(&error)
                                .unwrap_or_else(|| explanation_render_error(&error))
                        })?;
                    render_control.check().map_err(operation_interrupted)?;
                    Ok::<_, AppError>((document, render_request, rendered))
                })
                .await
                .map_err(join_error)
                .and_then(std::convert::identity)
            };
            self.guard_explanation_publication(&request, source_revision)
                .await?;
            let (document, render_request, rendered) = rendered?;
            control.check().map_err(operation_interrupted)?;
            let explanation = ExplanationResultV2::new(render_request.evidence, rendered)
                .map_err(|_| solution_contract_error())?;
            let current_revision = if matches!(
                &request.request.subject,
                ExplanationRequestSubjectV1::Validation { issue_id: Some(_) }
            ) {
                control.check().map_err(operation_interrupted)?;
                let (current_document, current_revision) =
                    self.current_scenario_snapshot(request.scenario_id).await?;
                let source_revision = revisions
                    .first()
                    .copied()
                    .ok_or_else(solution_contract_error)?;
                if current_revision != source_revision || current_document != document {
                    return Err(explanation_unavailable("validation"));
                }
                control.check().map_err(operation_interrupted)?;
                current_revision
            } else {
                let current_revision = self.current_scenario_revision(request.scenario_id).await;
                self.guard_explanation_publication(&request, source_revision)
                    .await?;
                current_revision?
            };
            self.guard_explanation_publication(&request, source_revision)
                .await?;
            control.check().map_err(operation_interrupted)?;
            Ok(AppQueryResult::SolutionExplanation(Box::new(
                SolutionExplanationDtoV2 {
                    schema_version: eutheto_domain_ir::EXPLANATION_RESULT_V2_SCHEMA_VERSION,
                    scenario_id,
                    current_revision,
                    scenario_revisions: revisions,
                    explanation,
                    infeasibility_context,
                },
            )))
        }
        .await;
        if source_admitted {
            for handle in ordinary.iter().flatten() {
                handle.admit()?;
            }
        }
        result
    }

    async fn guard_explanation_publication(
        &self,
        request: &SolutionExplainRequestV1,
        source_revision: Revision,
    ) -> Result<(), AppError> {
        match &request.request.subject {
            ExplanationRequestSubjectV1::Counterfactual { job_id, base } => {
                self.counterfactual
                    .guard_job_explanation_publication(
                        request.scenario_id,
                        source_revision,
                        *job_id,
                        base,
                    )
                    .await
            }
            ExplanationRequestSubjectV1::Infeasibility { solve_run_id, .. }
            | ExplanationRequestSubjectV1::OptimalityStatus { solve_run_id, .. } => {
                self.counterfactual
                    .guard_run_publication(request.scenario_id, *solve_run_id)
                    .await
            }
            _ => Ok(()),
        }
    }

    async fn capture_accepted_source(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<
        (
            AcceptedQuerySource,
            Revision,
            tokio::sync::OwnedSemaphorePermit,
            Option<OrdinaryPublicationHandle>,
        ),
        AppError,
    > {
        let ordinary = self.ordinary_solution_handle(scenario_id, solution_id)?;
        let slot =
            Self::acquire_accepted_slot(&self.accepted_query_slots, cancellation, deadline).await;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        let slot = slot?;
        let app = self.clone();
        let capture_cancellation = cancellation.clone();
        let captured_ordinary = ordinary.clone();
        let capture = tokio::spawn(async move {
            let control = OperationControl::Cancellation(capture_cancellation.clone());
            control.check().map_err(operation_interrupted)?;
            let mutation = tokio::select! {
                biased;
                () = wait_for_cancellation(&capture_cancellation) =>
                    return Err(operation_interrupted(OperationInterruption::Cancelled)),
                result = tokio::time::timeout_at(deadline, app.scenario_lock(scenario_id)) =>
                    result.map_err(|_| operation_interrupted(OperationInterruption::DeadlineExceeded))?,
            };
            let _guard = tokio::select! {
                biased;
                () = wait_for_cancellation(&capture_cancellation) =>
                    return Err(operation_interrupted(OperationInterruption::Cancelled)),
                result = tokio::time::timeout_at(deadline, mutation.lock()) =>
                    result.map_err(|_| operation_interrupted(OperationInterruption::DeadlineExceeded))?,
            };
            control.check().map_err(operation_interrupted)?;
            let terminal = app.load_solution(scenario_id, solution_id).await;
            captured_ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
            let source = match terminal {
                Ok(stored) => AcceptedQuerySource::Terminal(stored),
                Err(AppError::NotFound(ResourceRef::Solution(_))) => {
                    control.check().map_err(operation_interrupted)?;
                    let (document, checkpoint) = app
                        .store
                        .load_verified_checkpoint_for_solution(solution_id)
                        .await
                        .map_err(store_error)?
                        .ok_or(AppError::NotFound(ResourceRef::Solution(solution_id)))?;
                    if document.scenario_id != scenario_id
                        || checkpoint.run_input.scenario_id != scenario_id
                    {
                        return Err(AppError::NotFound(ResourceRef::Solution(solution_id)));
                    }
                    AcceptedQuerySource::Checkpoint(document, checkpoint)
                }
                Err(error) => return Err(error),
            };
            if let Some(handle) = &captured_ordinary {
                let input = match &source {
                    AcceptedQuerySource::Terminal(stored) => &stored.portable.run_input,
                    AcceptedQuerySource::Checkpoint(_, checkpoint) => &checkpoint.run_input,
                };
                handle.admit_input(input)?;
            }
            let run_id = source.run_id();
            app.counterfactual
                .guard_run_publication(scenario_id, run_id)
                .await?;
            control.check().map_err(operation_interrupted)?;
            let current_revision = app.current_scenario_revision(scenario_id).await;
            app.counterfactual
                .guard_run_publication(scenario_id, run_id)
                .await?;
            let current_revision = current_revision?;
            control.check().map_err(operation_interrupted)?;
            captured_ordinary
                .as_ref()
                .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
            Ok::<_, AppError>((source, current_revision, slot, captured_ordinary))
        });
        let result = tokio::select! {
            biased;
            () = wait_for_cancellation(cancellation) =>
                Err(operation_interrupted(OperationInterruption::Cancelled)),
            result = tokio::time::timeout_at(deadline, capture) =>
                result
                    .map_err(|_| operation_interrupted(OperationInterruption::DeadlineExceeded))
                    .and_then(|captured| captured.map_err(join_error))
                    .and_then(std::convert::identity),
        };
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        result
    }

    async fn current_scenario_revision(
        &self,
        scenario_id: ScenarioId,
    ) -> Result<Revision, AppError> {
        self.store
            .get_project(scenario_id)
            .await
            .map(|project| project.summary.revision)
            .map_err(store_error)
    }
    async fn current_scenario_snapshot(
        &self,
        scenario_id: ScenarioId,
    ) -> Result<(ScenarioDocument, Revision), AppError> {
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let project = self
            .store
            .get_project(scenario_id)
            .await
            .map_err(store_error)?;
        Ok((project.document, project.summary.revision))
    }

    async fn load_solution(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
    ) -> Result<StoredAcceptedResultV2, AppError> {
        let stored = self
            .store
            .load_accepted_result(solution_id)
            .await
            .map_err(store_error)?;
        ensure_solution_scenario(&stored, scenario_id)?;
        Ok(stored)
    }

    async fn load_referenced_solution(
        &self,
        scenario_id: ScenarioId,
        reference: &AcceptedResultRefV1,
    ) -> Result<StoredAcceptedResultV2, AppError> {
        reference
            .validate()
            .map_err(|_| solution_reference_error("/request/subject"))?;
        let stored = self
            .load_solution(scenario_id, reference.solution_id)
            .await?;
        if stored.portable.accepted_result.checksum != reference.result_checksum {
            return Err(solution_reference_error("/request/subject"));
        }
        self.counterfactual
            .guard_run_publication(scenario_id, stored.portable.run_input.run_id)
            .await?;
        Ok(stored)
    }
    async fn verify_accepted_off_lock(
        &self,
        stored: StoredAcceptedResultV2,
        control: OperationControl,
        permit: tokio::sync::OwnedSemaphorePermit,
        deadline: tokio::time::Instant,
        query_slot: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
    ) -> Result<(StoredAcceptedResultV2, VerificationReport), AppError> {
        let scenario_id = stored.portable.run_input.scenario_id;
        let run_id = stored.portable.run_input.run_id;
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        let cancellation = control.cancellation().clone();
        let mut guard = CancelOnDrop::new(cancellation.clone());
        control.check().map_err(operation_interrupted)?;
        let registry = Arc::clone(&self.pack_registry);
        let retain_admission = query_slot.is_some();
        let mut worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _query_slot = query_slot;
            control.check().map_err(operation_interrupted)?;
            let report = reverify_accepted_solution(&stored, &registry, &control)?;
            control.check().map_err(operation_interrupted)?;
            Ok((stored, report))
        });
        let result = tokio::select! {
            joined = tokio::time::timeout_at(deadline, &mut worker) => if let Ok(result) = joined {
                result.map_err(join_error).and_then(|result| result)
            } else {
                cancellation.cancel();
                if retain_admission {
                    let _ = worker.await;
                }
                Err(operation_interrupted(OperationInterruption::DeadlineExceeded))
            },
            () = wait_for_cancellation(&cancellation) => {
                if retain_admission {
                    let _ = worker.await;
                }
                Err(operation_interrupted(OperationInterruption::Cancelled))
            }
        };
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        guard.disarm();
        result
    }

    async fn verify_checkpoint_off_lock(
        &self,
        document: ScenarioDocument,
        checkpoint: VerifiedCheckpoint,
        cancellation: CancellationToken,
        permit: tokio::sync::OwnedSemaphorePermit,
        deadline: tokio::time::Instant,
    ) -> Result<(VerifiedCheckpoint, VerificationReport), AppError> {
        let scenario_id = checkpoint.run_input.scenario_id;
        let run_id = checkpoint.run_input.run_id;
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        let verification_cancellation = cancellation.child();
        let _cancel_on_drop = CancelOnDrop::new(verification_cancellation.clone());
        let control = OperationControl::Cancellation(verification_cancellation.clone());
        control.check().map_err(operation_interrupted)?;
        let registry = Arc::clone(&self.pack_registry);
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            control.check().map_err(operation_interrupted)?;
            let report = reverify_checkpoint_solution(&document, &checkpoint, &registry, &control)?;
            control.check().map_err(operation_interrupted)?;
            Ok((checkpoint, report))
        });
        let result = tokio::select! {
            joined = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                worker,
            ) => if let Ok(result) = joined {
                result.map_err(join_error).and_then(|result| result)
            } else {
                verification_cancellation.cancel();
                Err(operation_interrupted(OperationInterruption::DeadlineExceeded))
            },
            () = wait_for_cancellation(&cancellation) => {
                verification_cancellation.cancel();
                Err(operation_interrupted(OperationInterruption::Cancelled))
            }
        };
        self.counterfactual
            .guard_run_publication(scenario_id, run_id)
            .await?;
        result
    }

    async fn hash_accepted_off_executor(
        &self,
        stored: StoredAcceptedResultV2,
        permit: tokio::sync::OwnedSemaphorePermit,
        control: OperationControl,
        deadline: tokio::time::Instant,
        query_slot: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
    ) -> Result<
        (
            StoredAcceptedResultV2,
            AcceptedContentHash,
            tokio::sync::OwnedSemaphorePermit,
        ),
        AppError,
    > {
        let cancellation = control.cancellation().clone();
        let mut guard = CancelOnDrop::new(cancellation.clone());
        let retain_admission = query_slot.is_some();
        let mut worker = tokio::task::spawn_blocking(move || {
            let _query_slot = query_slot;
            let key = accepted_content_hash(&stored, &control, deadline)?;
            Ok((stored, key, permit))
        });
        let result = tokio::select! {
            joined = tokio::time::timeout_at(deadline, &mut worker) => if let Ok(result) = joined {
                result.map_err(join_error).and_then(|result| result)
            } else {
                cancellation.cancel();
                if retain_admission {
                    let _ = worker.await;
                }
                Err(operation_interrupted(OperationInterruption::DeadlineExceeded))
            },
            () = wait_for_cancellation(&cancellation) => {
                if retain_admission {
                    let _ = worker.await;
                }
                Err(operation_interrupted(OperationInterruption::Cancelled))
            }
        };
        guard.disarm();
        result
    }
    async fn acquire_verification_slot(
        &self,
        control: &OperationControl,
        deadline: tokio::time::Instant,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
        controlled_wait(
            Self::acquire_accepted_slot(
                &self.accepted_verification.cache.slots,
                control.cancellation(),
                deadline,
            ),
            control,
        )
        .await?
    }

    async fn acquire_accepted_query_slot(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
        Self::acquire_accepted_slot(
            &self.accepted_query_slots,
            cancellation,
            tokio::time::Instant::now() + ACCEPTED_VERIFICATION_TIMEOUT,
        )
        .await
    }

    async fn acquire_accepted_slot(
        slots: &Arc<Semaphore>,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
        loop {
            if cancellation.is_cancelled() {
                return Err(operation_interrupted(OperationInterruption::Cancelled));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(operation_interrupted(
                    OperationInterruption::DeadlineExceeded,
                ));
            }
            match Arc::clone(slots).try_acquire_owned() {
                Ok(permit) => return Ok(permit),
                Err(tokio::sync::TryAcquireError::Closed) => {
                    return Err(solution_contract_error());
                }
                Err(tokio::sync::TryAcquireError::NoPermits) => {
                    tokio::time::sleep(
                        deadline
                            .saturating_duration_since(tokio::time::Instant::now())
                            .min(Duration::from_millis(25)),
                    )
                    .await;
                }
            }
        }
    }

    fn ensure_fresh_accepted(
        &self,
        stored: StoredAcceptedResultV2,
        cancellation: CancellationToken,
    ) -> impl Future<Output = Result<StoredAcceptedResultV2, AppError>> + '_ {
        self.ensure_fresh_accepted_until(
            stored,
            OperationControl::Cancellation(cancellation),
            tokio::time::Instant::now() + ACCEPTED_VERIFICATION_TIMEOUT,
            None,
        )
    }

    fn ensure_fresh_accepted_until(
        &self,
        stored: StoredAcceptedResultV2,
        control: OperationControl,
        deadline: tokio::time::Instant,
        query_slot: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
    ) -> impl Future<Output = Result<StoredAcceptedResultV2, AppError>> + '_ {
        // Keep source-owning verification state from inflating every accepted-query caller.
        Box::pin(async move {
            let scenario_id = stored.portable.run_input.scenario_id;
            let run_id = stored.portable.run_input.run_id;
            self.counterfactual
                .guard_run_publication(scenario_id, run_id)
                .await?;
            control.check().map_err(operation_interrupted)?;
            let permit = self.acquire_verification_slot(&control, deadline).await?;
            // The blocking hasher owns its slot if this read future is dropped mid-hash.
            let hashed = self
                .hash_accepted_off_executor(
                    stored,
                    permit,
                    control.clone(),
                    deadline,
                    query_slot.clone(),
                )
                .await;
            self.counterfactual
                .guard_run_publication(scenario_id, run_id)
                .await?;
            let (stored, key, permit) = hashed?;
            if let Err(interruption) = control.check() {
                drop(permit);
                return Err(operation_interrupted(interruption));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(operation_interrupted(
                    OperationInterruption::DeadlineExceeded,
                ));
            }
            let cached = self
                .accepted_verification
                .cache
                .completed
                .lock()
                .await
                .contains(&key);
            if cached {
                self.counterfactual
                    .guard_run_publication(scenario_id, run_id)
                    .await?;
                drop(permit);
                return Ok(stored);
            }

            let verified = self
                .verify_accepted_off_lock(stored, control.clone(), permit, deadline, query_slot)
                .await;
            self.counterfactual
                .guard_run_publication(scenario_id, run_id)
                .await?;
            let result = match verified {
                Ok((stored, _report)) => {
                    control.check().map_err(operation_interrupted)?;
                    if tokio::time::Instant::now() >= deadline {
                        Err(operation_interrupted(
                            OperationInterruption::DeadlineExceeded,
                        ))
                    } else {
                        Ok(stored)
                    }
                }
                Err(error) => Err(error),
            };
            if result.is_ok() {
                let mut completed = self.accepted_verification.cache.completed.lock().await;
                completed.retain(|entry| entry != &key);
                completed.push_back(key);
                while completed.len() > ACCEPTED_VERIFICATION_CACHE_CAPACITY {
                    completed.pop_front();
                }
            }
            self.counterfactual
                .guard_run_publication(scenario_id, run_id)
                .await?;
            result
        })
    }

    async fn query_preview_import(
        &self,
        bytes: Vec<u8>,
        options: ImportOptions,
        cancellation: &CancellationToken,
    ) -> Result<AppQueryResult, AppError> {
        if options.restore_mode == RestoreMode::ImportScenario {
            self.preview_portable(bytes, options, cancellation).await
        } else {
            Err(validation_error(
                "portable.import_mode_invalid",
                "/restoreMode",
                "Project import requires import-scenario mode.",
            ))
        }
    }

    async fn query_preview_restore(
        &self,
        bytes: Vec<u8>,
        options: ImportOptions,
        cancellation: &CancellationToken,
    ) -> Result<AppQueryResult, AppError> {
        if options.restore_mode == RestoreMode::ImportScenario {
            Err(validation_error(
                "portable.restore_mode_invalid",
                "/restoreMode",
                "Backup restore requires add-backup or replace-library mode.",
            ))
        } else {
            self.preview_portable(bytes, options, cancellation).await
        }
    }

    fn query_domain_pack_metadata(&self, id: PackId) -> Result<AppQueryResult, AppError> {
        let descriptor = self
            .pack_registry
            .descriptors()
            .find(|descriptor| descriptor.id == id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(ResourceRef::Pack(id.clone())))?;
        let catalog = self
            .pack_registry
            .catalog(&id)
            .cloned()
            .ok_or(AppError::NotFound(ResourceRef::Pack(id)))?;
        Ok(AppQueryResult::DomainPack(Box::new(DomainPackMetadata {
            descriptor,
            catalog,
        })))
    }

    fn query_solver_support_matrix(&self) -> AppQueryResult {
        let matrix = self.solver_registry.matrix();
        AppQueryResult::SolverSupportMatrix(SolverSupportMatrixMetadata {
            schema_version: matrix.schema_version(),
            planning_ir_schema_version: matrix.planning_ir_schema_version(),
            features: matrix.features().cloned().collect(),
            production_backend_ids: matrix.production_backend_ids().cloned().collect(),
            backend_columns: matrix.backend_columns().collect(),
        })
    }

    async fn support_preview(&self) -> Result<SupportPreviewDto, AppError> {
        let generated_at = self.clock.now();
        let library = self
            .store
            .library_metadata_snapshot()
            .await
            .map_err(store_error)?;
        let interrupted_recovery_count = u64::try_from(
            self.initialization.recovery.interrupted_solve_run_ids.len(),
        )
        .map_err(|_| {
            protocol_error(
                "support_preview.recovery_count_overflow",
                "The support preview could not represent the startup recovery count.",
                false,
            )
        })?;
        let database = self.paths.database.clone();
        let safety_backups = self.paths.safety_backups.clone();
        let (application_data, safety_backups) = tokio::task::spawn_blocking(move || {
            (
                directory_availability(database.parent()),
                directory_availability(Some(&safety_backups)),
            )
        })
        .await
        .map_err(join_error)?;
        let application = application_metadata();

        Ok(SupportPreviewDto {
            schema_version: SUPPORT_PREVIEW_SCHEMA_VERSION,
            generated_at,
            application: SupportApplicationMetadataDto {
                name: application.name,
                version: application.version,
            },
            schemas: SupportSchemaMetadataDto {
                scenario_format_version: SCENARIO_FORMAT_VERSION,
                storage_schema_version: self.initialization.schema_version,
            },
            library: SupportLibraryMetadataDto {
                revision: library.revision,
                scenario_count: library.scenario_count,
                // Solve is deferred in Phase 02. Store startup also atomically
                // transitions every formerly running row to interrupted before
                // EuthetoApp can be constructed, so active is invariantly zero.
                active_solve_run_count: 0,
                interrupted_recovery_count,
            },
            directories: SupportDirectoryMetadataDto {
                application_data,
                safety_backups,
            },
        })
    }

    async fn create_project(
        &self,
        request_id: RequestId,
        title: String,
        description: String,
        domain_pack: DomainPackRef,
        settings: ScenarioSettings,
    ) -> Result<AppCommandResult, AppError> {
        let pack = creation_pack(&title, &domain_pack, &self.pack_registry)?;
        let library = self
            .store
            .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
            .await
            .map_err(store_error)?;
        let LibraryUuidClosure { occupied, .. } = library_uuid_closure(&library);
        let scenario_id = ScenarioId::from_uuid(allocate_unique_uuid(
            self.ids.as_ref(),
            &occupied,
            &mut BTreeSet::new(),
        )?);
        let now = self.clock.now();
        let shell = ScenarioDocument::new(
            scenario_id,
            domain_pack,
            ScenarioMetadata {
                title,
                description,
                created_at: now,
                updated_at: now,
            },
            settings,
            ScenarioDomain::default(),
            BTreeMap::new(),
        );
        let document = initialize_document(
            &shell,
            pack,
            &OperationControl::Cancellation(self.cancellation.clone()),
        )?;
        let project = self
            .store
            .create_project(NewProject { document })
            .await
            .map_err(store_error)?;
        self.publish_changed_event(
            scenario_id,
            request_id,
            project.summary.revision,
            ChangeSet {
                changes: vec![Change {
                    kind: ChangeKind::Added,
                    path: "/project".to_owned(),
                    before: None,
                    after: Some(Value::Bool(true)),
                }],
            },
        );
        Ok(AppCommandResult::Project(project_metadata(&project)))
    }

    async fn scenario_lock(&self, id: ScenarioId) -> ScenarioMutex {
        let mut locks = self.mutation_locks.lock().await;
        Arc::clone(locks.entry(id).or_insert_with(|| Arc::new(Mutex::new(()))))
    }

    #[allow(clippy::too_many_lines)]
    async fn apply_scenario(
        &self,
        request_id: RequestId,
        envelope: CommandEnvelope,
        truncate_redo: bool,
        cancellation: CancellationToken,
    ) -> Result<CommandResult, AppError> {
        let scenario_id = envelope.scenario_id;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        if cancellation.is_cancelled() {
            return Err(store_error(StoreError::OperationCancelled));
        }
        let initial = self
            .store
            .get_project(scenario_id)
            .await
            .map_err(store_error)?;
        if initial.summary.revision != envelope.expected_revision {
            return Err(AppError::Conflict {
                expected_revision: envelope.expected_revision,
                actual_revision: initial.summary.revision,
            });
        }
        let (captured, custody_stamp) = self
            .store
            .capture_scenario_custody(
                scenario_id,
                envelope.expected_revision,
                initial.document,
                OperationControl::Cancellation(cancellation.clone()),
                None,
            )
            .await
            .map_err(store_error)?;
        let original = captured.document;
        let mutation_cancellation = cancellation.clone();
        let applied_at = self.clock.now();
        let pack_registry = Arc::clone(&self.pack_registry);
        let result = self
            .store
            .execute_command(
                scenario_id,
                envelope.expected_revision,
                if truncate_redo {
                    RedoBranchPolicy::Truncate
                } else {
                    RedoBranchPolicy::Reject
                },
                custody_stamp,
                OperationControl::Cancellation(cancellation),
                move |document| {
                    ensure_supported_document(document, &pack_registry)?;
                    let mut applied = apply_command_with_registry(
                        document,
                        envelope.expected_revision,
                        &envelope,
                        &pack_registry,
                        &mutation_cancellation,
                    )
                    .map_err(|error| command_store_error(&error, Some(&envelope.command)))?;
                    applied.document.metadata.updated_at = applied_at;
                    let command =
                        serde_json::to_value(&envelope.command).map_err(StoreError::Json)?;
                    let inverse = applied
                        .result
                        .inverse
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(StoreError::Json)?;
                    let operation_control =
                        OperationControl::Cancellation(mutation_cancellation.clone());
                    let protected = finalize_base_change(
                        &original,
                        &mut applied.document,
                        &command,
                        inverse.as_ref(),
                        Some(&operation_control),
                    )?;
                    let mut output = applied.result;
                    let (command_type, command, inverse, actor, summary) =
                        if let Some(protected) = protected {
                            output
                                .change_set
                                .changes
                                .extend(protected.change_set.changes);
                            output.validation_delta = validation_delta(
                                &validate(&original, &pack_registry),
                                &validate(&applied.document, &pack_registry),
                            );
                            output.inverse = None;
                            (
                                eutheto_store::NATIVE_PRIVATE_HISTORY_COMMAND_TYPE.to_owned(),
                                protected.command,
                                protected.inverse,
                                private_actor(envelope.actor),
                                "Updated private Workforce selection".to_owned(),
                            )
                        } else {
                            (
                                applied.command_type,
                                command,
                                inverse,
                                envelope.actor,
                                applied.summary,
                            )
                        };
                    Ok(CommandWrite {
                        document: applied.document,
                        journal: JournalWrite {
                            command_type,
                            command,
                            command_id: envelope.command_id,
                            inverse,
                            actor,
                            source: envelope.source,
                            summary,
                            created_at: applied_at,
                        },
                        output,
                    })
                },
            )
            .await
            .map_err(store_error)?;
        let mut output = result.output;
        output.new_revision = result.new_revision;
        self.publish_scenario_events(scenario_id, request_id, &output);
        Ok(output)
    }

    async fn move_history(
        &self,
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        undo: bool,
    ) -> Result<AppCommandResult, AppError> {
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let applied_at = self.clock.now();
        let source = if undo {
            CommandSource::Undo
        } else {
            CommandSource::Redo
        };
        let pack_registry = Arc::clone(&self.pack_registry);
        let cancellation = self.cancellation.clone();
        let control = OperationControl::Cancellation(cancellation.clone());
        let apply = move |history: HistoryCommand| {
            history_apply(
                history,
                expected_revision,
                source,
                &pack_registry,
                &cancellation,
            )
        };
        let committed = if undo {
            self.store
                .undo(
                    scenario_id,
                    expected_revision,
                    applied_at,
                    control.clone(),
                    move |entry| related_replay_receipt_id(entry, scenario_id),
                    apply,
                )
                .await
        } else {
            self.store
                .redo(
                    scenario_id,
                    expected_revision,
                    applied_at,
                    control,
                    move |entry| related_replay_receipt_id(entry, scenario_id),
                    apply,
                )
                .await
        }
        .map_err(store_error)?;
        let mut output = committed.output;
        output.new_revision = committed.new_revision;
        self.publish_scenario_events(scenario_id, request_id, &output);
        Ok(AppCommandResult::ScenarioCommand(output))
    }

    fn publish_scenario_events(
        &self,
        scenario_id: ScenarioId,
        request_id: RequestId,
        result: &CommandResult,
    ) {
        let context = EventContext {
            event_version: EVENT_VERSION,
            timestamp: self.clock.now(),
            request_id: Some(request_id),
            scenario_id: Some(scenario_id),
            revision: Some(result.new_revision),
            solve_run_id: None,
        };
        let _ = self.events.send(AppEvent {
            topic: EventTopic::ScenarioChanged,
            payload: EventPayload::ScenarioChanged {
                context,
                change_set: result.change_set.clone(),
            },
        });
        if !result.validation_delta.added.is_empty() || !result.validation_delta.resolved.is_empty()
        {
            let _ = self.events.send(AppEvent {
                topic: EventTopic::ScenarioValidationChanged,
                payload: EventPayload::ScenarioValidationChanged {
                    context,
                    validation_delta: result.validation_delta.clone(),
                },
            });
        }
    }
    fn publish_changed_event(
        &self,
        scenario_id: ScenarioId,
        request_id: RequestId,
        revision: Revision,
        change_set: ChangeSet,
    ) {
        let _ = self.events.send(AppEvent {
            topic: EventTopic::ScenarioChanged,
            payload: EventPayload::ScenarioChanged {
                context: EventContext {
                    event_version: EVENT_VERSION,
                    timestamp: self.clock.now(),
                    request_id: Some(request_id),
                    scenario_id: Some(scenario_id),
                    revision: Some(revision),
                    solve_run_id: None,
                },
                change_set,
            },
        });
    }
    fn publish_app_notification(&self, request_id: RequestId, code: &str, message: &str) {
        let _ = self.events.send(AppEvent {
            topic: EventTopic::AppNotification,
            payload: EventPayload::AppNotification {
                context: EventContext {
                    event_version: EVENT_VERSION,
                    timestamp: self.clock.now(),
                    request_id: Some(request_id),
                    scenario_id: None,
                    revision: None,
                    solve_run_id: None,
                },
                code: code.to_owned(),
                message: message.to_owned(),
            },
        });
    }

    async fn preview_portable(
        &self,
        bytes: Vec<u8>,
        options: ImportOptions,
        cancellation: &CancellationToken,
    ) -> Result<AppQueryResult, AppError> {
        self.check_portable_cancelled(cancellation)?;
        let (metadata, bytes) = tokio::task::spawn_blocking(move || {
            let metadata = preflight_bundle_metadata(&bytes, &InspectionPolicy::default())?;
            Ok::<_, eutheto_import::ImportError>((metadata, bytes))
        })
        .await
        .map_err(join_error)?
        .map_err(|error| import_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        preflight_import_packs(&metadata, &self.pack_registry)?;
        let registry = Arc::clone(&self.pack_registry);
        let mut inspected =
            tokio::task::spawn_blocking(move || inspect_application_bundle(&bytes, &registry))
                .await
                .map_err(join_error)?
                .map_err(|error| import_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        let mut portable_settings =
            prepare_portable_inspection(&mut inspected, options.restore_mode, &self.pack_registry)?;
        let (local, local_settings) = self.local_library_snapshot().await?;
        self.check_portable_cancelled(cancellation)?;
        let mut preview = eutheto_import::build_preview(&inspected, &options, &local)
            .map_err(|error| import_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        preview.settings_changed = portable_settings
            .iter()
            .filter(|(key, imported)| local_settings.get(*key) != Some(*imported))
            .map(|(key, _)| key.clone())
            .collect();
        preview.settings_removed = if options.restore_mode == RestoreMode::ReplaceLibrary {
            local_settings
                .keys()
                .filter(|key| !portable_settings.contains_key(*key))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        // Add mode preserves the imported source timestamp for actual changes,
        // while byte-identical value-and-timestamp settings are omitted so a
        // fully skipped restore remains a durable no-op.
        if options.restore_mode == RestoreMode::AddBackup {
            portable_settings.retain(|key, imported| local_settings.get(key) != Some(&*imported));
        }
        self.check_portable_cancelled(cancellation)?;
        let preview_id = self
            .retain_portable_preview(
                None,
                PendingImportPreview {
                    inspected,
                    preview: preview.clone(),
                    options,
                    portable_settings,
                    safety_backup_failure: None,
                    retained_bytes: 0,
                },
                cancellation,
            )
            .await?;
        Ok(AppQueryResult::PortablePreview {
            preview_id,
            preview: Box::new(preview),
        })
    }
    async fn inspect_unopened_bundle(
        &self,
        bytes: Vec<u8>,
        cancellation: &CancellationToken,
    ) -> Result<AppQueryResult, AppError> {
        self.check_portable_cancelled(cancellation)?;
        let unopened = tokio::task::spawn_blocking(move || {
            inspect_unopened_bundle_for_exact_reexport(&bytes, &InspectionPolicy::default())
        })
        .await
        .map_err(join_error)?
        .map_err(|error| import_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        let metadata = unopened.metadata().clone();
        let pending = PendingPortablePreview::Unopened(unopened);
        let retained_bytes = pending.retained_bytes();
        if retained_bytes > MAX_PENDING_PREVIEW_BYTES {
            return Err(protocol_error(
                "portable.preview_too_large",
                "The unopened bundle exceeds the retained preview limit.",
                false,
            ));
        }
        self.check_portable_cancelled(cancellation)?;
        let mut previews = self.previews.lock().await;
        let preview_id = self
            .next_preview_id(&previews)?
            .ok_or_else(Self::preview_id_unavailable)?;
        self.check_portable_cancelled(cancellation)?;
        while previews.len() >= MAX_PENDING_PREVIEWS
            || preview_total_bytes(&previews)
                .checked_add(retained_bytes)
                .is_none_or(|total| total > MAX_PENDING_PREVIEW_BYTES)
        {
            let Some(oldest) = previews.keys().next().copied() else {
                break;
            };
            previews.remove(&oldest);
        }
        previews.insert(preview_id, pending);
        drop(previews);
        Ok(AppQueryResult::UnopenedBundlePreview {
            preview_id,
            metadata,
        })
    }

    async fn exact_reexport_unopened_bundle(
        &self,
        preview_id: RequestId,
        destination: PathBuf,
        cancellation: &CancellationToken,
    ) -> Result<AppCommandResult, AppError> {
        let mut previews = self.previews.lock().await;
        self.check_portable_cancelled(cancellation)?;
        match previews.get(&preview_id) {
            Some(PendingPortablePreview::Unopened(_)) => {}
            Some(_) => {
                return Err(protocol_error(
                    "portable.preview_capability_mismatch",
                    "The portable preview has a different capability.",
                    false,
                ));
            }
            None => {
                return Err(protocol_error(
                    "portable.preview_not_found",
                    "The portable preview is no longer available.",
                    false,
                ));
            }
        }
        let Some(PendingPortablePreview::Unopened(unopened)) = previews.remove(&preview_id) else {
            return Err(protocol_error(
                "portable.preview_not_found",
                "The portable preview is no longer available.",
                false,
            ));
        };
        drop(previews);
        self.check_portable_cancelled(cancellation)?;
        let bytes = unopened.into_exact_bytes();
        let cancellation = cancellation.clone();
        tokio::task::spawn_blocking(move || {
            write_bundle_atomic_cancellable(&destination, &bytes, &cancellation)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        Ok(AppCommandResult::UnopenedBundleReexported)
    }

    async fn local_library_snapshot(
        &self,
    ) -> Result<(LocalLibrarySnapshot, BTreeMap<String, AppSetting<Value>>), AppError> {
        let snapshot = self
            .store
            .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
            .await
            .map_err(store_error)?;
        let LibraryUuidClosure {
            mut owned_by_scenario,
            occupied,
        } = library_uuid_closure(&snapshot);
        let occupied_uuids = occupied.into_iter().map(|id| id.to_string()).collect();
        let scenarios = snapshot
            .projects
            .iter()
            .map(|project| {
                let scenario_id = project.document.scenario_id;
                LocalScenarioSnapshot {
                    scenario_id,
                    title: project.document.metadata.title.clone(),
                    revision: project.summary.revision,
                    archived: project.summary.archived_at.is_some(),
                    owned_uuids: owned_by_scenario.remove(&scenario_id).unwrap_or_default(),
                }
            })
            .collect();
        let settings = snapshot.settings;
        let local = LocalLibrarySnapshot {
            revision: snapshot.revision,
            scenario_revision_high_water: snapshot.scenario_revision_high_water,
            identity_owners: snapshot.scenario_identity_owners,
            scenario_ids: snapshot
                .projects
                .iter()
                .map(|project| project.document.scenario_id)
                .collect(),
            occupied_uuids,
            scenarios,
            settings: settings
                .iter()
                .map(|(key, setting)| (key.clone(), setting.value.clone()))
                .collect(),
            supplemental_identities: snapshot.supplemental_identities,
            supplemental_identity_owners: snapshot.supplemental_identity_owners,
        };
        Ok((local, settings))
    }

    async fn apply_portable(
        &self,
        request_id: RequestId,
        preview_id: RequestId,
        collision_plan: CollisionPlan,
        authorization: Option<RestoreAuthorization>,
        cancellation: CancellationToken,
    ) -> Result<AppCommandResult, AppError> {
        let token = cancellation.child();
        let _guard = CancelOnDrop::new(token.clone());
        let app = self.clone();
        tokio::spawn(async move {
            app.apply_portable_owned(request_id, preview_id, collision_plan, authorization, token)
                .await
        })
        .await
        .map_err(join_error)?
    }

    async fn apply_portable_owned(
        &self,
        request_id: RequestId,
        preview_id: RequestId,
        collision_plan: CollisionPlan,
        authorization: Option<RestoreAuthorization>,
        cancellation: CancellationToken,
    ) -> Result<AppCommandResult, AppError> {
        self.check_portable_cancelled(&cancellation)?;
        let (pending, local) = self
            .take_pending_portable_preview(preview_id, &cancellation)
            .await?;
        let copy_control = OperationControl::Cancellation(cancellation.clone());
        let staged = self.stage_portable_apply(&pending, local, &collision_plan, &copy_control)?;
        self.check_portable_cancelled(&cancellation)?;
        let committed = self
            .commit_staged_portable(
                preview_id,
                pending,
                &collision_plan,
                authorization,
                staged,
                &cancellation,
            )
            .await?;
        if committed.library_changed {
            self.publish_app_notification(
                request_id,
                "library.refreshed",
                "The portable library changed and authoritative views must be refreshed.",
            );
        }
        self.publish_portable_apply_events(request_id, committed.events);
        Ok(AppCommandResult::PortableApplied {
            scenarios: committed.scenarios,
            library_revision: committed.library_revision,
            safety_backup: committed.safety_backup,
        })
    }

    async fn take_pending_portable_preview(
        &self,
        preview_id: RequestId,
        cancellation: &CancellationToken,
    ) -> Result<(PendingImportPreview, LocalLibrarySnapshot), AppError> {
        let mut previews = self.previews.lock().await;
        self.check_portable_cancelled(cancellation)?;
        match previews.get(&preview_id) {
            Some(PendingPortablePreview::Import(_)) => {}
            Some(_) => {
                return Err(protocol_error(
                    "portable.preview_capability_mismatch",
                    "The portable preview has a different capability.",
                    false,
                ));
            }
            None => {
                return Err(protocol_error(
                    "portable.preview_not_found",
                    "The portable preview is no longer available.",
                    false,
                ));
            }
        }
        let Some(PendingPortablePreview::Import(pending)) = previews.remove(&preview_id) else {
            return Err(protocol_error(
                "portable.preview_not_found",
                "The portable preview is no longer available.",
                false,
            ));
        };
        drop(previews);
        let (local, _) = self.local_library_snapshot().await?;
        if pending.preview.binding.local_library_revision != local.revision {
            return Err(AppError::Conflict {
                expected_revision: pending.preview.binding.local_library_revision,
                actual_revision: local.revision,
            });
        }
        eutheto_import::validate_preview_binding(
            &pending.preview.binding,
            &pending.inspected.file_sha256,
            &pending.options,
            local.revision,
        )
        .map_err(|error| import_error(&error))?;
        Ok((*pending, local))
    }

    // Keep current/history normalization and validation together before preparing one transaction.
    #[allow(clippy::too_many_lines)]
    fn stage_portable_apply(
        &self,
        pending: &PendingImportPreview,
        local: LocalLibrarySnapshot,
        collision_plan: &CollisionPlan,
        control: &OperationControl,
    ) -> Result<StagedPortableApply, AppError> {
        let mut import = eutheto_import::stage_import(
            &pending.inspected,
            &pending.preview,
            &pending.options,
            &local,
            collision_plan,
        )
        .map_err(|error| import_error(&error))?;
        let reference_remap = std::mem::take(&mut import.reference_remap);
        for staged in &mut import.scenarios {
            if staged.scenario.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
                continue;
            }
            let original = pending
                .inspected
                .scenarios
                .iter()
                .find(|source| source.document.scenario_id == staged.original_id)
                .ok_or_else(|| {
                    import_error(&eutheto_import::ImportError::Remap(
                        "Workforce import source scenario is missing".to_owned(),
                    ))
                })?;
            normalize_workforce_manual_copy(
                &original.document,
                &mut staged.scenario.document,
                &staged.id_remap,
                &reference_remap,
                Some(control),
            )
            .map_err(store_error)?;
        }
        let copied_scenarios = &import.scenarios;
        for copied_revision in &mut import.scenario_revisions {
            let Some(staged) = copied_scenarios.iter().find(|staged| {
                staged.scenario.document.scenario_id == copied_revision.document.scenario_id
                    && staged.scenario.document.domain_pack.id.as_str() == WORKFORCE_PACK_ID
            }) else {
                continue;
            };
            let original = pending
                .inspected
                .scenario_revisions
                .iter()
                .chain(&pending.inspected.scenarios)
                .find(|source| {
                    source.document.scenario_id == staged.original_id
                        && source.revision == copied_revision.revision
                })
                .ok_or_else(|| {
                    import_error(&eutheto_import::ImportError::Remap(
                        "historical Workforce import source revision is missing".to_owned(),
                    ))
                })?;
            normalize_workforce_manual_copy(
                &original.document,
                &mut copied_revision.document,
                &staged.id_remap,
                &reference_remap,
                Some(control),
            )
            .map_err(store_error)?;
        }
        drop(reference_remap);
        validate_import_documents(
            import
                .scenarios
                .iter()
                .map(|item| &item.scenario.document)
                .chain(import.scenario_revisions.iter().map(|item| &item.document)),
            &self.pack_registry,
        )?;
        let scenarios = import
            .scenarios
            .iter()
            .map(|item| AppliedPortableScenario {
                source_scenario_id: item.original_id,
                scenario_id: item.scenario.document.scenario_id,
            })
            .collect::<Vec<_>>();
        let mut events = portable_apply_events(&import.scenarios);
        let remove_scenario_ids = if pending.options.restore_mode == RestoreMode::ReplaceLibrary {
            let imported = scenarios
                .iter()
                .map(|scenario| scenario.scenario_id)
                .collect::<BTreeSet<_>>();
            for removed in &local.scenarios {
                if local.scenario_ids.contains(&removed.scenario_id)
                    && !imported.contains(&removed.scenario_id)
                {
                    events.push((removed.scenario_id, removed.revision, ChangeKind::Removed));
                }
            }
            local.scenario_ids
        } else {
            BTreeSet::new()
        };
        Ok(StagedPortableApply {
            import,
            scenarios,
            events,
            remove_scenario_ids,
        })
    }

    async fn commit_staged_portable(
        &self,
        preview_id: RequestId,
        mut pending: PendingImportPreview,
        collision_plan: &CollisionPlan,
        authorization: Option<RestoreAuthorization>,
        staged: StagedPortableApply,
        cancellation: &CancellationToken,
    ) -> Result<CommittedPortableApply, AppError> {
        let StagedPortableApply {
            import,
            scenarios,
            events,
            remove_scenario_ids,
        } = staged;
        let applied_at = self.clock.now();
        let (outcome, safety_backup) = if let Some(authorization) = authorization {
            let (restore_authorization, safety_backup) = self
                .authorize_portable_restore(
                    preview_id,
                    &mut pending,
                    collision_plan,
                    authorization,
                    cancellation,
                )
                .await?;
            self.check_portable_cancelled(cancellation)?;
            let outcome = self
                .store
                .apply_staged_library(
                    StagedLibraryApply::BackupRestore {
                        restore: eutheto_import::StagedBackupRestore {
                            import,
                            remove_scenario_ids,
                            authorization: restore_authorization,
                        },
                        settings: pending.portable_settings,
                    },
                    applied_at,
                    cancellation.clone(),
                )
                .await
                .map_err(store_error)?;
            (outcome, safety_backup)
        } else {
            if pending.options.restore_mode != RestoreMode::ImportScenario {
                return Err(validation_error(
                    "portable.import_mode_invalid",
                    "/restoreMode",
                    "Project import requires import-scenario mode.",
                ));
            }
            if !pending.portable_settings.is_empty() {
                return Err(validation_error(
                    "settings.import_mode_invalid",
                    "/restoreMode",
                    "Application settings may only be restored from a full backup.",
                ));
            }
            self.check_portable_cancelled(cancellation)?;
            let outcome = self
                .store
                .apply_staged_library(
                    StagedLibraryApply::Import(import),
                    applied_at,
                    cancellation.clone(),
                )
                .await
                .map_err(store_error)?;
            (outcome, SafetyBackupOutcome::NotRequired)
        };
        Ok(CommittedPortableApply {
            scenarios,
            events,
            library_changed: outcome.library_revision
                != pending.preview.binding.local_library_revision,
            library_revision: outcome.library_revision,
            safety_backup,
        })
    }

    async fn authorize_portable_restore(
        &self,
        preview_id: RequestId,
        pending: &mut PendingImportPreview,
        collision_plan: &CollisionPlan,
        authorization: RestoreAuthorization,
        cancellation: &CancellationToken,
    ) -> Result<(RestoreAuthorization, SafetyBackupOutcome), AppError> {
        self.check_portable_cancelled(cancellation)?;
        match pending.options.restore_mode {
            RestoreMode::ImportScenario => Err(validation_error(
                "portable.restore_mode_invalid",
                "/restoreMode",
                "Backup restore requires add-backup or replace-library mode.",
            )),
            RestoreMode::AddBackup => {
                if authorization.safety_backup != SafetyBackupEvidence::NotRequired
                    || authorization.prospective_failure_receipt_token.is_some()
                    || authorization.collision_plan_sha256.is_some()
                {
                    return Err(validation_error(
                        "restore.safety_backup_evidence_invalid",
                        "/authorization",
                        "Add restore does not accept safety-backup receipt fields or evidence.",
                    ));
                }
                Ok((
                    RestoreAuthorization {
                        destructive_action_confirmed: authorization.destructive_action_confirmed,
                        safety_backup: SafetyBackupEvidence::NotRequired,
                        prospective_failure_receipt_token: None,
                        collision_plan_sha256: None,
                    },
                    SafetyBackupOutcome::NotRequired,
                ))
            }
            RestoreMode::ReplaceLibrary => {
                self.authorize_replace_restore(
                    preview_id,
                    pending,
                    collision_plan,
                    authorization,
                    cancellation,
                )
                .await
            }
        }
    }

    async fn authorize_replace_restore(
        &self,
        preview_id: RequestId,
        pending: &mut PendingImportPreview,
        collision_plan: &CollisionPlan,
        authorization: RestoreAuthorization,
        cancellation: &CancellationToken,
    ) -> Result<(RestoreAuthorization, SafetyBackupOutcome), AppError> {
        if !authorization.destructive_action_confirmed {
            return Err(validation_error(
                "restore.confirmation_required",
                "/authorization/destructiveActionConfirmed",
                "Replace mode requires explicit destructive confirmation.",
            ));
        }
        let plan_sha256 = collision_plan_sha256(collision_plan)?;
        if let Some(failure) = &pending.safety_backup_failure {
            return retained_failure_authorization(
                failure,
                &plan_sha256,
                &authorization.safety_backup,
            )
            .map(|authorization| (authorization, SafetyBackupOutcome::ConfirmedBypass));
        }
        match authorization.safety_backup {
            SafetyBackupEvidence::FailedWithStrongConfirmation { proof } => {
                if proof == REPLACE_WITHOUT_BACKUP_PHRASE {
                    return Err(validation_error(
                        "restore.safety_backup_override_not_available",
                        "/authorization/safetyBackup",
                        "The safety-backup override phrase is accepted only after this preview's backup attempt fails.",
                    ));
                }
                Ok((
                    RestoreAuthorization {
                        destructive_action_confirmed: true,
                        safety_backup: SafetyBackupEvidence::FailedWithStrongConfirmation { proof },
                        prospective_failure_receipt_token: None,
                        collision_plan_sha256: Some(plan_sha256),
                    },
                    SafetyBackupOutcome::ConfirmedBypass,
                ))
            }
            SafetyBackupEvidence::Verified { .. } => Err(validation_error(
                "restore.safety_backup_override_not_available",
                "/authorization/safetyBackup",
                "Safety-backup verification evidence is produced only by this application.",
            )),
            SafetyBackupEvidence::NotRequired => {
                if authorization.collision_plan_sha256.is_some() {
                    return Err(validation_error(
                        "restore.collision_plan_hash_invalid",
                        "/authorization/collisionPlanSha256",
                        "A first replace attempt does not accept a caller-supplied collision-plan hash.",
                    ));
                }
                self.authorize_first_replace_attempt(
                    preview_id,
                    pending,
                    &plan_sha256,
                    authorization.prospective_failure_receipt_token,
                    cancellation,
                )
                .await
            }
        }
    }

    async fn authorize_first_replace_attempt(
        &self,
        preview_id: RequestId,
        pending: &mut PendingImportPreview,
        collision_plan_sha256: &str,
        prospective_token: Option<String>,
        cancellation: &CancellationToken,
    ) -> Result<(RestoreAuthorization, SafetyBackupOutcome), AppError> {
        let proof = prospective_failure_proof(self.ids.as_ref(), prospective_token)?;
        match self.create_portable_safety_backup(cancellation).await {
            Ok(backup) => Ok((
                RestoreAuthorization {
                    destructive_action_confirmed: true,
                    safety_backup: SafetyBackupEvidence::Verified {
                        bundle_sha256: backup.sha256,
                    },
                    prospective_failure_receipt_token: None,
                    collision_plan_sha256: Some(collision_plan_sha256.to_owned()),
                },
                SafetyBackupOutcome::CreatedAndVerified {
                    artifact_name: backup.artifact_name,
                },
            )),
            Err(error) => {
                self.check_portable_cancelled(cancellation)?;
                let Some(safe_reason) = safe_backup_failure_reason(&error) else {
                    return Err(error);
                };
                self.store
                    .record_safety_backup_failure_receipt(SafetyBackupFailureReceipt {
                        proof: proof.clone(),
                        binding: pending.preview.binding.clone(),
                        collision_plan_sha256: collision_plan_sha256.to_owned(),
                        safe_reason: safe_reason.clone(),
                        created_at: self.clock.now(),
                    })
                    .await
                    .map_err(store_error)?;
                pending.safety_backup_failure = Some(PendingSafetyBackupFailure {
                    proof,
                    collision_plan_sha256: collision_plan_sha256.to_owned(),
                });
                self.retain_portable_preview(Some(preview_id), pending.clone(), cancellation)
                    .await?;
                Err(protocol_error(
                    "restore.safety_backup_failed",
                    &safe_reason,
                    false,
                ))
            }
        }
    }

    fn publish_portable_apply_events(
        &self,
        request_id: RequestId,
        events: Vec<(ScenarioId, Revision, ChangeKind)>,
    ) {
        for (scenario_id, revision, kind) in events {
            self.publish_changed_event(
                scenario_id,
                request_id,
                revision,
                ChangeSet {
                    changes: vec![Change {
                        kind,
                        path: "/project".to_owned(),
                        before: (kind != ChangeKind::Added).then_some(Value::Bool(true)),
                        after: (kind != ChangeKind::Removed).then_some(Value::Bool(true)),
                    }],
                },
            );
        }
    }

    async fn retain_portable_preview(
        &self,
        requested_id: Option<RequestId>,
        mut pending: PendingImportPreview,
        cancellation: &CancellationToken,
    ) -> Result<RequestId, AppError> {
        pending.retained_bytes = pending
            .retained_memory_charge()
            .filter(|charge| *charge <= MAX_PENDING_PREVIEW_BYTES)
            .ok_or_else(|| {
                protocol_error(
                    "portable.preview_too_large",
                    "The portable preview exceeds the retained preview limit.",
                    false,
                )
            })?;
        let mut previews = self.previews.lock().await;
        let preview_id = match requested_id {
            Some(id) if !previews.contains_key(&id) => id,
            Some(_) => return Err(Self::preview_id_unavailable()),
            None => self
                .next_preview_id(&previews)?
                .ok_or_else(Self::preview_id_unavailable)?,
        };
        self.check_portable_cancelled(cancellation)?;
        while previews.len() >= MAX_PENDING_PREVIEWS
            || preview_total_bytes(&previews)
                .checked_add(pending.retained_bytes)
                .is_none_or(|total| total > MAX_PENDING_PREVIEW_BYTES)
        {
            let Some(oldest) = previews.keys().next().copied() else {
                break;
            };
            previews.remove(&oldest);
        }
        previews.insert(
            preview_id,
            PendingPortablePreview::Import(Box::new(pending)),
        );
        Ok(preview_id)
    }

    fn next_preview_id(
        &self,
        previews: &BTreeMap<RequestId, PendingPortablePreview>,
    ) -> Result<Option<RequestId>, AppError> {
        for _ in 0..MAX_ID_ALLOCATION_ATTEMPTS {
            let id = RequestId::new(self.ids.as_ref()).map_err(id_error)?;
            if !previews.contains_key(&id) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    fn preview_id_unavailable() -> AppError {
        protocol_error(
            "portable.preview_id_unavailable",
            "The portable preview identity is unavailable; create a fresh preview.",
            false,
        )
    }

    async fn create_portable_safety_backup(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<CreatedPortableSafetyBackup, AppError> {
        self.check_portable_cancelled(cancellation)?;
        let safety_backups = self.paths.safety_backups.clone();
        tokio::task::spawn_blocking(move || {
            eutheto_store::ensure_private_application_directory(safety_backups)
        })
        .await
        .map_err(join_error)?
        .map_err(store_error)?;
        let (bytes, _, _) = self
            .export_backup(
                "Automatic pre-restore safety backup".to_owned(),
                BackupSelection::default(),
                cancellation,
            )
            .await?;
        let digest = eutheto_export::sha256_hex(&bytes);
        let expected_length = bytes.len();
        let bundle_id = BundleId::new(self.ids.as_ref()).map_err(id_error)?;
        let artifact_name = format!("{bundle_id}.eutheto");
        let destination = self.paths.safety_backups.join(&artifact_name);
        self.write_bundle(destination.clone(), bytes, cancellation)
            .await?;
        self.check_portable_cancelled(cancellation)?;
        let registry = Arc::clone(&self.pack_registry);
        let verification_cancellation = cancellation.clone();
        let verification = tokio::task::spawn_blocking(move || {
            use std::io::Read;

            let read_limit = expected_length.saturating_add(1);
            let mut published = Vec::with_capacity(read_limit);
            std::fs::File::open(destination)?
                .take(u64::try_from(read_limit).map_err(io::Error::other)?)
                .read_to_end(&mut published)?;
            if verification_cancellation.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Safety backup verification cancelled.",
                ));
            }
            if published.len() != expected_length {
                return Err(io::Error::other("published safety backup length differs"));
            }
            inspect_application_bundle(&published, &registry)
                .map_err(|_| io::Error::other("published safety backup verification failed"))?;
            Ok::<_, io::Error>(eutheto_export::sha256_hex(&published))
        })
        .await
        .map_err(join_error)?;
        self.check_portable_cancelled(cancellation)?;
        let verified_digest = verification.map_err(filesystem_error)?;
        if verified_digest != digest {
            return Err(protocol_error(
                "restore.safety_backup_verification_failed",
                "The published safety backup could not be verified.",
                false,
            ));
        }
        Ok(CreatedPortableSafetyBackup {
            sha256: digest,
            artifact_name,
        })
    }

    /// Reports whether the same bound restore review survived a real safety-backup failure.
    /// Native custody uses this for retry ownership, not as authorization to commit.
    pub async fn portable_restore_retry_is_retained(
        &self,
        preview_id: RequestId,
        expected_library_revision: Revision,
    ) -> bool {
        let previews = self.previews.lock().await;
        matches!(
            previews.get(&preview_id),
            Some(PendingPortablePreview::Import(pending))
                if pending.options.restore_mode == RestoreMode::ReplaceLibrary
                    && pending.preview.binding.local_library_revision == expected_library_revision
                    && pending.safety_backup_failure.is_some()
        )
    }

    async fn cancel_portable_preview(
        &self,
        preview_id: RequestId,
    ) -> Result<AppCommandResult, AppError> {
        let mut previews = self.previews.lock().await;
        if matches!(
            previews.get(&preview_id),
            Some(PendingPortablePreview::Csv(_) | PendingPortablePreview::Settings(_))
        ) {
            return Err(protocol_error(
                "portable.preview_capability_mismatch",
                "The portable preview has a different capability.",
                false,
            ));
        }
        if previews.remove(&preview_id).is_none() {
            return Err(protocol_error(
                "portable.preview_not_found",
                "The portable preview is no longer available.",
                false,
            ));
        }
        Ok(AppCommandResult::PortablePreviewCancelled)
    }

    /// Preserve imported opaque result data as inert backup payload; only canonical local
    /// accepted results may be represented as freshly verified in an exported archive.
    async fn verify_portable_result_sections(
        &self,
        sections: &BTreeMap<String, Value>,
        canonical_result_ids: &BTreeSet<SolutionId>,
        cancellation: &CancellationToken,
    ) -> Result<(), AppError> {
        for (key, value) in sections {
            self.check_portable_cancelled(cancellation)?;
            let result_id = Uuid::parse_str(key)
                .map(SolutionId::from_uuid)
                .map_err(|_| solution_verification_mismatch())?;
            if !canonical_result_ids.contains(&result_id) {
                continue;
            }
            let stored = self
                .store
                .load_accepted_result(result_id)
                .await
                .map_err(|error| match error {
                    StoreError::AcceptedResultNotFound(_) => solution_verification_mismatch(),
                    error => store_error(error),
                })?;
            let verified = self
                .ensure_fresh_accepted(stored, cancellation.child())
                .await?;
            if serde_json::to_value(&verified.portable).map_err(|_| solution_contract_error())?
                != *value
            {
                return Err(solution_verification_mismatch());
            }
        }
        Ok(())
    }

    async fn export_scenario(
        &self,
        scenario_id: ScenarioId,
        cancellation: &CancellationToken,
    ) -> Result<(Vec<u8>, Revision, Revision), AppError> {
        self.check_portable_cancelled(cancellation)?;
        let query_slot = self.acquire_accepted_query_slot(cancellation).await?;
        let library = self
            .store
            .library_snapshot(eutheto_store::NativeArchivalSelection::Scenario(
                scenario_id,
            ))
            .await
            .map_err(store_error)?;
        self.check_portable_cancelled(cancellation)?;
        let project = library
            .projects
            .iter()
            .find(|project| project.document.scenario_id == scenario_id)
            .ok_or(AppError::NotFound(ResourceRef::Scenario(scenario_id)))?;
        let scenario_revision = project.summary.revision;
        let library_revision = library.revision;
        let scenario = portable_scenario(project, false);
        let sections = scenario_export_sections(&library, &scenario)?;
        self.verify_portable_result_sections(
            &sections.results,
            &library.canonical_result_ids,
            cancellation,
        )
        .await?;
        let manifest_extensions = scenario_backup_selection_manifest_extensions(&sections)?;
        let snapshot = ScenarioExportSnapshot {
            bundle_id: BundleId::new(self.ids.as_ref()).map_err(id_error)?,
            created_at: self.clock.now().to_string(),
            application: application_metadata(),
            title: project.document.metadata.title.clone(),
            scenario,
            scenario_revisions: library
                .scenario_revisions
                .iter()
                .filter(|stored| stored.scenario.document.scenario_id == scenario_id)
                .map(|stored| stored.scenario.clone())
                .collect(),
            sections,
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions,
        };
        let registry = Arc::clone(&self.pack_registry);
        let bytes = tokio::task::spawn_blocking(move || {
            let _query_slot = query_slot;
            assemble_scenario_export(&snapshot, &|document| {
                encode_portable_domain(document, &registry)
            })
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        Ok((bytes, scenario_revision, library_revision))
    }
    async fn export_backup(
        &self,
        title: String,
        selection: BackupSelection,
        cancellation: &CancellationToken,
    ) -> Result<(Vec<u8>, BackupSummary, Revision), AppError> {
        self.check_portable_cancelled(cancellation)?;
        if selection.include_audit {
            return Err(AppError::Unsupported(UnsupportedFeature {
                code: "backup.audit_unavailable".to_owned(),
                capability: "Portable backup audit history".to_owned(),
            }));
        }
        let query_slot = self.acquire_accepted_query_slot(cancellation).await?;
        let native_archives = if selection.include_results {
            eutheto_store::NativeArchivalSelection::All
        } else {
            eutheto_store::NativeArchivalSelection::Exclude
        };
        let library = self
            .store
            .library_snapshot(native_archives)
            .await
            .map_err(store_error)?;
        self.check_portable_cancelled(cancellation)?;
        let library_revision = library.revision;
        let (sections, summary) = backup_sections(&library, selection)?;
        self.verify_portable_result_sections(
            &sections.results,
            &library.canonical_result_ids,
            cancellation,
        )
        .await?;
        let scenarios = library
            .projects
            .iter()
            .map(|project| portable_scenario(project, true))
            .collect();
        let scenario_revisions = if selection.include_results {
            library
                .scenario_revisions
                .iter()
                .map(|stored| stored.scenario.clone())
                .collect()
        } else {
            Vec::new()
        };
        let mut manifest_extensions = library.manifest_extensions.clone();
        manifest_extensions.insert(
            BACKUP_SELECTION_EXTENSION.to_owned(),
            backup_selection_manifest_extension(&summary)?,
        );
        let snapshot = FullBackupSnapshot {
            bundle_id: BundleId::new(self.ids.as_ref()).map_err(id_error)?,
            created_at: self.clock.now().to_string(),
            application: application_metadata(),
            title,
            scenarios,
            scenario_revisions,
            sections,
            nonsemantic_extensions: library.nonsemantic_extensions.clone(),
            manifest_extensions,
        };
        let registry = Arc::clone(&self.pack_registry);
        let bytes = tokio::task::spawn_blocking(move || {
            let _query_slot = query_slot;
            assemble_full_backup(&snapshot, &|document| {
                encode_portable_domain(document, &registry)
            })
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        Ok((bytes, summary, library_revision))
    }

    async fn publish_prepared_portable(
        &self,
        destination: PathBuf,
        bytes: Vec<u8>,
        expected_sha256: &str,
        binding: PreparedPortableBinding,
        cancellation: &CancellationToken,
    ) -> Result<AppCommandResult, AppError> {
        self.check_portable_cancelled(cancellation)?;
        if eutheto_export::sha256_hex(&bytes) != expected_sha256 {
            return Err(protocol_error(
                "portable.prepared_digest_mismatch",
                "The prepared portable bytes no longer match the reviewed digest.",
                false,
            ));
        }
        let registry = Arc::clone(&self.pack_registry);
        let (inspected, bytes) = tokio::task::spawn_blocking(move || {
            let inspected = inspect_application_bundle(&bytes, &registry)?;
            Ok::<_, eutheto_import::ImportError>((inspected, bytes))
        })
        .await
        .map_err(join_error)?
        .map_err(|error| import_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        let (expected_library_revision, expected_scenario) = match binding {
            PreparedPortableBinding::Scenario {
                scenario_id,
                expected_revision,
                expected_library_revision,
            } => {
                if inspected.manifest.bundle_kind != BundleKind::ScenarioExport
                    || inspected.scenarios.len() != 1
                    || inspected.scenarios[0].document.scenario_id != scenario_id
                    || inspected.scenarios[0].revision != expected_revision
                {
                    return Err(protocol_error(
                        "portable.prepared_binding_mismatch",
                        "The prepared portable bytes do not match the reviewed scenario binding.",
                        false,
                    ));
                }
                (
                    expected_library_revision,
                    Some((scenario_id, expected_revision)),
                )
            }
            PreparedPortableBinding::Backup {
                expected_library_revision,
            } => {
                if inspected.manifest.bundle_kind != BundleKind::FullBackup {
                    return Err(protocol_error(
                        "portable.prepared_binding_mismatch",
                        "The prepared portable bytes do not match the reviewed backup binding.",
                        false,
                    ));
                }
                (expected_library_revision, None)
            }
        };
        let preparation_cancellation = cancellation.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_bundle_atomic_cancellable(&destination, &bytes, &preparation_cancellation)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        self.check_portable_cancelled(cancellation)?;
        let cancellation = cancellation.clone();
        self.store
            .with_publication_revision_lease(
                expected_library_revision,
                expected_scenario,
                move || prepared.publish_controlled(&OperationControl::Cancellation(cancellation)),
            )
            .await
            .map_err(store_error)?
            .map_err(|error| export_error(&error))?;
        Ok(AppCommandResult::BundleWritten)
    }

    /// Creates a `SQLite` safety backup in the injected private backup directory.
    ///
    /// # Errors
    ///
    /// Returns a typed identity or storage failure if the safety backup cannot
    /// be created at the injected private path.
    pub async fn create_safety_backup(&self) -> Result<PathBuf, AppError> {
        let id = BundleId::new(self.ids.as_ref()).map_err(id_error)?;
        let destination = self.paths.safety_backups.join(format!("{id}.sqlite"));
        self.store
            .safety_backup(&destination)
            .await
            .map_err(store_error)?;
        Ok(destination)
    }
    fn check_cancelled(&self) -> Result<(), AppError> {
        if self.cancellation.is_cancelled() {
            Err(protocol_error(
                "operation.cancelled",
                "The operation was cancelled.",
                false,
            ))
        } else {
            Ok(())
        }
    }

    fn check_portable_cancelled(&self, cancellation: &CancellationToken) -> Result<(), AppError> {
        if self.cancellation.is_cancelled() || cancellation.is_cancelled() {
            Err(protocol_error(
                "operation.cancelled",
                "The operation was cancelled.",
                false,
            ))
        } else {
            Ok(())
        }
    }

    async fn write_bundle(
        &self,
        destination: PathBuf,
        bytes: Vec<u8>,
        cancellation: &CancellationToken,
    ) -> Result<(), AppError> {
        self.check_portable_cancelled(cancellation)?;
        let cancellation = cancellation.clone();
        tokio::task::spawn_blocking(move || {
            write_bundle_atomic_cancellable(&destination, &bytes, &cancellation)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))
    }
}

fn portable_apply_events(
    scenarios: &[eutheto_import::StagedScenario],
) -> Vec<(ScenarioId, Revision, ChangeKind)> {
    scenarios
        .iter()
        .map(|staged| {
            let kind = match staged.disposition {
                StagedDisposition::Create | StagedDisposition::CreateCopy => ChangeKind::Added,
                StagedDisposition::Replace => ChangeKind::Updated,
            };
            (
                staged.scenario.document.scenario_id,
                staged.scenario.revision,
                kind,
            )
        })
        .collect()
}

fn prospective_failure_proof(
    ids: &dyn IdGenerator,
    prospective_token: Option<String>,
) -> Result<String, AppError> {
    prospective_token.map_or_else(
        || RequestId::new(ids).map(|token| token.to_string()).map_err(id_error),
        |token| {
            token
                .parse::<RequestId>()
                .map(|parsed| parsed.to_string())
                .map_err(|_| {
                    validation_error(
                        "restore.failure_receipt_token_invalid",
                        "/authorization/prospectiveFailureReceiptToken",
                        "The prospective failure-receipt token must be a UUIDv7 request identifier.",
                    )
                })
        },
    )
}

fn retained_failure_authorization(
    failure: &PendingSafetyBackupFailure,
    collision_plan_sha256: &str,
    evidence: &SafetyBackupEvidence,
) -> Result<RestoreAuthorization, AppError> {
    if failure.collision_plan_sha256 != collision_plan_sha256 {
        return Err(validation_error(
            "restore.safety_backup_binding_changed",
            "/collisionPlan",
            "The reviewed collision choices changed after the safety backup failed.",
        ));
    }
    match evidence {
        SafetyBackupEvidence::FailedWithStrongConfirmation { proof }
            if proof == REPLACE_WITHOUT_BACKUP_PHRASE =>
        {
            Ok(RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup: SafetyBackupEvidence::FailedWithStrongConfirmation {
                    proof: failure.proof.clone(),
                },
                prospective_failure_receipt_token: None,
                collision_plan_sha256: Some(collision_plan_sha256.to_owned()),
            })
        }
        _ => Err(validation_error(
            "restore.safety_backup_override_required",
            "/authorization/safetyBackup",
            "The exact safety-backup override phrase is required for this failed preview.",
        )),
    }
}

fn safe_backup_failure_reason(error: &AppError) -> Option<String> {
    let is_backup_failure = match error {
        AppError::Storage(failure) => matches!(
            failure.code.as_str(),
            "storage.directory_initialization_failed"
                | "storage.private_path_unsafe"
                | "portable.destination_exists"
                | "portable.export_verification_failed"
                | "portable.export_encoding_failed"
                | "portable.export_io_failed"
        ),
        AppError::Protocol(failure) => {
            failure.code == "restore.safety_backup_verification_failed"
                || failure.code == "application.blocking_task_failed"
                || failure.code == "identity.generation_failed"
                || failure.code.starts_with("backup.")
        }
        _ => false,
    };
    is_backup_failure.then(|| match error {
        AppError::Storage(failure) => failure.message.clone(),
        AppError::Protocol(failure) => failure.message.clone(),
        _ => unreachable!("only storage and protocol failures are classified"),
    })
}

fn inspect_application_bundle(
    bytes: &[u8],
    registry: &Arc<DomainPackRegistry>,
) -> Result<InspectedBundle, eutheto_import::ImportError> {
    let migrations = application_portable_migrations(registry)?;
    inspect_bundle(bytes, &InspectionPolicy::default(), &migrations, &|wire| {
        decode_portable_domain(wire, registry)
    })
}

fn application_portable_migrations(
    registry: &Arc<DomainPackRegistry>,
) -> Result<MigrationRegistries, eutheto_import::ImportError> {
    let migration_registry = Arc::clone(registry);
    MigrationRegistries::new(
        Vec::new(),
        vec![PortableMigrationStep {
            from_version: 1,
            to_version: 2,
            name: "global-portable-v1-to-v2",
            migrate: Arc::new(move |value| migrate_global_portable_v1(value, &migration_registry)),
        }],
    )
}

fn portable_conversion_failure() -> MigrationFailure {
    MigrationFailure::Invalid(
        "The registered domain pack could not safely convert this scenario revision.".to_owned(),
    )
}

fn domain_conversion_error(error: &DomainPackError) -> MigrationFailure {
    match error {
        DomainPackError::ResourceLimitExceeded => MigrationFailure::ResourceLimitExceeded,
        _ => portable_conversion_failure(),
    }
}

fn portable_conversion_error(error: AppError) -> MigrationFailure {
    match error {
        AppError::Validation(report) => MigrationFailure::Validation(report),
        AppError::Protocol(failure) if failure.code == "operation.resource_limit" => {
            MigrationFailure::ResourceLimitExceeded
        }
        _ => portable_conversion_failure(),
    }
}

fn migrate_global_portable_v1(
    value: Value,
    registry: &DomainPackRegistry,
) -> Result<PortableMigrationOutcome, MigrationFailure> {
    let mut snapshot: ScenarioSnapshotV1 =
        serde_json::from_value(value).map_err(|_| portable_conversion_failure())?;
    eutheto_export::validate_scenario_snapshot(&snapshot)
        .map_err(|_| portable_conversion_failure())?;
    validate_document_shape(&snapshot.document).map_err(|error| {
        portable_conversion_error(validation_error(
            error.code(),
            "/domain",
            "The imported domain payload is malformed.",
        ))
    })?;
    let mut applied_migrations = Vec::new();
    migrate_import_document(
        &mut snapshot.document,
        registry,
        snapshot.revision,
        &mut applied_migrations,
    )
    .map_err(portable_conversion_error)?;
    let domain =
        encode_portable_domain(&snapshot.document, registry).map_err(|error| match error {
            ExportError::ResourceLimitExceeded => MigrationFailure::ResourceLimitExceeded,
            _ => portable_conversion_failure(),
        })?;
    let introduced_requirements = domain
        .required_capabilities
        .difference(&snapshot.required_capabilities)
        .cloned()
        .collect();
    let wire = PortableScenario::from_snapshot(&snapshot, domain)
        .map_err(|_| portable_conversion_failure())?;
    Ok(PortableMigrationOutcome {
        value: serde_json::to_value(wire).map_err(|_| portable_conversion_failure())?,
        introduced_requirements,
        applied_migrations,
    })
}

fn validate_portable_domain_envelope(
    domain: &PortableDomainDocument,
    descriptor: &DomainPackDescriptor,
) -> Result<(), MigrationFailure> {
    if domain.pack_id != descriptor.id
        || !descriptor.portable_versions.supports(domain.schema_version)
        || domain.payload.get("schemaVersion").and_then(Value::as_u64)
            != Some(u64::from(domain.schema_version))
        || !domain
            .required_capabilities
            .is_subset(&descriptor.portable_capabilities)
    {
        return Err(portable_conversion_failure());
    }
    let bytes =
        eutheto_export::canonical_json(domain).map_err(|_| portable_conversion_failure())?;
    if u64::try_from(bytes.len()).map_or(true, |size| {
        size > eutheto_export::PORTABLE_LIMITS.max_json_bytes
    }) {
        return Err(portable_conversion_failure());
    }
    eutheto_export::validate_portable_json_value(
        &domain.payload,
        &eutheto_export::PORTABLE_LIMITS,
        0,
    )
    .map_err(|_| portable_conversion_failure())
}

fn decode_portable_domain(
    wire: &PortableScenario,
    registry: &DomainPackRegistry,
) -> Result<DecodedPortableDomain, MigrationFailure> {
    let descriptor = registry
        .descriptors()
        .find(|descriptor| descriptor.id == wire.domain.pack_id)
        .ok_or_else(portable_conversion_failure)?;
    let pack = registry
        .require(&wire.domain.pack_id)
        .map_err(|_| portable_conversion_failure())?;
    validate_portable_domain_envelope(&wire.domain, descriptor)?;
    let mut domain = std::borrow::Cow::Borrowed(&wire.domain);
    let mut applied_migrations = Vec::new();
    while domain.schema_version != descriptor.portable_versions.latest {
        let previous = domain.schema_version;
        let expected = previous
            .checked_add(1)
            .ok_or_else(portable_conversion_failure)?;
        domain = std::borrow::Cow::Owned(
            pack.migrate_portable_step(domain.into_owned())
                .map_err(|error| domain_conversion_error(&error))?,
        );
        if domain.schema_version != expected {
            return Err(portable_conversion_failure());
        }
        validate_portable_domain_envelope(&domain, descriptor)?;
        applied_migrations.push(AppliedMigration {
            registry: MigrationRegistryKind::Portable,
            name: "pack-portable-schema".to_owned(),
            from_version: previous,
            to_version: expected,
            version_space: Some(PackMigrationVersionSpace::Portable),
            subject: Some(MigrationSubject {
                pack_id: wire.domain.pack_id.clone(),
                scenario_id: wire.scenario_id,
                revision: wire.revision,
            }),
        });
    }
    let context = PortableImportContext {
        scenario_shell: ScenarioDocument::new(
            wire.scenario_id,
            DomainPackRef {
                id: wire.domain.pack_id.clone(),
                schema_version: descriptor.scenario_versions.latest,
            },
            wire.metadata.clone(),
            wire.settings.clone(),
            ScenarioDomain::default(),
            BTreeMap::new(),
        ),
    };
    let document = pack
        .import_portable(&domain, &context)
        .map_err(|error| domain_conversion_error(&error))?;
    if document.domain_pack.schema_version != descriptor.scenario_versions.latest {
        return Err(portable_conversion_failure());
    }
    validate_document_shape(&document).map_err(|_| portable_conversion_failure())?;
    Ok(DecodedPortableDomain {
        document,
        required_capabilities: domain.required_capabilities.clone(),
        applied_migrations,
    })
}

fn preflight_import_packs(
    metadata: &PreservedBundleMetadata,
    registry: &DomainPackRegistry,
) -> Result<(), AppError> {
    for scenario in &metadata.scenarios {
        let Some(pack_id) = scenario
            .pack_id
            .as_deref()
            .and_then(|pack_id| pack_id.parse::<PackId>().ok())
        else {
            return Err(unsupported_import_pack());
        };
        let Some(descriptor) = registry
            .descriptors()
            .find(|descriptor| descriptor.id == pack_id)
        else {
            return Err(unsupported_import_pack());
        };
        let supported = match (
            scenario.internal_pack_schema_version,
            scenario.portable_pack_schema_version,
        ) {
            (Some(version), None) => descriptor.scenario_versions.supports(version),
            (None, Some(version)) => descriptor.portable_versions.supports(version),
            _ => false,
        };
        if !supported {
            return Err(unsupported_import_pack());
        }
    }
    Ok(())
}

fn prepare_portable_inspection(
    inspected: &mut InspectedBundle,
    restore_mode: RestoreMode,
    registry: &DomainPackRegistry,
) -> Result<BTreeMap<String, AppSetting<Value>>, AppError> {
    let expected_kind = if restore_mode == RestoreMode::ImportScenario {
        BundleKind::ScenarioExport
    } else {
        BundleKind::FullBackup
    };
    if inspected.manifest.bundle_kind != expected_kind {
        return Err(validation_error(
            "portable.bundle_kind_invalid",
            "/manifest/bundleKind",
            if expected_kind == BundleKind::ScenarioExport {
                "Project import requires a scenario-export bundle."
            } else {
                "Backup restore requires a full-backup bundle."
            },
        ));
    }
    let mut portable_preferences = BTreeMap::new();
    for path in [
        "preferences/application-settings.json",
        "preferences/application-settings",
    ] {
        if let Some(bytes) = inspected.additional_entries.remove(path) {
            portable_preferences.insert(path.trim_start_matches("preferences/").to_owned(), bytes);
        }
    }
    let portable_settings = take_portable_settings(&mut portable_preferences, restore_mode)?;
    validate_import_documents(
        inspected
            .scenarios
            .iter()
            .chain(&inspected.scenario_revisions)
            .map(|item| &item.document),
        registry,
    )?;
    Ok(portable_settings)
}

fn preview_total_bytes(previews: &BTreeMap<RequestId, PendingPortablePreview>) -> usize {
    previews.values().fold(0_usize, |total, preview| {
        total.saturating_add(preview.retained_bytes())
    })
}

fn migrate_import_document(
    document: &mut ScenarioDocument,
    registry: &DomainPackRegistry,
    revision: Revision,
    applied: &mut Vec<AppliedMigration>,
) -> Result<(), AppError> {
    let Some(descriptor) = registry
        .descriptors()
        .find(|descriptor| descriptor.id == document.domain_pack.id)
    else {
        return Err(unsupported_import_pack());
    };
    let source_version = document.domain_pack.schema_version;
    let target_version = descriptor.scenario_versions.latest;
    if source_version == target_version {
        return Ok(());
    }
    if !descriptor
        .scenario_versions
        .migratable_from
        .contains(&source_version)
    {
        return Err(unsupported_import_pack());
    }
    let pack = registry
        .require(&document.domain_pack.id)
        .map_err(|_| unsupported_import_pack())?;
    let original_format = document.format;
    let original_format_version = document.format_version;
    let original_scenario_id = document.scenario_id;
    let original_pack_id = document.domain_pack.id.clone();
    let original_metadata = document.metadata.clone();
    let original_settings = document.settings.clone();
    let original_extensions = document.extensions.clone();
    let mut migrated = document.clone();
    while migrated.domain_pack.schema_version != target_version {
        let previous_version = migrated.domain_pack.schema_version;
        let expected_version = previous_version.checked_add(1).ok_or_else(|| {
            migration_contract_error("The imported scenario migration version overflowed.")
        })?;
        if expected_version != target_version
            && !descriptor
                .scenario_versions
                .migratable_from
                .contains(&expected_version)
        {
            return Err(migration_contract_error(
                "The imported scenario does not have a complete sequential migration path.",
            ));
        }
        migrated = pack.migrate_document(migrated).map_err(|error| {
            domain_interruption(&error).unwrap_or_else(|| {
                migration_contract_error(
                    "The imported scenario domain document could not be migrated.",
                )
            })
        })?;
        if migrated.domain_pack.schema_version != expected_version
            || migrated.domain_pack.schema_version > target_version
        {
            return Err(migration_contract_error(
                "The domain pack did not perform exactly one sequential migration step.",
            ));
        }
        if migrated.format != original_format
            || migrated.format_version != original_format_version
            || migrated.scenario_id != original_scenario_id
            || migrated.domain_pack.id != original_pack_id
            || migrated.metadata != original_metadata
            || migrated.settings != original_settings
            || migrated.extensions != original_extensions
        {
            return Err(migration_contract_error(
                "The domain pack migration changed host-owned scenario data.",
            ));
        }
        validate_document_shape(&migrated).map_err(|_| {
            migration_contract_error("The migrated domain payload is malformed or exceeds limits.")
        })?;
        let bytes = eutheto_export::canonical_json(&migrated).map_err(|_| {
            migration_contract_error("The migrated domain payload could not be serialized.")
        })?;
        if u64::try_from(bytes.len()).map_or(true, |size| {
            size > eutheto_export::PORTABLE_LIMITS.max_json_bytes
        }) {
            return Err(migration_contract_error(
                "The migrated domain payload exceeds the document byte limit.",
            ));
        }
        applied.push(AppliedMigration {
            registry: MigrationRegistryKind::Portable,
            name: "pack-internal-schema".to_owned(),
            from_version: previous_version,
            to_version: expected_version,
            version_space: Some(PackMigrationVersionSpace::Internal),
            subject: Some(MigrationSubject {
                pack_id: original_pack_id.clone(),
                scenario_id: original_scenario_id,
                revision,
            }),
        });
    }
    *document = migrated;
    Ok(())
}

fn unsupported_import_pack() -> AppError {
    validation_error(
        "domain_pack.unsupported",
        "/domainPack",
        "The imported scenario domain pack is not available.",
    )
}

fn migration_contract_error(message: &str) -> AppError {
    validation_error("domain_pack.migration_failed", "/domainPack", message)
}

fn validate_import_documents<'a>(
    documents: impl IntoIterator<Item = &'a ScenarioDocument>,
    registry: &DomainPackRegistry,
) -> Result<(), AppError> {
    for document in documents {
        if available_pack(document, registry).is_none() {
            return Err(validation_error(
                "domain_pack.unsupported",
                "/domainPack",
                "The imported scenario domain pack is not available.",
            ));
        }
        // Portability accepts structurally valid drafts. Solving validation remains an explicit
        // ValidateScenario/solve operation, not an import or migration gate.
        validate_document_shape(document).map_err(|error| {
            validation_error(
                error.code(),
                "/domain",
                "The imported domain payload is malformed.",
            )
        })?;
    }
    Ok(())
}

fn backup_sections(
    library: &eutheto_store::LibrarySnapshot,
    selection: BackupSelection,
) -> Result<(BackupSections, BackupSummary), AppError> {
    let results = if selection.include_results {
        portable_json_section(&library.sections.results, None)?
    } else {
        BTreeMap::new()
    };
    let shared_records = portable_json_section(&library.sections.shared_records, None)?;
    let mut preferences = portable_json_section(
        &library.sections.preferences,
        Some(PORTABLE_SETTINGS_SECTION),
    )?;
    let mut settings = serde_json::Map::new();
    for (key, setting) in &library.settings {
        if validate_app_setting(key, &setting.value).is_ok() {
            settings.insert(
                key.clone(),
                serde_json::json!({
                    "value": setting.value,
                    "updatedAt": setting.updated_at.to_string(),
                }),
            );
        }
    }
    preferences.insert(
        PORTABLE_SETTINGS_SECTION.to_owned(),
        Value::Object(settings),
    );
    let SelectedBackupAssets {
        assets,
        excluded_ids: excluded_asset_ids,
        inherited_omissions,
    } = selected_backup_assets(&library.sections.assets, selection.assets)?;
    let excluded_asset_count = u64::try_from(excluded_asset_ids.len()).map_err(|_| {
        protocol_error(
            "backup.asset_count_overflow",
            "The excluded asset count could not be represented.",
            false,
        )
    })?;
    let exclusion_scope = (excluded_asset_count > 0).then(|| match selection.assets {
        BackupAssetSelection::ExcludeAll => "all-assets".to_owned(),
        BackupAssetSelection::IncludeUnderThreshold if inherited_omissions => {
            "assets-above-version-1-threshold-or-inherited-omissions".to_owned()
        }
        BackupAssetSelection::IncludeUnderThreshold => {
            "assets-above-version-1-threshold".to_owned()
        }
        BackupAssetSelection::IncludeAll => "inherited-omitted-assets".to_owned(),
    });
    Ok((
        BackupSections {
            results,
            shared_records,
            preferences,
            assets,
        },
        BackupSummary {
            include_results: selection.include_results,
            asset_selection: selection.assets,
            fixed_exclusions: FixedExclusion::ALL.into_iter().collect(),
            excluded_asset_count,
            excluded_asset_ids,
            exclusion_scope,
        },
    ))
}

struct SelectedBackupAssets {
    assets: BTreeMap<String, PortableAsset>,
    excluded_ids: Vec<String>,
    inherited_omissions: bool,
}

fn selected_backup_assets(
    source: &BTreeMap<String, PortableAsset>,
    selection: BackupAssetSelection,
) -> Result<SelectedBackupAssets, AppError> {
    let mut selected = BTreeMap::new();
    let mut excluded_ids = Vec::new();
    let mut inherited_omissions = false;
    for (key, asset) in source {
        if parse_omitted_asset_placeholder(asset)
            .map_err(|error| export_error(&error))?
            .is_some()
        {
            inherited_omissions = true;
            excluded_ids.push(key.clone());
            selected.insert(key.clone(), asset.clone());
            continue;
        }
        let omission = match selection {
            BackupAssetSelection::ExcludeAll => Some(OmittedAssetReason::ExcludeAll),
            BackupAssetSelection::IncludeUnderThreshold
                if asset.bytes.len() > eutheto_types::PORTABLE_LARGE_ASSET_BYTES_V1 =>
            {
                Some(OmittedAssetReason::AboveV1Threshold)
            }
            BackupAssetSelection::IncludeAll | BackupAssetSelection::IncludeUnderThreshold => None,
        };
        let selected_asset = if let Some(reason) = omission {
            excluded_ids.push(key.clone());
            omitted_portable_asset(asset, reason)?
        } else {
            asset.clone()
        };
        selected.insert(key.clone(), selected_asset);
    }
    Ok(SelectedBackupAssets {
        assets: selected,
        excluded_ids,
        inherited_omissions,
    })
}

fn omitted_portable_asset(
    original: &PortableAsset,
    reason: OmittedAssetReason,
) -> Result<PortableAsset, AppError> {
    omitted_asset_placeholder(original, reason).map_err(|error| export_error(&error))
}

fn backup_selection_manifest_extension(summary: &BackupSummary) -> Result<Value, AppError> {
    let asset_selection = match summary.asset_selection {
        BackupAssetSelection::ExcludeAll => PortableBackupAssetSelection::ExcludeAll,
        BackupAssetSelection::IncludeAll => PortableBackupAssetSelection::All,
        BackupAssetSelection::IncludeUnderThreshold => PortableBackupAssetSelection::V1Threshold,
    };
    let (threshold_version, threshold_bytes) =
        if summary.asset_selection == BackupAssetSelection::IncludeUnderThreshold {
            (
                Some(BACKUP_SELECTION_VERSION),
                Some(
                    u64::try_from(eutheto_types::PORTABLE_LARGE_ASSET_BYTES_V1).map_err(|_| {
                        protocol_error(
                            "backup.asset_threshold_overflow",
                            "The portable asset threshold could not be represented.",
                            false,
                        )
                    })?,
                ),
            )
        } else {
            (None, None)
        };
    backup_selection_extension_value(&PortableBackupSelectionMetadata {
        include_results: summary.include_results,
        fixed_exclusions: summary.fixed_exclusions.clone(),
        asset_selection,
        threshold_version,
        threshold_bytes,
        excluded_asset_count: summary.excluded_asset_count,
        excluded_asset_ids: summary.excluded_asset_ids.iter().cloned().collect(),
        scope: BackupSelectionScope::Library,
    })
    .map_err(|error| export_error(&error))
}

fn scenario_backup_selection_manifest_extensions(
    sections: &BackupSections,
) -> Result<BTreeMap<String, Value>, AppError> {
    let mut excluded_asset_ids = BTreeSet::new();
    for (key, asset) in &sections.assets {
        if parse_omitted_asset_placeholder(asset)
            .map_err(|error| export_error(&error))?
            .is_some()
        {
            excluded_asset_ids.insert(key.clone());
        }
    }
    if excluded_asset_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let excluded_asset_count = u64::try_from(excluded_asset_ids.len()).map_err(|_| {
        protocol_error(
            "backup.asset_count_overflow",
            "The excluded asset count could not be represented.",
            false,
        )
    })?;
    let selection = PortableBackupSelectionMetadata {
        include_results: true,
        fixed_exclusions: FixedExclusion::ALL.into_iter().collect(),
        asset_selection: PortableBackupAssetSelection::All,
        threshold_version: None,
        threshold_bytes: None,
        excluded_asset_count,
        excluded_asset_ids,
        scope: BackupSelectionScope::Scenario,
    };
    Ok(BTreeMap::from([(
        BACKUP_SELECTION_EXTENSION.to_owned(),
        backup_selection_extension_value(&selection).map_err(|error| export_error(&error))?,
    )]))
}

fn encode_portable_domain(
    document: &ScenarioDocument,
    registry: &DomainPackRegistry,
) -> Result<PortableDomainDocument, ExportError> {
    let failed = || {
        ExportError::InvalidModel(
            "registered pack rejected portable conversion of this scenario revision".to_owned(),
        )
    };
    let descriptor = registry
        .descriptors()
        .find(|descriptor| descriptor.id == document.domain_pack.id)
        .ok_or_else(failed)?;
    if !descriptor
        .scenario_versions
        .supports(document.domain_pack.schema_version)
    {
        return Err(failed());
    }
    let pack = registry
        .require(&document.domain_pack.id)
        .map_err(|_| failed())?;
    let encoded = pack
        .export_portable(document)
        .map_err(|error| match error {
            DomainPackError::ResourceLimitExceeded => ExportError::ResourceLimitExceeded,
            _ => failed(),
        })?;
    if encoded.pack_id != descriptor.id
        || encoded.schema_version != descriptor.portable_versions.latest
        || !encoded
            .required_capabilities
            .is_subset(&descriptor.portable_capabilities)
    {
        return Err(failed());
    }
    Ok(encoded)
}

fn portable_scenario(
    project: &StoredProject,
    include_project_metadata: bool,
) -> ScenarioSnapshotV1 {
    let mut scenario = ScenarioSnapshotV1::current(
        project.summary.revision,
        project.document.clone(),
        project.portable.required_capabilities.clone(),
    );
    if include_project_metadata {
        scenario.project = Some(PortableProjectMetadata {
            archived_at: project.summary.archived_at,
        });
    }
    scenario.semantic_extensions = project.portable.semantic_extensions.clone();
    scenario.extensions = project.portable.extensions.clone();
    scenario
}

struct LibraryUuidClosure {
    owned_by_scenario: BTreeMap<ScenarioId, BTreeSet<Uuid>>,
    occupied: BTreeSet<Uuid>,
}

fn library_uuid_closure(snapshot: &eutheto_store::LibrarySnapshot) -> LibraryUuidClosure {
    let mut occupied = BTreeSet::new();
    let mut owned_by_scenario: BTreeMap<ScenarioId, BTreeSet<Uuid>> = BTreeMap::new();
    for (identity, scenario_id) in &snapshot.scenario_identity_owners {
        occupied.insert(*identity);
        owned_by_scenario
            .entry(*scenario_id)
            .or_default()
            .insert(*identity);
    }
    occupied.extend(
        snapshot
            .scenario_revision_high_water
            .keys()
            .map(|scenario_id| scenario_id.as_uuid()),
    );
    for project in &snapshot.projects {
        let scenario_id = project.document.scenario_id;
        let owned = collect_scenario_owned_uuids(&portable_scenario(project, true));
        occupied.extend(owned.iter().copied());
        owned_by_scenario
            .entry(scenario_id)
            .or_default()
            .extend(owned);
    }
    for historical in &snapshot.scenario_revisions {
        let scenario_id = historical.scenario.document.scenario_id;
        let owned = collect_scenario_owned_uuids(&historical.scenario);
        occupied.extend(owned.iter().copied());
        owned_by_scenario
            .entry(scenario_id)
            .or_default()
            .extend(owned);
    }
    occupied.extend(snapshot.supplemental_identity_owners.keys().copied());
    LibraryUuidClosure {
        owned_by_scenario,
        occupied,
    }
}

fn scenario_export_sections(
    library: &eutheto_store::LibrarySnapshot,
    scenario: &ScenarioSnapshotV1,
) -> Result<BackupSections, AppError> {
    let scenario_id = scenario.document.scenario_id;
    let mut results = BTreeMap::new();
    for value in portable_json_section(&library.sections.results, None)?.into_values() {
        let dependency = extract_result_dependency(&value).map_err(portable_reference_error)?;
        if dependency.scenario_id == scenario_id {
            let result_id = extract_result_id(&value).map_err(portable_reference_error)?;
            results.insert(result_id.to_string(), value);
        }
    }

    let mut shared_records = portable_json_section(&library.sections.shared_records, None)?;
    retain_scenario_references(&mut shared_records, scenario_id)?;
    let mut preferences = portable_json_section(
        &library.sections.preferences,
        Some(PORTABLE_SETTINGS_SECTION),
    )?;
    retain_scenario_references(&mut preferences, scenario_id)?;

    let mut asset_references = BTreeSet::new();
    collect_asset_references(
        &serde_json::to_value(scenario).map_err(|_| {
            protocol_error(
                "portable.scenario_reference_invalid",
                "The scenario could not be inspected for portable asset references.",
                false,
            )
        })?,
        &mut asset_references,
    )?;
    for historical in library
        .scenario_revisions
        .iter()
        .filter(|stored| stored.scenario.document.scenario_id == scenario_id)
    {
        collect_asset_references(
            &serde_json::to_value(&historical.scenario).map_err(|_| {
                protocol_error(
                    "portable.scenario_reference_invalid",
                    "A retained scenario revision could not be inspected for portable asset references.",
                    false,
                )
            })?,
            &mut asset_references,
        )?;
    }
    for value in results
        .values()
        .chain(shared_records.values())
        .chain(preferences.values())
    {
        collect_asset_references(value, &mut asset_references)?;
    }
    let assets = library
        .sections
        .assets
        .iter()
        .filter(|(key, _)| asset_references.contains(*key))
        .map(|(key, asset)| (key.clone(), asset.clone()))
        .collect();
    Ok(BackupSections {
        results,
        shared_records,
        preferences,
        assets,
    })
}

fn retain_scenario_references(
    entries: &mut BTreeMap<String, Value>,
    scenario_id: ScenarioId,
) -> Result<(), AppError> {
    let mut retained = BTreeMap::new();
    for (key, mut value) in std::mem::take(entries) {
        if extract_scenario_references(&value)
            .map_err(portable_reference_error)?
            .contains(&scenario_id)
        {
            scope_scenario_references(&mut value, scenario_id);
            retained.insert(key, value);
        }
    }
    *entries = retained;
    Ok(())
}

fn scope_scenario_references(value: &mut Value, selected: ScenarioId) {
    let selected = selected.to_string();
    scope_scenario_reference_string(value, &selected);
}

fn scope_scenario_reference_string(value: &mut Value, selected: &str) {
    match value {
        Value::Array(values) => {
            for value in values {
                scope_scenario_reference_string(value, selected);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let normalized = key
                    .chars()
                    .filter(|character| !matches!(character, '-' | '_'))
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if normalized == "scenarioids" {
                    if let Some(ids) = value.as_array_mut() {
                        ids.retain(|id| id.as_str() == Some(selected));
                    }
                } else if normalized != "scenarioid" {
                    scope_scenario_reference_string(value, selected);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn collect_asset_references(
    value: &Value,
    references: &mut BTreeSet<String>,
) -> Result<(), AppError> {
    references.extend(extract_asset_references(value).map_err(portable_reference_error)?);
    Ok(())
}

fn portable_reference_error(_: eutheto_types::PortableReferenceError) -> AppError {
    protocol_error(
        "portable.stored_reference_invalid",
        "A retained portable record has an invalid declared scenario reference.",
        false,
    )
}

fn portable_json_section(
    entries: &BTreeMap<String, Vec<u8>>,
    reserved: Option<&str>,
) -> Result<BTreeMap<String, Value>, AppError> {
    let mut values = BTreeMap::new();
    for (stored_name, bytes) in entries {
        let name = stored_name.strip_suffix(".json").unwrap_or(stored_name);
        if reserved == Some(name) {
            continue;
        }
        let value = serde_json::from_slice(bytes).map_err(|_| {
            protocol_error(
                "portable.stored_section_invalid",
                "A retained portable section could not be backed up safely.",
                false,
            )
        })?;
        if values.insert(name.to_owned(), value).is_some() {
            return Err(protocol_error(
                "portable.stored_section_duplicate",
                "A retained portable section has a duplicate logical name.",
                false,
            ));
        }
    }
    Ok(values)
}

fn take_portable_settings(
    preferences: &mut BTreeMap<String, Vec<u8>>,
    mode: RestoreMode,
) -> Result<BTreeMap<String, AppSetting<Value>>, AppError> {
    let bytes = preferences
        .remove(&format!("{PORTABLE_SETTINGS_SECTION}.json"))
        .or_else(|| preferences.remove(PORTABLE_SETTINGS_SECTION));
    let Some(bytes) = bytes else {
        return Ok(BTreeMap::new());
    };
    let Value::Object(entries) = serde_json::from_slice(&bytes).map_err(|_| {
        validation_error(
            "settings.import_invalid",
            "/preferences/application-settings",
            "Imported application settings must be a JSON object.",
        )
    })?
    else {
        return Err(validation_error(
            "settings.import_invalid",
            "/preferences/application-settings",
            "Imported application settings must be a JSON object.",
        ));
    };
    let mut settings = BTreeMap::new();
    for (key, entry) in entries {
        let Value::Object(mut fields) = entry else {
            return Err(validation_error(
                "settings.import_invalid",
                "/preferences/application-settings",
                "Each imported setting must contain a value and update timestamp.",
            ));
        };
        if fields.len() != 2 || !fields.contains_key("value") || !fields.contains_key("updatedAt") {
            return Err(validation_error(
                "settings.import_invalid",
                "/preferences/application-settings",
                "Each imported setting must contain only value and updatedAt.",
            ));
        }
        let Some(value) = fields.remove("value") else {
            return Err(validation_error(
                "settings.import_invalid",
                "/preferences/application-settings",
                "An imported setting value is missing.",
            ));
        };
        validate_app_setting(&key, &value)?;
        let updated_at = fields
            .remove("updatedAt")
            .and_then(|value| value.as_str().map(str::to_owned))
            .and_then(|value| Rfc3339Timestamp::parse(&value).ok())
            .ok_or_else(|| {
                validation_error(
                    "settings.import_invalid",
                    "/preferences/application-settings",
                    "An imported setting update timestamp is invalid.",
                )
            })?;
        settings.insert(key, AppSetting { value, updated_at });
    }
    if mode == RestoreMode::ImportScenario {
        return Err(validation_error(
            "settings.import_mode_invalid",
            "/restoreMode",
            "Application settings may only be restored from a full backup.",
        ));
    }
    Ok(settings)
}

fn validate_app_setting_key(key: &str) -> Result<(), AppError> {
    if matches!(key, "appearance" | "locale" | "units") {
        Ok(())
    } else {
        Err(validation_error(
            "settings.key_unsupported",
            "/key",
            "Only appearance, locale, and units are supported application settings.",
        ))
    }
}

fn validate_app_setting(key: &str, value: &Value) -> Result<(), AppError> {
    validate_app_setting_key(key)?;
    let valid = match key {
        "appearance" => value.as_object().is_some_and(|appearance| {
            appearance
                .keys()
                .all(|name| matches!(name.as_str(), "theme" | "reducedMotion"))
                && appearance
                    .get("theme")
                    .is_none_or(|theme| matches!(theme.as_str(), Some("system" | "light" | "dark")))
                && appearance
                    .get("reducedMotion")
                    .is_none_or(Value::is_boolean)
        }),
        "locale" => value.as_str().is_some_and(valid_locale_setting),
        "units" => matches!(value.as_str(), Some("metric" | "us-customary")),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(validation_error(
            "settings.value_invalid",
            "/value",
            "The setting value does not match the documented nonsecret schema.",
        ))
    }
}

fn valid_locale_setting(locale: &str) -> bool {
    !locale.is_empty()
        && locale.len() <= 64
        && locale.is_ascii()
        && locale.split('-').all(|segment| {
            !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn history_apply(
    history: HistoryCommand,
    current_revision: Revision,
    source: CommandSource,
    registry: &DomainPackRegistry,
    cancellation: &CancellationToken,
) -> Result<(ScenarioDocument, CommandResult), StoreError> {
    if history.entry.command_type == eutheto_store::NATIVE_PRIVATE_HISTORY_COMMAND_TYPE {
        return apply_private_history(history, current_revision, source, registry, cancellation);
    }
    let envelope = CommandEnvelope {
        command_id: history.entry.id,
        scenario_id: history.document.scenario_id,
        expected_revision: current_revision,
        actor: ActorRef {
            actor_id: None,
            display_name: "History".to_owned(),
        },
        source,
        command: serde_json::from_value(history.command).map_err(StoreError::Json)?,
    };
    ensure_supported_document(&history.document, registry)?;
    let mut applied = apply_command_with_registry(
        &history.document,
        current_revision,
        &envelope,
        registry,
        cancellation,
    )
    .map_err(|error| command_store_error(&error, Some(&envelope.command)))?;
    applied.document.metadata.updated_at = history.target_document_updated_at;
    Ok((applied.document, applied.result))
}

fn available_pack<'a>(
    document: &ScenarioDocument,
    registry: &'a DomainPackRegistry,
) -> Option<&'a dyn eutheto_domain_api::DomainPack> {
    let supports_schema = registry.descriptors().any(|descriptor| {
        descriptor.id == document.domain_pack.id
            && descriptor
                .scenario_versions
                .supports(document.domain_pack.schema_version)
    });
    supports_schema
        .then(|| registry.require(&document.domain_pack.id).ok())
        .flatten()
}

fn validate(document: &ScenarioDocument, registry: &DomainPackRegistry) -> ValidationReport {
    let Some(pack) = available_pack(document, registry) else {
        return ValidationReport {
            issues: vec![ValidationIssue {
                code: "domain_pack.unsupported".to_owned(),
                severity: ValidationSeverity::Error,
                message: "The scenario domain pack is not available in this application phase."
                    .to_owned(),
                field_path: Some("/domainPack".to_owned()),
                resource: Some(ResourceRef::Pack(document.domain_pack.id.clone())),
            }],
        };
    };
    ValidationReport {
        issues: pack.validate_fast(document).issues,
    }
}

fn ensure_supported_document(
    document: &ScenarioDocument,
    registry: &DomainPackRegistry,
) -> Result<(), StoreError> {
    if available_pack(document, registry).is_some() {
        Ok(())
    } else {
        Err(StoreError::CommandApplication {
            code: "command.unsupported".to_owned(),
            message: format!(
                "domain pack {} schema {} is unavailable",
                document.domain_pack.id, document.domain_pack.schema_version
            ),
            field_path: None,
        })
    }
}

fn scenario_view(project: StoredProject, registry: &DomainPackRegistry) -> ScenarioViewDto {
    let validation = validate(&project.document, registry);
    ScenarioViewDto {
        revision: project.summary.revision,
        document: project.document,
        validation,
    }
}

fn project_summary(project: eutheto_store::ProjectSummary) -> ProjectListItemV1 {
    ProjectListItemV1 {
        schema_version: PROJECT_LIST_SCHEMA_VERSION,
        scenario_id: project.id,
        title: project.title,
        domain_pack_id: project.domain_pack_id,
        revision: project.revision,
        updated_at: project.updated_at,
        archived: project.archived_at.is_some(),
        last_opened_at: project.last_opened_at,
    }
}

fn project_metadata(project: &StoredProject) -> ProjectMetadataDto {
    ProjectMetadataDto {
        scenario_id: project.document.scenario_id,
        title: project.document.metadata.title.clone(),
        description: project.document.metadata.description.clone(),
        domain_pack: project.document.domain_pack.clone(),
        revision: project.summary.revision,
        created_at: project.document.metadata.created_at,
        updated_at: project.document.metadata.updated_at,
        archived_at: project.summary.archived_at,
    }
}

fn solution_summary(summary: &StoredAcceptedResultSummary) -> SolutionSummaryDtoV1 {
    SolutionSummaryDtoV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        result: summary.result.clone(),
        run_id: summary.run_id,
        scenario_id: summary.scenario_id,
        scenario_revision: summary.scenario_revision,
        current_revision: summary.current_revision,
        stale: summary.stale,
        selected: summary.selected,
        status: summary.status,
        score: summary.score.clone(),
        verification_checksum: summary.verification_checksum.clone(),
        finished_at: summary.finished_at,
    }
}

fn solution_revision(stored: &StoredAcceptedResultV2) -> Result<Revision, AppError> {
    Revision::try_new(stored.portable.scenario_revision).map_err(|_| solution_contract_error())
}

fn solution_detail(
    stored: &StoredAcceptedResultV2,
    current_revision: Revision,
    selected: bool,
) -> Result<SolutionDetailDtoV1, AppError> {
    Ok(SolutionDetailDtoV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        scenario_id: stored.portable.scenario_id,
        current_revision,
        scenario_revision: solution_revision(stored)?,
        selected,
        result: stored.portable.accepted_result.clone(),
    })
}

fn accepted_reference(accepted: &AcceptedResult) -> Result<AcceptedResultRefV1, AppError> {
    AcceptedResultRefV1::from_result(accepted).map_err(|_| solution_contract_error())
}
fn accepted_view_fingerprint(
    accepted: &AcceptedResult,
    query: &AcceptedDomainQueryV1,
) -> Result<[u8; 32], AppError> {
    const DOMAIN: &[u8] = b"eutheto/accepted-result-view/v1\0";
    let canonical = eutheto_export::canonical_json(&(
        accepted.checksum.as_str(),
        query.view_id.as_str(),
        &query.parameters,
    ))
    .map_err(|error| export_error(&error))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(&canonical);
    Ok(*hasher.finalize().as_bytes())
}
fn ensure_counterfactual_schema(schema_version: u32) -> Result<(), AppError> {
    if schema_version == COUNTERFACTUAL_API_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(unsupported_solution_schema())
    }
}

fn ensure_solution_schema(schema_version: u32) -> Result<(), AppError> {
    if schema_version == SOLUTION_API_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(unsupported_solution_schema())
    }
}
fn ensure_adopt_base_schema(schema_version: u32) -> Result<(), AppError> {
    if schema_version == SOLUTION_ADOPT_BASE_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(unsupported_solution_schema())
    }
}

fn selected_workforce_assignments(
    accepted: &AcceptedResult,
) -> Result<Vec<AssignmentPair>, AppError> {
    let mut decoded = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for assignment in &accepted.solution.assignments {
        let (pair, is_selected) = decode_workforce_assignment(assignment)
            .map_err(|_| solution_verification_mismatch())?;
        if !decoded.insert(pair) {
            return Err(solution_verification_mismatch());
        }
        if is_selected {
            selected.insert(pair);
        }
    }
    Ok(selected.into_iter().collect())
}

fn workforce_base_schedule(document: &ScenarioDocument) -> Result<Option<BaseSchedule>, AppError> {
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
        return Err(unsupported_project_pack(&document.domain_pack));
    }
    let mut base = None;
    for (map_id, value) in &document.domain.entities {
        if value.get("kind").and_then(Value::as_str) != Some("baseSchedule") {
            continue;
        }
        let entity: WorkforceEntity = serde_json::from_value(value.clone()).map_err(|_| {
            validation_error(
                "solution.base_adoption_invalid",
                "/domain/entities",
                "The current Workforce base schedule is malformed.",
            )
        })?;
        let WorkforceEntity::BaseSchedule(candidate) = entity else {
            return Err(validation_error(
                "solution.base_adoption_invalid",
                "/domain/entities",
                "The current Workforce base schedule has an invalid kind.",
            ));
        };
        if candidate.id != *map_id {
            return Err(validation_error(
                "solution.base_adoption_invalid",
                "/domain/entities",
                "The current Workforce base schedule identity is inconsistent.",
            ));
        }
        if base.replace(candidate).is_some() {
            return Err(validation_error(
                "solution.base_adoption_invalid",
                "/domain/entities",
                "The current Workforce document contains more than one base schedule.",
            ));
        }
    }
    Ok(base)
}

fn unsupported_solution_schema() -> AppError {
    validation_error(
        "solution.schema_version_unsupported",
        "/schemaVersion",
        "The solution request schema version is not supported.",
    )
}

fn ensure_solution_scenario(
    stored: &StoredAcceptedResultV2,
    scenario_id: ScenarioId,
) -> Result<(), AppError> {
    if stored.portable.scenario_id == scenario_id
        && stored.document.scenario_id == scenario_id
        && stored.portable.accepted_result.solution.scenario_id == scenario_id
    {
        Ok(())
    } else {
        Err(solution_reference_error("/scenarioId"))
    }
}

fn assignment_evidence(
    accepted: &AcceptedResult,
    assignment_id: &eutheto_domain_ir::DomainAssignmentId,
) -> Result<AssignmentEvidenceV1, AppError> {
    let assignment = accepted
        .solution
        .assignments
        .iter()
        .find(|assignment| &assignment.id == assignment_id)
        .cloned()
        .ok_or(AppError::NotFound(ResourceRef::Solution(
            accepted.solution.solution_id,
        )))?;
    let related_rules = accepted
        .verification
        .required_rule_results
        .iter()
        .filter(|rule| {
            rule.affected_entities.contains(&assignment.entity)
                || rule
                    .evidence
                    .iter()
                    .any(|id| assignment.evidence.contains(id))
        })
        .cloned()
        .collect();
    Ok(AssignmentEvidenceV1 {
        assignment,
        related_rules,
        score_contributions: Vec::new(),
        metrics: BTreeMap::new(),
        lock_state: None,
    })
}
fn validation_issue_evidence(
    issue: &ValidationIssue,
    expected_issue_id: &eutheto_domain_ir::VerificationIssueId,
) -> Result<ValidationIssueEvidenceV1, AppError> {
    let issue_id = validation_issue_identity(issue).map_err(|_| resource_limit_error())?;
    if &issue_id != expected_issue_id {
        return Err(explanation_unavailable("validation"));
    }
    let severity = match issue.severity {
        ValidationSeverity::Error => ExplanationValidationSeverity::MustFix,
        ValidationSeverity::Warning => ExplanationValidationSeverity::ReviewSuggested,
        ValidationSeverity::Info => ExplanationValidationSeverity::Information,
    };
    Ok(ValidationIssueEvidenceV1 {
        issue_id,
        severity,
        message_key: issue.code.clone(),
        parameters: BTreeMap::new(),
        // Native field/resource provenance stays with the pack renderer; do not invent typed IR
        // facts from a generic native issue.
        field_path: None,
        entity: None,
        rule_id: None,
    })
}

struct Blake3Writer<'a> {
    hasher: &'a mut blake3::Hasher,
    control: &'a OperationControl,
    deadline: tokio::time::Instant,
    unchecked_bytes: usize,
}

impl Write for Blake3Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.unchecked_bytes = self.unchecked_bytes.saturating_add(bytes.len());
        if self.unchecked_bytes >= 16 * 1024 {
            self.unchecked_bytes = 0;
            check_accepted_hash(self.control, self.deadline).map_err(|_| {
                io::Error::new(io::ErrorKind::Interrupted, "accepted hash interrupted")
            })?;
        }
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn check_accepted_hash(
    control: &OperationControl,
    deadline: tokio::time::Instant,
) -> Result<(), OperationInterruption> {
    control.check()?;
    if tokio::time::Instant::now() >= deadline {
        Err(OperationInterruption::DeadlineExceeded)
    } else {
        Ok(())
    }
}

/// Hashes the complete captured source tuple, not any persisted checksum field alone.
fn accepted_content_hash(
    stored: &StoredAcceptedResultV2,
    control: &OperationControl,
    deadline: tokio::time::Instant,
) -> Result<AcceptedContentHash, AppError> {
    const DOMAIN: &[u8] = b"eutheto/accepted-source-tuple/v1\0";
    check_accepted_hash(control, deadline).map_err(operation_interrupted)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    {
        let mut writer = Blake3Writer {
            hasher: &mut hasher,
            control,
            deadline,
            unchecked_bytes: 0,
        };
        let source_tuple = (
            &stored.document,
            &stored.portable,
            &stored.portable.scenario_id,
            stored.portable.scenario_revision,
        );
        serde_json::to_writer(&mut writer, &source_tuple).map_err(|_| {
            check_accepted_hash(control, deadline)
                .err()
                .map_or_else(solution_contract_error, operation_interrupted)
        })?;
    }
    check_accepted_hash(control, deadline).map_err(operation_interrupted)?;
    Ok(*hasher.finalize().as_bytes())
}

fn compare_stored_solutions(
    base: &StoredAcceptedResultV2,
    candidate: &StoredAcceptedResultV2,
    registry: &DomainPackRegistry,
    control: &OperationControl,
) -> Result<SolutionComparisonV2, AppError> {
    control.check().map_err(operation_interrupted)?;
    let prepared = prepare_comparison_v2(&base.portable, &candidate.portable, Some(control))
        .map_err(|error| comparison_error(&error))?;
    let pack = available_pack(&base.document, registry)
        .ok_or_else(|| unsupported_project_pack(&base.document.domain_pack))?;
    let mut enrichment = pack
        .comparison_enrichment(
            &base.document,
            &base.portable,
            &candidate.document,
            &candidate.portable,
            &prepared,
            Some(control),
        )
        .map_err(|error| domain_interruption(&error).unwrap_or_else(comparison_invalid))?;
    if enrichment.scoring_contexts.is_some() {
        let base_policy = current_comparison_policy(base, pack, control)?;
        let admitted = if let Some(base_policy) = base_policy.as_ref()
            && base_policy == &base.portable.run_input.objective_policy_hash
        {
            if same_comparison_policy_source(base, candidate) {
                base_policy == &candidate.portable.run_input.objective_policy_hash
            } else {
                current_comparison_policy(candidate, pack, control)?.as_deref()
                    == Some(candidate.portable.run_input.objective_policy_hash.as_str())
            }
        } else {
            false
        };
        if !admitted {
            enrichment.scoring_contexts = None;
        }
    }
    prepared
        .finish(enrichment)
        .map_err(|error| comparison_error(&error))
}

fn comparison_error(error: &eutheto_domain_ir::ComparisonError) -> AppError {
    match error {
        eutheto_domain_ir::ComparisonError::Interrupted(reason) => operation_interrupted(*reason),
        eutheto_domain_ir::ComparisonError::InvalidComparison(
            eutheto_domain_ir::DomainContractError::LimitExceeded(_),
        )
        | eutheto_domain_ir::ComparisonError::InvalidAcceptedResult {
            source: eutheto_domain_ir::DomainContractError::LimitExceeded(_),
            ..
        } => resource_limit_error(),
        _ => comparison_invalid(),
    }
}

fn same_comparison_policy_source(
    base: &StoredAcceptedResultV2,
    candidate: &StoredAcceptedResultV2,
) -> bool {
    let before = &base.portable.run_input;
    let after = &candidate.portable.run_input;
    // Both hashes already bind freshly verified exact original documents.
    before.snapshot_document_hash == after.snapshot_document_hash
        && before.scenario_revision == after.scenario_revision
        && before.objective_context == after.objective_context
        && eutheto_planning_ir::PlanningIrLimitsV1::DEFAULT
            .tightened_by(before.solve_options.resource_limits)
            == eutheto_planning_ir::PlanningIrLimitsV1::DEFAULT
                .tightened_by(after.solve_options.resource_limits)
}

fn current_comparison_policy(
    source: &StoredAcceptedResultV2,
    pack: &dyn eutheto_domain_api::DomainPack,
    control: &OperationControl,
) -> Result<Option<String>, AppError> {
    control.check().map_err(operation_interrupted)?;
    let input = &source.portable.run_input;
    let Some(objective_context) = input.objective_context.as_ref() else {
        return Ok(None);
    };
    let limits = eutheto_planning_ir::PlanningIrLimitsV1::DEFAULT
        .tightened_by(input.solve_options.resource_limits);
    let problem = match pack.compile(
        &source.document,
        &eutheto_domain_api::CompileContext {
            objective_context: Some(objective_context.clone()),
            scenario_revision: input.scenario_revision,
            semantic_metadata: BTreeMap::new(),
            control: control.clone(),
            planning_limits: limits,
        },
    ) {
        Ok(problem) => problem,
        Err(error) => return domain_interruption(&error).map_or(Ok(None), Err),
    };
    control.check().map_err(operation_interrupted)?;
    if problem.schema_version != input.planning_ir_schema_version
        || problem.metadata.pack_id != input.pack_id
        || problem.metadata.scenario_id != input.scenario_id
        || problem.metadata.scenario_revision != input.scenario_revision
        || problem.metadata.compiler_version != input.compiler_version
        || problem.metadata.projection_version
            != source.portable.accepted_result.solution.projection_version
    {
        return Ok(None);
    }
    // This is current per-source policy consistency, not historical worker authentication.
    let policy = eutheto_planning_ir::canonical_objective_policy_hash(&problem, limits);
    control.check().map_err(operation_interrupted)?;
    match policy {
        Ok(hash) => Ok(Some(hash)),
        Err(eutheto_planning_ir::CanonicalError::Validation(error))
            if error.code == eutheto_planning_ir::ValidationCode::LimitExceeded =>
        {
            Err(resource_limit_error())
        }
        Err(_) => Ok(None),
    }
}

fn comparison_invalid() -> AppError {
    validation_error(
        "solution.comparison_invalid",
        "/request",
        "The selected accepted solutions cannot be compared.",
    )
}

fn reverify_accepted_solution(
    stored: &StoredAcceptedResultV2,
    registry: &DomainPackRegistry,
    control: &OperationControl,
) -> Result<VerificationReport, AppError> {
    reverify_portable_solution(
        &stored.document,
        stored.portable.scenario_revision,
        &stored.portable,
        registry,
        control,
    )
}

fn reverify_portable_solution(
    document: &ScenarioDocument,
    revision: u64,
    portable: &PortableAcceptedResult,
    registry: &DomainPackRegistry,
    control: &OperationControl,
) -> Result<VerificationReport, AppError> {
    control.check().map_err(operation_interrupted)?;
    portable
        .validate()
        .map_err(|_| solution_verification_mismatch())?;
    crate::native_observations::validate_fingerprint(&portable.run_input, &portable.run_manifest)
        .map_err(|_| solution_verification_mismatch())?;
    reverify_original_domain_result(
        document,
        revision,
        &portable.run_input,
        &portable.accepted_result,
        registry,
        control,
    )
}

fn reverify_checkpoint_solution(
    document: &ScenarioDocument,
    checkpoint: &VerifiedCheckpoint,
    registry: &DomainPackRegistry,
    control: &OperationControl,
) -> Result<VerificationReport, AppError> {
    control.check().map_err(operation_interrupted)?;
    checkpoint
        .validate()
        .map_err(|_| solution_verification_mismatch())?;
    reverify_original_domain_result(
        document,
        checkpoint.run_input.scenario_revision,
        &checkpoint.run_input,
        &checkpoint.accepted_result,
        registry,
        control,
    )
}

fn reverify_original_domain_result(
    document: &ScenarioDocument,
    revision: u64,
    input: &RunInput,
    accepted: &AcceptedResult,
    registry: &DomainPackRegistry,
    control: &OperationControl,
) -> Result<VerificationReport, AppError> {
    control.check().map_err(operation_interrupted)?;
    let solution = &accepted.solution;
    let pack = available_pack(document, registry)
        .ok_or_else(|| unsupported_project_pack(&document.domain_pack))?;
    let document_hash = serde_json::to_vec(document)
        .map(|bytes| blake3_hex(&bytes))
        .map_err(|_| solution_contract_error())?;
    if document_hash != input.snapshot_document_hash
        || document_hash != accepted.verification.document_hash
        || solution.pack_id != document.domain_pack.id
        || input.pack_schema_version != document.domain_pack.schema_version
        || solution.scenario_id != document.scenario_id
        || solution.scenario_revision != revision
        || input.scenario_id != document.scenario_id
        || input.scenario_revision != revision
    {
        return Err(solution_verification_mismatch());
    }
    let scope = pack
        .verification_scope(document, solution.scenario_revision, control)
        .map_err(|error| {
            domain_interruption(&error).unwrap_or_else(solution_verification_failed)
        })?;
    if scope.checksum != accepted.verification.verification_scope_checksum {
        return Err(solution_verification_mismatch());
    }
    let context = VerificationContextV1::new(
        solution.scenario_id,
        solution.scenario_revision,
        document_hash,
        input.model_hash.clone(),
        solution
            .canonical_hash()
            .map_err(|_| solution_verification_mismatch())?,
        scope.checksum,
    )
    .map_err(|_| solution_verification_mismatch())?;
    let authoritative_score = pack
        .score(
            document,
            solution,
            input.objective_context.as_ref(),
            control,
        )
        .map_err(|error| {
            domain_interruption(&error).unwrap_or_else(solution_verification_failed)
        })?;
    let report = pack
        .verify(
            document,
            solution,
            input.objective_context.as_ref(),
            &context,
            &authoritative_score,
            control,
        )
        .map_err(|error| {
            domain_interruption(&error).unwrap_or_else(solution_verification_failed)
        })?;
    report
        .validate()
        .map_err(|_| solution_verification_mismatch())?;
    if report != accepted.verification
        || report.checksum != accepted.verification.checksum
        || AcceptedResult::new(solution.clone(), report.clone())
            .map_err(|_| solution_verification_mismatch())?
            .checksum
            != accepted.checksum
    {
        return Err(solution_verification_mismatch());
    }
    control.check().map_err(operation_interrupted)?;
    Ok(report)
}

pub(super) fn same_accepted_original(
    left: &RunBaselineOriginal,
    right: &PortableAcceptedResult,
) -> bool {
    // Source provenance only: callers must separately bind any recorded carrier pair census.
    // Embedded and run-only carrier entity IDs are not part of native source identity.
    left.accepted_result.solution_id == right.result_id
        && left.accepted_result.result_checksum == right.accepted_result.checksum
        && left.source_scenario_id == right.scenario_id
        && left.source_revision == right.scenario_revision
        && left.source_document_hash == right.run_input.snapshot_document_hash
        && left.source_run_id == right.run_input.run_id
        && left.source_run_input_checksum == right.run_input.checksum
        && left.source_run_manifest_checksum == right.run_manifest.checksum
}

async fn wait_for_cancellation(token: &CancellationToken) {
    while !token.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn operation_interrupted(reason: OperationInterruption) -> AppError {
    match reason {
        OperationInterruption::Cancelled => {
            protocol_error("operation.cancelled", "The operation was cancelled.", false)
        }
        OperationInterruption::DeadlineExceeded => protocol_error(
            "operation.deadline_exceeded",
            "The operation deadline was reached.",
            false,
        ),
    }
}

pub(crate) fn resource_limit_error() -> AppError {
    protocol_error(
        "operation.resource_limit",
        "The domain operation could not complete within its resource limit.",
        false,
    )
}

fn domain_interruption(error: &DomainPackError) -> Option<AppError> {
    match error {
        DomainPackError::Cancelled => Some(operation_interrupted(OperationInterruption::Cancelled)),
        DomainPackError::BudgetExpired => Some(operation_interrupted(
            OperationInterruption::DeadlineExceeded,
        )),
        DomainPackError::ResourceLimitExceeded => Some(resource_limit_error()),
        _ => None,
    }
}

fn solution_view_error(error: &DomainPackError) -> AppError {
    if let Some(interruption) = domain_interruption(error) {
        return interruption;
    }
    match error {
        DomainPackError::UnsupportedExplanationCapability(_) => explanation_unavailable("view"),
        _ => validation_error(
            "solution.view_invalid",
            "/viewId",
            "The selected result view is unavailable for this accepted solution.",
        ),
    }
}

fn explanation_render_request(
    payload: ExplanationEvidencePayloadV2,
) -> Result<EvidenceRenderRequestV2, AppError> {
    let evidence = ExplanationEvidenceV2::new(payload).map_err(|_| solution_contract_error())?;
    EvidenceRenderRequestV2::new(evidence).map_err(|_| solution_contract_error())
}

fn explanation_render_error(error: &DomainPackError) -> AppError {
    match error {
        DomainPackError::ResourceLimitExceeded => resource_limit_error(),
        DomainPackError::UnsupportedExplanationCapability(capability) => {
            AppError::Unsupported(UnsupportedFeature {
                code: "solution.explanation_capability_unavailable".to_owned(),
                capability: format!("{capability:?} explanation"),
            })
        }
        _ => protocol_error(
            "solution.explanation_render_failed",
            "The selected evidence could not be rendered safely.",
            false,
        ),
    }
}

fn explanation_unavailable(capability: &str) -> AppError {
    AppError::Unsupported(UnsupportedFeature {
        code: format!("solution.explanation_{capability}_unavailable"),
        capability: format!("{capability} evidence"),
    })
}

fn solution_reference_error(path: &str) -> AppError {
    validation_error(
        "solution.reference_mismatch",
        path,
        "The selected solution reference does not match retained local authority.",
    )
}

pub(crate) fn solution_contract_error() -> AppError {
    protocol_error(
        "solution.authority_invalid",
        "The retained accepted-solution authority could not be used safely.",
        false,
    )
}

fn solution_verification_failed() -> AppError {
    AppError::Verification(VerificationFailure {
        code: "solution.reverification_failed".to_owned(),
        message: "Independent verification could not be completed safely.".to_owned(),
        retryable: false,
        diagnostic_id: None,
        known_alarm: None,
    })
}

pub(crate) fn solution_verification_mismatch() -> AppError {
    AppError::Verification(VerificationFailure {
        code: "solution.reverification_mismatch".to_owned(),
        message: "Fresh independent verification did not match the retained accepted result."
            .to_owned(),
        retryable: false,
        diagnostic_id: None,
        known_alarm: None,
    })
}

fn application_metadata() -> ApplicationMetadata {
    ApplicationMetadata {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn directory_availability(path: Option<&Path>) -> DirectoryAvailabilityLabel {
    match path.and_then(|path| std::fs::metadata(path).ok()) {
        Some(metadata) if metadata.is_dir() => DirectoryAvailabilityLabel::Available,
        _ => DirectoryAvailabilityLabel::Unavailable,
    }
}

fn normalize_workforce_manual_copy(
    original: &ScenarioDocument,
    copied: &mut ScenarioDocument,
    mapping: &BTreeMap<Uuid, Uuid>,
    reference_remap: &BTreeMap<Uuid, Uuid>,
    control: Option<&OperationControl>,
) -> Result<(), StoreError> {
    if original.domain_pack.id.as_str() == WORKFORCE_PACK_ID {
        if copied.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
            return Err(StoreError::InvalidDuplicateMapping(
                "copied domain pack does not match its source".to_owned(),
            ));
        }
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            original,
            copied,
            mapping,
            reference_remap,
            control,
        )
        .map_err(diagnostic_actions::pack_store_error)
    } else if copied.domain_pack.id.as_str() == WORKFORCE_PACK_ID {
        Err(StoreError::InvalidDuplicateMapping(
            "copied domain pack does not match its source".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn command_store_error(error: &CommandError, command: Option<&ScenarioCommand>) -> StoreError {
    if matches!(error, CommandError::Cancelled) {
        return StoreError::OperationCancelled;
    }
    if let CommandError::Conflict { expected, actual } = error {
        return StoreError::Conflict {
            expected: Revision::new(*expected),
            actual: Revision::new(*actual),
        };
    }
    StoreError::CommandApplication {
        code: error.code().to_owned(),
        message: error.to_string(),
        field_path: match error {
            CommandError::InvalidDomainPayload { path, .. }
                if matches!(command, Some(ScenarioCommand::ApplyDomainCommand(_)))
                    && path.starts_with("/payload/") =>
            {
                Some(path.clone())
            }
            _ => None,
        },
    }
}

fn store_error(error: StoreError) -> AppError {
    match error {
        StoreError::OperationCancelled => {
            protocol_error("operation.cancelled", "The operation was cancelled.", false)
        }
        StoreError::ScenarioNotFound(id) => AppError::NotFound(ResourceRef::Scenario(id)),
        StoreError::AcceptedResultNotFound(id) => AppError::NotFound(ResourceRef::Solution(id)),
        StoreError::AcceptedResultChanged(_) => solution_verification_mismatch(),
        StoreError::CounterfactualJobNotFound(_) => validation_error(
            "solution.counterfactual_not_found",
            "/request/subject/jobId",
            "The selected completed counterfactual was not found.",
        ),
        StoreError::CounterfactualJobListLimit { .. } => validation_error(
            "solution.counterfactual_page_limit",
            "/limit",
            "Diagnostic history pages must contain between 1 and 32 jobs.",
        ),
        StoreError::InvalidCounterfactualCursor(_) => validation_error(
            "solution.counterfactual_page_cursor",
            "/before",
            "The diagnostic history continuation is invalid for this scenario.",
        ),
        StoreError::Conflict { expected, actual }
        | StoreError::LibraryConflict { expected, actual } => AppError::Conflict {
            expected_revision: expected,
            actual_revision: actual,
        },
        StoreError::SolveRequestIdConflict { .. } => validation_error(
            "solve.request_id_conflict",
            "/requestId",
            "This request identity already belongs to different solve semantics.",
        ),
        StoreError::ScenarioAlreadyExists(_) => validation_error(
            "project.scenario_already_exists",
            "/scenarioId",
            "A project with this scenario identity already exists.",
        ),
        StoreError::CommandApplication {
            code,
            message,
            field_path,
        } => {
            if code == "command.unsupported" {
                AppError::Unsupported(UnsupportedFeature {
                    code,
                    capability: message,
                })
            } else if code == "command.resource_limit" {
                resource_limit_error()
            } else {
                validation_error(&code, field_path.as_deref().unwrap_or("/command"), &message)
            }
        }
        StoreError::IdentityCollision(_) | StoreError::InvalidScenarioIdentity(_) => {
            validation_error(
                "scenario.identity_conflict",
                "/identity",
                "This identity is already owned by another portable record.",
            )
        }
        StoreError::RedoBranchRequiresTruncation => validation_error(
            "history.redo_branch_requires_truncation",
            "/truncateRedo",
            "A new command requires explicit confirmation to discard redo history.",
        ),
        StoreError::NoUndo => validation_error(
            "history.undo_unavailable",
            "/scenarioId",
            "There is no command to undo.",
        ),
        StoreError::NoRedo => validation_error(
            "history.redo_unavailable",
            "/scenarioId",
            "There is no command to redo.",
        ),
        StoreError::CommandNotReversible => validation_error(
            "history.command_not_reversible",
            "/scenarioId",
            "The selected command cannot be reversed.",
        ),
        StoreError::SafetyBackupFailureReceiptRejected => validation_error(
            "restore.safety_backup_receipt_rejected",
            "/authorization/safetyBackup",
            "The safety-backup failure receipt is missing, stale, mismatched, or already consumed.",
        ),
        StoreError::PrivatePath(_) => AppError::Storage(StorageFailure {
            code: "storage.private_path_unsafe".to_owned(),
            message: "The private application storage path is unsafe or inaccessible.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),

        _ => AppError::Storage(StorageFailure {
            code: "storage.operation_failed".to_owned(),
            message: "The local data operation failed safely.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
    }
}
fn unsupported_project_pack(domain_pack: &DomainPackRef) -> AppError {
    AppError::Unsupported(UnsupportedFeature {
        code: "domain_pack.unsupported".to_owned(),
        capability: format!(
            "Domain pack {} schema {}",
            domain_pack.id, domain_pack.schema_version
        ),
    })
}

fn project_initialization_error() -> AppError {
    protocol_error(
        "application.pack_initialization_invalid",
        "The selected domain pack could not initialize a valid project.",
        false,
    )
}

fn valid_command_actor(actor: &ActorRef) -> bool {
    std::iter::once(actor.display_name.as_str())
        .chain(actor.actor_id.as_deref())
        .all(|value| value.len() <= 256 && !value.chars().any(char::is_control))
}

fn validation_error(code: &str, path: &str, message: &str) -> AppError {
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

fn unsupported(capability: DeferredCapability) -> AppError {
    let (code, name) = match capability {
        DeferredCapability::Solve => ("capability.solve_unavailable", "Solve services"),
        DeferredCapability::ArtificialIntelligence => (
            "capability.ai_unavailable",
            "Artificial intelligence services",
        ),
    };
    AppError::Unsupported(UnsupportedFeature {
        code: code.to_owned(),
        capability: name.to_owned(),
    })
}

fn protocol_error(code: &str, message: &str, retryable: bool) -> AppError {
    AppError::Protocol(ProtocolFailure {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
        diagnostic_id: None,
    })
}

fn allocate_unique_uuid(
    ids: &dyn IdGenerator,
    occupied: &BTreeSet<Uuid>,
    allocated: &mut BTreeSet<Uuid>,
) -> Result<Uuid, AppError> {
    for _ in 0..MAX_ID_ALLOCATION_ATTEMPTS {
        let candidate = ids.next_uuid().map_err(id_error)?;
        if candidate.get_version_num() != 7 {
            return Err(id_error(eutheto_types::IdGenerationError::NotVersion7));
        }
        if !occupied.contains(&candidate) && allocated.insert(candidate) {
            return Ok(candidate);
        }
    }
    Err(protocol_error(
        "identity.collision_exhausted",
        "A unique identity could not be allocated safely.",
        false,
    ))
}

fn id_error(_: eutheto_types::IdGenerationError) -> AppError {
    protocol_error(
        "identity.generation_failed",
        "A unique operation identity could not be generated.",
        false,
    )
}

#[derive(Clone, Copy)]
enum ImportVersionKind {
    Newer,
    Older,
}

fn import_version_error(
    kind: ImportVersionKind,
    space: eutheto_import::VersionSpace,
    found: u32,
    current: u32,
) -> AppError {
    match kind {
        ImportVersionKind::Newer => validation_error(
            "portable.version_newer",
            "/bundle",
            &format!("The {space} version is {found}; this application supports up to {current}."),
        ),
        ImportVersionKind::Older => validation_error(
            "portable.version_unsupported",
            "/bundle",
            &format!("The {space} version is {found}; this application currently uses {current}."),
        ),
    }
}

#[derive(Clone, Copy)]
enum ImportLimitKind {
    ArchiveBytes,
    EntryCount,
    EntryBytes,
    TotalBytes,
    CompressionRatio,
    Json(&'static str),
}

fn import_limit_error(kind: ImportLimitKind) -> AppError {
    let (code, message) = match kind {
        ImportLimitKind::ArchiveBytes => (
            "portable.limit.archive_bytes",
            "The portable archive exceeds the compressed-size limit.",
        ),
        ImportLimitKind::EntryCount => (
            "portable.limit.entry_count",
            "The portable archive contains too many entries.",
        ),
        ImportLimitKind::EntryBytes => (
            "portable.limit.entry_bytes",
            "A portable archive entry exceeds its size limit.",
        ),
        ImportLimitKind::TotalBytes => (
            "portable.limit.total_bytes",
            "The portable archive exceeds the total expanded-size limit.",
        ),
        ImportLimitKind::CompressionRatio => (
            "portable.limit.compression_ratio",
            "A portable archive entry exceeds the compression-ratio limit.",
        ),
        ImportLimitKind::Json(limit) => {
            return validation_error(
                "portable.limit.json",
                "/bundle",
                &format!("Portable JSON exceeds the {limit} limit."),
            );
        }
    };
    validation_error(code, "/bundle", message)
}

fn import_error(error: &eutheto_import::ImportError) -> AppError {
    use eutheto_import::ImportError;
    match error {
        ImportError::UnsupportedNewerVersion {
            space,
            found,
            current,
        } => import_version_error(ImportVersionKind::Newer, *space, *found, *current),
        ImportError::UnsupportedOlderVersion {
            space,
            found,
            current,
        } => import_version_error(ImportVersionKind::Older, *space, *found, *current),
        ImportError::ArchiveTooLarge => import_limit_error(ImportLimitKind::ArchiveBytes),
        ImportError::TooManyEntries => import_limit_error(ImportLimitKind::EntryCount),
        ImportError::EntryTooLarge { .. } => import_limit_error(ImportLimitKind::EntryBytes),
        ImportError::TotalSizeExceeded => import_limit_error(ImportLimitKind::TotalBytes),
        ImportError::CompressionRatio { .. } => {
            import_limit_error(ImportLimitKind::CompressionRatio)
        }
        ImportError::JsonLimit { limit, .. } => import_limit_error(ImportLimitKind::Json(limit)),
        ImportError::UnsupportedKind => AppError::Unsupported(UnsupportedFeature {
            code: "portable.kind_unsupported".to_owned(),
            capability: "Portable bundle kind".to_owned(),
        }),
        ImportError::UnsupportedCapability { .. } => AppError::Unsupported(UnsupportedFeature {
            code: "portable.capability_unsupported".to_owned(),
            capability: "Required portable semantic capability".to_owned(),
        }),
        ImportError::StalePreview => validation_error(
            "portable.preview_stale",
            "/preview",
            "The import preview is stale; inspect the bundle again.",
        ),
        ImportError::UnresolvedCollision(_) | ImportError::UnresolvedSupplementalCollision(_) => {
            validation_error(
                "portable.collision_unresolved",
                "/collisionPlan",
                "Every collision requires an explicit resolution.",
            )
        }
        ImportError::UnknownCollision(_) | ImportError::UnknownSupplementalCollision(_) => {
            validation_error(
                "portable.collision_unknown",
                "/collisionPlan",
                "The collision plan names an item outside the current preview.",
            )
        }
        ImportError::InvalidRestore(_) => validation_error(
            "portable.restore_invalid",
            "/restore",
            "The backup restore authorization or mode is invalid.",
        ),
        ImportError::RevisionOverflow { .. } => protocol_error(
            "portable.revision_overflow",
            "The imported scenario revision cannot advance within the supported format.",
            false,
        ),
        ImportError::MigrationRegistry(_) => protocol_error(
            "portable.migration_registry_invalid",
            "The portable migration registry is invalid.",
            false,
        ),
        ImportError::Migration(MigrationFailure::ResourceLimitExceeded) => resource_limit_error(),
        ImportError::Migration(MigrationFailure::Validation(report)) => {
            AppError::Validation(report.clone())
        }
        ImportError::Migration(_) => validation_error(
            "portable.migration_failed",
            "/bundle",
            "The portable scenario could not be migrated safely.",
        ),
        ImportError::Zip(_) | ImportError::Io(_) => AppError::Storage(StorageFailure {
            code: "portable.archive_unreadable".to_owned(),
            message: "The portable archive could not be read safely.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
        ImportError::UnsafePath { .. }
        | ImportError::DuplicatePath(_)
        | ImportError::InvalidZipStructure(_)
        | ImportError::UnsupportedZipFeature(_)
        | ImportError::CaseCollision(_)
        | ImportError::NonRegularEntry { .. }
        | ImportError::ProhibitedContent { .. }
        | ImportError::MissingEntry(_)
        | ImportError::UndeclaredEntry(_)
        | ImportError::MissingChecksum(_)
        | ImportError::ChecksumMismatch(_)
        | ImportError::InvalidChecksums(_)
        | ImportError::InvalidJson { .. }
        | ImportError::InvalidManifest(_)
        | ImportError::InvalidScenario { .. }
        | ImportError::CountMismatch(_)
        | ImportError::ConflictingScenarioRevision { .. }
        | ImportError::Remap(_) => validation_error(
            "portable.content_invalid",
            "/bundle",
            "The portable bundle failed structural or content validation.",
        ),
    }
}

fn filesystem_error(_: io::Error) -> AppError {
    AppError::Storage(StorageFailure {
        code: "storage.directory_initialization_failed".to_owned(),
        message: "The application data directories could not be initialized.".to_owned(),
        retryable: false,
        diagnostic_id: None,
    })
}

fn export_error(error: &ExportError) -> AppError {
    match error {
        ExportError::ResourceLimitExceeded => resource_limit_error(),
        ExportError::Cancelled => {
            protocol_error("operation.cancelled", "The operation was cancelled.", false)
        }
        ExportError::DeadlineExceeded => {
            operation_interrupted(OperationInterruption::DeadlineExceeded)
        }
        ExportError::DestinationExists(_) => AppError::Storage(StorageFailure {
            code: "portable.destination_exists".to_owned(),
            message: "The selected destination already exists; choose a new destination."
                .to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
        ExportError::MissingScenarioDependency(scenario_id) => validation_error(
            "portable.export_dependency_missing",
            "/selection",
            &format!(
                "The portable selection omits referenced scenario {scenario_id}; include it or remove the reference."
            ),
        ),
        ExportError::InvalidModel(_) => validation_error(
            "portable.export_content_invalid",
            "/selection",
            "The selected data cannot be represented as a valid portable bundle.",
        ),
        ExportError::Verification(_) => AppError::Storage(StorageFailure {
            code: "portable.export_verification_failed".to_owned(),
            message: "The created portable bundle did not pass integrity verification.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
        ExportError::Json(_) | ExportError::Zip(_) => AppError::Storage(StorageFailure {
            code: "portable.export_encoding_failed".to_owned(),
            message: "The portable bundle could not be encoded safely.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
        ExportError::Io(_) => AppError::Storage(StorageFailure {
            code: "portable.export_io_failed".to_owned(),
            message: "The portable bundle could not be written safely.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }),
    }
}

fn join_error(_: tokio::task::JoinError) -> AppError {
    protocol_error(
        "application.blocking_task_failed",
        "A background application operation did not complete.",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::error::Error;

    async fn app_with_clock(
        now: Rfc3339Timestamp,
    ) -> Result<(EuthetoApp, tempfile::TempDir), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let app = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("app.sqlite"),
                safety_backups: directory.path().join("backups"),
            },
            clock: Arc::new(eutheto_types::FixedClock::new(now)),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(eutheto_types::SystemIdGenerator),
            cancellation: CancellationToken::new(),
        })
        .await
        .map_err(|error| io::Error::other(format!("opening app fixture: {error:?}")))?;
        Ok((app, directory))
    }

    #[tokio::test]
    async fn native_verification_slot_preserves_parent_deadline_and_cancellation()
    -> Result<(), Box<dyn Error>> {
        let (app, _directory) = app_with_clock("2026-01-01T00:00:00Z".parse()?).await?;
        let _occupied = Arc::clone(&app.accepted_verification.cache.slots)
            .acquire_many_owned(u32::try_from(ACCEPTED_VERIFICATION_MAX_IN_FLIGHT)?)
            .await?;
        for cancel in [false, true] {
            let clock = Arc::new(eutheto_types::FixedMonotonicClock::default());
            let cancellation = CancellationToken::new();
            let budget = eutheto_types::ParentSolveBudget::new(
                eutheto_types::DurationMillis::new(1000)?,
                clock.clone(),
                cancellation.clone(),
            )?;
            let control = OperationControl::Solve(budget.phase_view());
            let pending = app.acquire_verification_slot(
                &control,
                tokio::time::Instant::now() + Duration::from_secs(30),
            );
            tokio::pin!(pending);
            std::future::poll_fn(|context| {
                assert!(pending.as_mut().poll(context).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            let expected = if cancel {
                cancellation.cancel();
                "operation.cancelled"
            } else {
                clock.advance(Duration::from_secs(1))?;
                "operation.deadline_exceeded"
            };
            let result = tokio::time::timeout(Duration::from_secs(1), pending).await?;
            assert!(matches!(result, Err(AppError::Protocol(failure)) if failure.code == expected));
        }
        Ok(())
    }

    // One complete accepted-result/run binding fixture; splitting it hides checksum ancestry.
    #[allow(clippy::too_many_lines)]
    fn publication_fixture() -> Result<(NewSolveRunV1, RunInput, AcceptedResult), Box<dyn Error>> {
        let run_id = "018f47f2-e880-7000-8000-000000000101".parse::<SolveRunId>()?;
        let request_id = "018f47f2-e880-7000-8000-000000000102".parse::<RequestId>()?;
        let scenario_id = "018f47f2-e880-7000-8000-000000000103".parse::<ScenarioId>()?;
        let snapshot_id =
            "018f47f2-e880-7000-8000-000000000104".parse::<eutheto_types::ScenarioSnapshotId>()?;
        let solution_id = "018f47f2-e880-7000-8000-000000000105".parse::<SolutionId>()?;
        let started_at = Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?;
        let pack_id = PackId::new("official.synthetic")?;
        let backend_id = BackendId::new("ortools.cp-sat")?;
        let model_hash = blake3_hex(b"model");
        let objective_policy_hash = blake3_hex(b"objective-policy");
        let solve_options = eutheto_types::SolveOptions {
            backend: eutheto_types::BackendSelection::Auto,
            mode: eutheto_types::SolveMode::Balanced,
            time_limit_milliseconds: eutheto_types::DurationMillis::new(30_000)?,
            memory_limit_bytes: Some(64 * 1024 * 1024),
            worker_threads: eutheto_types::WorkerThreadPolicy::Exact(1),
            random_seed: 7,
            solution_limit: Some(1),
            stop_after_first_feasible: true,
            collect_intermediate_solutions: false,
            explanation_mode: eutheto_types::ExplanationMode::Standard,
            preserve_existing: eutheto_types::PreservationPolicy::None,
            reproducibility: eutheto_types::ReproducibilityMode::Deterministic,
            resource_limits: eutheto_types::ResourceLimits {
                max_entities: 1_000,
                max_rules: 1_000,
                max_variables: 10_000,
                max_constraints: 10_000,
            },
        };
        let input = RunInput::new(
            run_id,
            request_id,
            scenario_id,
            7,
            snapshot_id,
            blake3_hex(b"snapshot"),
            started_at,
            pack_id.clone(),
            1,
            1,
            "compiler-1".to_owned(),
            "app-1".to_owned(),
            backend_id.clone(),
            "backend-1".to_owned(),
            "adapter-1".to_owned(),
            "worker-1".to_owned(),
            "solver-1".to_owned(),
            1,
            0,
            model_hash.clone(),
            objective_policy_hash.clone(),
            solve_options.clone(),
            "America/Chicago".parse()?,
            None,
            None,
        )?;
        let request = NewSolveRunV1 {
            objective_context: None,
            run_id,
            request_id,
            scenario_id,
            expected_revision: Revision::new(7),
            planning_ir_schema_version: 1,
            compiler_version: "compiler-1".to_owned(),
            application_version: "app-1".to_owned(),
            backend_id,
            backend_version: "backend-1".to_owned(),
            adapter_version: "adapter-1".to_owned(),
            worker_version: "worker-1".to_owned(),
            solver_version: "solver-1".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            model_hash,
            objective_policy_hash,
            solve_options,
            temporary_condition_hash: None,
            started_at,
        };
        let solution = eutheto_domain_ir::NormalizedSolution {
            schema_version: eutheto_domain_ir::NORMALIZED_SOLUTION_SCHEMA_VERSION,
            pack_id,
            scenario_id,
            scenario_revision: 7,
            projection_version: 1,
            solution_id,
            assignments: Vec::new(),
        };
        let context = VerificationContextV1::new(
            scenario_id,
            7,
            blake3_hex(b"snapshot"),
            blake3_hex(b"model"),
            solution.canonical_hash()?,
            blake3_hex(b"verification-scope"),
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
        Ok((request, input, AcceptedResult::new(solution, report)?))
    }

    fn publication_state(
        app: &EuthetoApp,
        accepted_at: Rfc3339Timestamp,
    ) -> Result<
        (
            OrdinarySolveLease,
            OrdinaryCheckpointWork,
            RunInput,
            AcceptedResult,
        ),
        Box<dyn Error>,
    > {
        let (request, input, accepted) = publication_fixture()?;
        let lease = app
            .reserve_ordinary_solve_custody(&request)
            .map_err(|error| io::Error::other(format!("reserving lease fixture: {error:?}")))?;
        lease
            .bind_started(&input, request.started_at)
            .map_err(|error| io::Error::other(format!("binding lease fixture: {error:?}")))?;
        lease
            .record_first_accepted(
                accepted.solution.solution_id,
                &accepted.checksum,
                accepted_at,
            )
            .map_err(|error| io::Error::other(format!("recording accepted fixture: {error:?}")))?;
        let work = lease
            .checkpoint_work()
            .map_err(|error| io::Error::other(format!("claiming checkpoint fixture: {error:?}")))?;
        Ok((lease, work, input, accepted))
    }

    #[test]
    fn recipient_source_counts_captured_json_spare_capacity() -> Result<(), Box<dyn Error>> {
        let (_, mut input, accepted) = publication_fixture()?;
        let hash = input.snapshot_document_hash.clone();
        input.objective_context = Some(serde_json::from_value(serde_json::json!({
            "schemaVersion": 2, "packId": input.pack_id,
            "profileId": "eutheto.objective.authored", "semanticVersion": 1,
            "operation": "repair",
            "baseline": {
                "original": {
                    "entity": { "kind": "tests.baseline", "id": "tests.original" },
                    "pairContentHash": hash,
                    "acceptedResult": {
                        "solutionId": accepted.solution.solution_id,
                        "resultChecksum": accepted.checksum,
                    },
                    "sourceScenarioId": input.scenario_id, "sourceRevision": input.scenario_revision,
                    "sourceDocumentHash": hash, "sourceRunId": input.run_id,
                    "sourceRunInputChecksum": input.checksum, "sourceRunManifestChecksum": hash,
                },
                "selection": { "kind": "originalAccepted" }, "selectedPairContentHash": hash,
                "capturedOriginal": { "label": "x", "pairs": [] },
            },
        }))?);
        let control = OperationControl::Cancellation(CancellationToken::new());
        let measure = |input: &RunInput| -> Result<usize, Box<dyn Error>> {
            let mut bytes = recipient_memory::SourceBytes::new(&control)
                .map_err(|error| format!("{error:?}"))?;
            bytes.input(input).map_err(|error| format!("{error:?}"))?;
            Ok(bytes.finish())
        };
        let before = measure(&input)?;
        let context = input
            .objective_context
            .as_mut()
            .ok_or("missing objective context")?;
        let baseline = context
            .baseline
            .as_mut()
            .ok_or("missing original baseline")?;
        let original = Arc::get_mut(baseline.captured_original.as_mut().ok_or("missing JSON")?)
            .ok_or("unexpected shared test JSON")?;
        let object = original
            .as_object_mut()
            .ok_or("original is not an object")?;
        let Some(Value::String(label)) = object.get_mut("label") else {
            return Err("missing label".into());
        };
        let previous_label = label.capacity();
        label.reserve(1024 * 1024);
        let extra_label = label.capacity() - previous_label;
        let pairs = object
            .get_mut("pairs")
            .and_then(Value::as_array_mut)
            .ok_or("missing pairs")?;
        let previous_pairs = pairs.capacity();
        pairs.reserve(32_768);
        let extra_pairs = (pairs.capacity() - previous_pairs) * size_of::<Value>();
        // Logical/canonical content is unchanged; its allocated backing is not.
        assert_eq!(measure(&input)?, before + extra_label + extra_pairs);
        control.cancellation().cancel();
        assert!(measure(&input).is_err());
        Ok(())
    }

    fn checkpoint(
        input: RunInput,
        accepted: AcceptedResult,
        accepted_at: Rfc3339Timestamp,
    ) -> Result<VerifiedCheckpoint, Box<dyn Error>> {
        Ok(VerifiedCheckpoint::new(
            input,
            eutheto_types::DurationMillis::new(100)?,
            eutheto_types::DurationMillis::new(200)?,
            accepted_at,
            accepted,
            BTreeMap::new(),
        )?)
    }

    #[tokio::test]
    async fn verified_publication_keeps_acceptance_time_separate_from_event_time()
    -> Result<(), Box<dyn Error>> {
        let accepted_at = Rfc3339Timestamp::parse("2026-01-01T00:00:01Z")?;
        let published_at = Rfc3339Timestamp::parse("2026-01-01T00:00:02Z")?;
        let (app, _directory) = app_with_clock(published_at).await?;
        let mut events = app.events.subscribe();
        let (lease, work, input, accepted) = publication_state(&app, accepted_at)?;
        let checkpoint = checkpoint(input, accepted, accepted_at)?;
        assert_eq!(checkpoint.accepted_at, accepted_at);

        app.publish_solve_verified(&checkpoint, &work)
            .map_err(|error| {
                io::Error::other(format!("publishing checkpoint fixture: {error:?}"))
            })?;

        let event = events.try_recv()?;
        assert_eq!(event.topic, EventTopic::SolveVerified);
        assert!(matches!(
            event.payload,
            EventPayload::SolveVerified { context, .. }
                if context.timestamp == published_at
                    && context.timestamp != checkpoint.accepted_at
        ));
        let bounds = lease
            .observation_bounds()
            .map_err(|error| io::Error::other(format!("reading publication bounds: {error:?}")))?;
        assert_eq!(bounds.latest_wall, Some(published_at));
        lease.observe_wall(accepted_at);
        let bounds = lease
            .observation_bounds()
            .map_err(|error| io::Error::other(format!("reading final bounds: {error:?}")))?;
        assert!(bounds.unreliable);
        assert_eq!(bounds.latest_wall, Some(published_at));
        Ok(())
    }

    #[tokio::test]
    async fn verified_publication_rejects_a_checkpoint_with_a_different_acceptance_time()
    -> Result<(), Box<dyn Error>> {
        let accepted_at = Rfc3339Timestamp::parse("2026-01-01T00:00:01Z")?;
        let (app, _directory) =
            app_with_clock(Rfc3339Timestamp::parse("2026-01-01T00:00:03Z")?).await?;
        let mut events = app.events.subscribe();
        let (_lease, work, input, accepted) = publication_state(&app, accepted_at)?;
        let checkpoint = checkpoint(
            input,
            accepted,
            Rfc3339Timestamp::parse("2026-01-01T00:00:02Z")?,
        )?;

        assert!(matches!(
            app.publish_solve_verified(&checkpoint, &work),
            Err(AppError::Protocol(failure)) if failure.code == "solution.authority_invalid"
        ));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn verified_publication_rejects_a_regressed_publish_time_without_emitting()
    -> Result<(), Box<dyn Error>> {
        let accepted_at = Rfc3339Timestamp::parse("2026-01-01T00:00:02Z")?;
        let (app, _directory) =
            app_with_clock(Rfc3339Timestamp::parse("2026-01-01T00:00:01Z")?).await?;
        let mut events = app.events.subscribe();
        let (_lease, work, input, accepted) = publication_state(&app, accepted_at)?;
        let checkpoint = checkpoint(input, accepted, accepted_at)?;

        assert!(matches!(
            app.publish_solve_verified(&checkpoint, &work),
            Err(AppError::Protocol(failure)) if failure.code == "solve.clock_invalid"
        ));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        Ok(())
    }

    #[test]
    fn portable_error_mapping_is_typed_and_does_not_expose_internal_details() {
        let version = import_error(&eutheto_import::ImportError::UnsupportedNewerVersion {
            space: eutheto_import::VersionSpace::PortableSchema,
            found: 3,
            current: 1,
        });
        assert!(matches!(
            version,
            AppError::Validation(report)
                if report.issues.len() == 1
                    && report.issues[0].code == "portable.version_newer"
                    && report.issues[0].message.contains("portable schema")
                    && report.issues[0].message.contains('3')
                    && report.issues[0].message.contains('1')
        ));

        let secret_path = "/home/operator/private-library/scenario.json";
        let secret_backend = "backend parse details";
        let invalid = import_error(&eutheto_import::ImportError::InvalidJson {
            path: secret_path.to_owned(),
            message: secret_backend.to_owned(),
        });
        let rendered = format!("{invalid:?}");
        assert!(!rendered.contains(secret_path));
        assert!(!rendered.contains(secret_backend));
        assert!(matches!(
            invalid,
            AppError::Validation(report)
                if report.issues.len() == 1
                    && report.issues[0].code == "portable.content_invalid"
        ));

        let destination = "/home/operator/private-library/backup.eutheto";
        let raw_error = ExportError::DestinationExists(PathBuf::from(destination));
        let exists = export_error(&raw_error);
        assert!(!format!("{exists:?}").contains(destination));
        assert!(matches!(
            exists,
            AppError::Storage(failure)
                if failure.code == "portable.destination_exists" && !failure.retryable
        ));

        let identity = Uuid::now_v7();
        let collision = store_error(StoreError::IdentityCollision(identity));
        assert!(!format!("{collision:?}").contains(&identity.to_string()));
        assert!(matches!(
            collision,
            AppError::Validation(report)
                if report.issues.len() == 1
                    && report.issues[0].code == "scenario.identity_conflict"
                    && report.issues[0].field_path.as_deref() == Some("/identity")
        ));
    }

    #[test]
    fn safety_backup_failure_mapping_accepts_backup_paths_but_not_unrelated_storage() {
        let directory_failure =
            filesystem_error(io::Error::other("blocked safety backup directory"));
        assert_eq!(
            safe_backup_failure_reason(&directory_failure).as_deref(),
            Some("The application data directories could not be initialized.")
        );
        let unsafe_private_path = store_error(StoreError::PrivatePath(io::Error::other(
            "/home/operator/private unsafe path",
        )));
        assert_eq!(
            safe_backup_failure_reason(&unsafe_private_path).as_deref(),
            Some("The private application storage path is unsafe or inaccessible.")
        );
        assert!(!format!("{unsafe_private_path:?}").contains("/home/operator"));
        let publication_failure = export_error(&ExportError::DestinationExists(PathBuf::from(
            "/private/safety-backup.eutheto",
        )));
        assert!(safe_backup_failure_reason(&publication_failure).is_some());
        let unrelated = AppError::Storage(StorageFailure {
            code: "storage.operation_failed".to_owned(),
            message: "Unrelated authoritative storage failed.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        });
        assert!(safe_backup_failure_reason(&unrelated).is_none());
    }
}
