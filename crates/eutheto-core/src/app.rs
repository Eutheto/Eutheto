use crate::counterfactual::{
    COUNTERFACTUAL_API_SCHEMA_VERSION, CounterfactualRuntime, CounterfactualRuntimeServices,
    SolutionCancelCounterfactualDtoV1, SolutionCancelCounterfactualRequestV1,
    SolutionStartCounterfactualDtoV1, SolutionStartCounterfactualRequestV1,
};
#[cfg(debug_assertions)]
use eutheto_command::official_registry;
use eutheto_command::{CommandError, apply_command_with_registry, validate_document_shape};
use eutheto_domain_api::{
    DomainCatalog, DomainPackDescriptor, DomainPackError, DomainPackRegistry, DomainView,
    PortableImportContext,
};
use eutheto_domain_ir::{
    AcceptedResult, AcceptedResultRefV1, AssignmentEvidenceV1, ComparisonContext,
    ComparisonRunManifests, EvidenceRenderRequestV1, ExplanationEvidencePayloadV1,
    ExplanationEvidenceV1, ExplanationRequestSubjectV1, ExplanationRequestV1, ExplanationResultV1,
    OptimalityStatusEvidenceV1, RepairCausalityV1, RepairEvidenceV1, ScoreVector,
    SolutionComparisonV1, VerificationContextV1, VerificationReport, blake3_hex,
    compare_accepted_results,
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
    AppSetting, CommandWrite, HistoryCommand, HistoryEntry, InitializationOutcome, JournalWrite,
    NewProject, ProjectListScope, RedoBranchPolicy, SafetyBackupFailureReceipt,
    SqliteScenarioStore, StagedLibraryApply, StoreError, StoredAcceptedResultSummary,
    StoredAcceptedResultV2, StoredProject,
};
use eutheto_types::{
    ActorRef, AppError, BackendId, BundleId, CancellationToken, Change, ChangeKind, ChangeSet,
    Clock, CommandEnvelope, CommandResult, CommandSource, DirectoryAvailabilityLabel,
    DomainPackRef, EventContext, EventPayload, EventTopic, IdGenerator, MonotonicClock, PackId,
    PortableAsset, PortableDomainDocument, PortableProjectMetadata, ProjectMetadataDto,
    ProjectSummaryDto, ProtocolFailure, RequestId, ResourceRef, Revision, Rfc3339Timestamp,
    SCENARIO_FORMAT_VERSION, SUPPORT_PREVIEW_SCHEMA_VERSION, ScenarioDocument, ScenarioDomain,
    ScenarioId, ScenarioMetadata, ScenarioSettings, ScenarioSnapshotV1, ScenarioViewDto,
    SolutionId, SolveRunId, SolveStatus, StorageFailure, SupportApplicationMetadataDto,
    SupportDirectoryMetadataDto, SupportLibraryMetadataDto, SupportPreviewDto,
    SupportSchemaMetadataDto, UnsupportedFeature, ValidationIssue, ValidationReport,
    ValidationSeverity, VerificationFailure, collect_scenario_owned_uuids,
    extract_asset_references, extract_result_dependency, extract_result_id,
    extract_scenario_references,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

const EVENT_VERSION: u32 = 1;
/// Current application solution-read wire schema.
pub const SOLUTION_API_SCHEMA_VERSION: u32 = 1;
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
#[cfg(debug_assertions)]
fn validated_static_pack_registry() -> Result<DomainPackRegistry, AppError> {
    // The synthetic conformance pack is available only to debug/test binaries. Release binaries
    // start without domain packs until a real pack is explicitly supplied.
    official_registry().map_err(|_| {
        protocol_error(
            "application.pack_registry_invalid",
            "Compiled domain-pack metadata failed its startup invariant.",
            false,
        )
    })
}

#[cfg(not(debug_assertions))]
fn validated_static_pack_registry() -> Result<DomainPackRegistry, AppError> {
    DomainPackRegistry::builder().build().map_err(|_| {
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

/// Deferred capability families with stable unavailable responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferredCapability {
    Solve,
    Solution,
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

/// Selects one pack-owned result view from an accepted solution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionViewRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
    pub view_id: String,
}

/// Requests current independent re-verification of one accepted solution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionVerifyRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
}

/// Selects two independently accepted results for deterministic comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionCompareRequestV1 {
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
    pub solutions: Vec<SolutionSummaryDtoV1>,
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

/// Deterministic comparison envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionComparisonDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub comparison: SolutionComparisonV1,
}

/// Rendered typed evidence and all selected recorded revisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SolutionExplanationDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub current_revision: Revision,
    pub scenario_revisions: Vec<Revision>,
    pub explanation: ExplanationResultV1,
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
        key: String,
        value: Value,
    },
    DeleteSetting {
        request_id: RequestId,
        key: String,
    },
    SolutionSelect(SolutionSelectRequestV1),
    SolutionStartCounterfactual(SolutionStartCounterfactualRequestV1),
    SolutionCancelCounterfactual(SolutionCancelCounterfactualRequestV1),
    ExportScenario {
        scenario_id: ScenarioId,
        destination: PathBuf,
    },
    CreateBackup {
        title: String,
        destination: PathBuf,
        selection: BackupSelection,
    },
    PublishPreparedPortable {
        destination: PathBuf,
        bytes: Vec<u8>,
        expected_sha256: String,
        binding: PreparedPortableBinding,
    },
    ApplyImport {
        request_id: RequestId,
        preview_id: RequestId,
        collision_plan: CollisionPlan,
    },
    ApplyRestore {
        request_id: RequestId,
        preview_id: RequestId,
        collision_plan: CollisionPlan,
        authorization: RestoreAuthorization,
    },
    CancelPortablePreview {
        preview_id: RequestId,
    },
    /// Atomically writes and consumes one previously inspected unopened bundle.
    ExactReexportUnopenedBundle {
        preview_id: RequestId,
        destination: PathBuf,
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
    Setting(String),
    SolutionList(SolutionListRequestV1),
    SolutionGetSummary(SolutionSummaryRequestV1),
    SolutionGetView(SolutionViewRequestV1),
    SolutionVerify(SolutionVerifyRequestV1),
    SolutionCompare(SolutionCompareRequestV1),
    SolutionExplain(SolutionExplainRequestV1),
    PreviewImport {
        bytes: Vec<u8>,
        options: ImportOptions,
    },
    PreviewRestore {
        bytes: Vec<u8>,
        options: ImportOptions,
    },
    ExportScenario(ScenarioId),
    ExportBackup {
        title: String,
        selection: BackupSelection,
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
    SettingUpdated,
    SolutionSelected(SolutionSummaryDtoV1),
    CounterfactualStarted(SolutionStartCounterfactualDtoV1),
    CounterfactualCancelled(SolutionCancelCounterfactualDtoV1),
    SettingDeleted(bool),
    BundleWritten,
    BackupWritten(BackupSummary),
    PortableApplied {
        scenarios: Vec<AppliedPortableScenario>,
    },
    PortablePreviewCancelled,
    UnopenedBundleReexported,
}

/// Results from read-only application operations.
#[derive(Clone, Debug)]
pub enum AppQueryResult {
    Projects(Vec<ProjectSummaryDto>),
    Project(ProjectMetadataDto),
    Scenario(Box<ScenarioViewDto>),
    Validation(ValidationReport),
    History(Vec<HistoryEntry>),
    Setting(Option<AppSetting<Value>>),
    SolutionList(SolutionListDtoV1),
    SolutionSummary(Box<SolutionDetailDtoV1>),
    SolutionView(Box<SolutionViewDtoV1>),
    SolutionVerification(Box<SolutionVerificationDtoV1>),
    SolutionComparison(Box<SolutionComparisonDtoV1>),
    SolutionExplanation(Box<SolutionExplanationDtoV1>),
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
}

impl PendingPortablePreview {
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Import(preview) => preview.retained_bytes,
            Self::Unopened(bundle) => bundle.retained_memory_bytes(),
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
}
struct CommittedPortableApply {
    scenarios: Vec<AppliedPortableScenario>,
    events: Vec<(ScenarioId, Revision, ChangeKind)>,
    library_changed: bool,
}

#[derive(Clone, Copy)]
enum ProjectLifecycleAction {
    Archive,
    Unarchive,
    Delete,
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
    mutation_locks: ScenarioLocks,
    previews: Arc<Mutex<BTreeMap<RequestId, PendingPortablePreview>>>,
    events: broadcast::Sender<AppEvent>,
    counterfactual: CounterfactualRuntime,
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
            mutation_locks: Arc::new(Mutex::new(BTreeMap::new())),
            previews: Arc::new(Mutex::new(BTreeMap::new())),
            events,
            counterfactual,
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

    /// Executes one application mutation.
    /// # Errors
    ///
    /// Returns validation, conflict, unsupported, storage, or protocol errors
    /// without exposing backend diagnostics.
    pub async fn execute(&self, command: AppCommand) -> Result<AppCommandResult, AppError> {
        self.check_cancelled()?;
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
            } => {
                self.apply_scenario(request_id, envelope, truncate_redo)
                    .await
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
            AppCommand::SolutionStartCounterfactual(request) => {
                self.start_counterfactual(request).await
            }
            AppCommand::SolutionCancelCounterfactual(request) => {
                self.cancel_counterfactual(request).await
            }
            AppCommand::ExportScenario {
                scenario_id,
                destination,
            } => {
                let (bytes, _, _) = self.export_scenario(scenario_id).await?;
                self.write_bundle(destination, bytes).await?;
                Ok(AppCommandResult::BundleWritten)
            }
            AppCommand::CreateBackup {
                title,
                destination,
                selection,
            } => {
                let (bytes, summary, _) = self.export_backup(title, selection).await?;
                self.write_bundle(destination, bytes).await?;
                Ok(AppCommandResult::BackupWritten(summary))
            }
            AppCommand::PublishPreparedPortable {
                destination,
                bytes,
                expected_sha256,
                binding,
            } => {
                self.publish_prepared_portable(destination, bytes, &expected_sha256, binding)
                    .await
            }
            AppCommand::ExactReexportUnopenedBundle {
                preview_id,
                destination,
            } => {
                self.exact_reexport_unopened_bundle(preview_id, destination)
                    .await
            }
            command @ (AppCommand::ApplyImport { .. }
            | AppCommand::ApplyRestore { .. }
            | AppCommand::CancelPortablePreview { .. }) => {
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

    async fn execute_setting_command(
        &self,
        command: AppCommand,
    ) -> Result<AppCommandResult, AppError> {
        match command {
            AppCommand::SetSetting {
                request_id,
                key,
                value,
            } => {
                validate_app_setting(&key, &value)?;
                self.store
                    .set_setting(key, value, self.clock.now())
                    .await
                    .map_err(store_error)?;
                self.publish_app_notification(
                    request_id,
                    "settings.updated",
                    "Application settings changed.",
                );
                Ok(AppCommandResult::SettingUpdated)
            }
            AppCommand::DeleteSetting { request_id, key } => {
                validate_app_setting_key(&key)?;
                let existed = self.store.delete_setting(key).await.map_err(store_error)?;
                if existed {
                    self.publish_app_notification(
                        request_id,
                        "settings.deleted",
                        "Application settings changed.",
                    );
                }
                Ok(AppCommandResult::SettingDeleted(existed))
            }
            _ => unreachable!("dispatcher passes only setting commands"),
        }
    }

    async fn execute_portable_command(
        &self,
        command: AppCommand,
    ) -> Result<AppCommandResult, AppError> {
        match command {
            AppCommand::ApplyImport {
                request_id,
                preview_id,
                collision_plan,
            } => {
                self.apply_portable(request_id, preview_id, collision_plan, None)
                    .await
            }
            AppCommand::ApplyRestore {
                request_id,
                preview_id,
                collision_plan,
                authorization,
            } => {
                self.apply_portable(request_id, preview_id, collision_plan, Some(authorization))
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
        let library = self.store.library_snapshot().await.map_err(store_error)?;
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
                    projects.iter().map(project_summary).collect(),
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
            AppQuery::Setting(key) => {
                validate_app_setting_key(&key)?;
                self.store
                    .get_setting(key)
                    .await
                    .map(AppQueryResult::Setting)
                    .map_err(store_error)
            }
            AppQuery::SolutionList(request) => self.query_solution_list(request).await,
            AppQuery::SolutionGetSummary(request) => self.query_solution_summary(request).await,
            AppQuery::SolutionGetView(request) => self.query_solution_view(request).await,
            AppQuery::SolutionVerify(request) => self.query_solution_verify(request).await,
            AppQuery::SolutionCompare(request) => self.query_solution_compare(request).await,
            AppQuery::SolutionExplain(request) => self.query_solution_explain(request).await,
            AppQuery::PreviewImport { bytes, options } => {
                self.query_preview_import(bytes, options).await
            }
            AppQuery::PreviewRestore { bytes, options } => {
                self.query_preview_restore(bytes, options).await
            }
            AppQuery::ExportScenario(id) => {
                let (bytes, scenario_revision, library_revision) = self.export_scenario(id).await?;
                Ok(AppQueryResult::Bundle {
                    bytes,
                    scenario_revision,
                    library_revision,
                })
            }
            AppQuery::ExportBackup { title, selection } => {
                let (bytes, summary, library_revision) =
                    self.export_backup(title, selection).await?;
                Ok(AppQueryResult::BackupBundle {
                    bytes,
                    summary,
                    library_revision,
                })
            }
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
            AppQuery::InspectUnopenedBundle { bytes } => self.inspect_unopened_bundle(bytes).await,
        }
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
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let selected = self
            .store
            .select_accepted_result(
                request.scenario_id,
                request.expected_revision,
                request.solution_id,
            )
            .await
            .map_err(store_error)?;
        Ok(AppCommandResult::SolutionSelected(solution_summary(
            &selected,
        )))
    }
    async fn start_counterfactual(
        &self,
        request: SolutionStartCounterfactualRequestV1,
    ) -> Result<AppCommandResult, AppError> {
        let request_id = request.request_id;
        let scenario_id = request.scenario_id;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let job = Box::pin(
            self.counterfactual
                .start(request, Arc::clone(&self.monotonic_clock)),
        )
        .await?;
        let current_revision = self.current_scenario_revision(scenario_id).await?;
        Ok(AppCommandResult::CounterfactualStarted(
            SolutionStartCounterfactualDtoV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                request_id,
                current_revision,
                job,
            },
        ))
    }

    async fn cancel_counterfactual(
        &self,
        request: SolutionCancelCounterfactualRequestV1,
    ) -> Result<AppCommandResult, AppError> {
        let cancel_request_id = request.cancel_request_id;
        let scenario_id = request.scenario_id;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let job = self.counterfactual.cancel(request).await?;
        let current_revision = self.current_scenario_revision(scenario_id).await?;
        Ok(AppCommandResult::CounterfactualCancelled(
            SolutionCancelCounterfactualDtoV1 {
                schema_version: COUNTERFACTUAL_API_SCHEMA_VERSION,
                cancel_request_id,
                current_revision,
                job,
            },
        ))
    }

    async fn query_solution_list(
        &self,
        request: SolutionListRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let summaries = self
            .store
            .list_accepted_results(request.scenario_id)
            .await
            .map_err(store_error)?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await?;
        Ok(AppQueryResult::SolutionList(SolutionListDtoV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            current_revision,
            solutions: summaries.iter().map(solution_summary).collect(),
        }))
    }

    async fn query_solution_summary(
        &self,
        request: SolutionSummaryRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
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
        Ok(AppQueryResult::SolutionSummary(Box::new(solution_detail(
            &stored,
            current_revision,
            selected,
        )?)))
    }

    async fn query_solution_view(
        &self,
        request: SolutionViewRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let stored = self
            .load_solution(request.scenario_id, request.solution_id)
            .await?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await?;
        let pack = solution_pack(&stored, &self.pack_registry)?;
        let accepted = &stored.portable.accepted_result;
        let view = pack
            .build_view(&stored.document, Some(&accepted.solution), &request.view_id)
            .map_err(|error| solution_view_error(&error))?;
        Ok(AppQueryResult::SolutionView(Box::new(SolutionViewDtoV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            current_revision,
            scenario_revision: solution_revision(&stored)?,
            result: accepted_reference(accepted)?,
            view,
        })))
    }

    async fn query_solution_verify(
        &self,
        request: SolutionVerifyRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let stored = self
            .load_solution(request.scenario_id, request.solution_id)
            .await?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await?;
        let accepted = &stored.portable.accepted_result;
        let report = reverify_accepted_solution(&stored, &self.pack_registry)?;
        Ok(AppQueryResult::SolutionVerification(Box::new(
            SolutionVerificationDtoV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                current_revision,
                scenario_revision: solution_revision(&stored)?,
                result: accepted_reference(accepted)?,
                verification: report,
            },
        )))
    }

    async fn query_solution_compare(
        &self,
        request: SolutionCompareRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        let base = self
            .load_solution(request.scenario_id, request.base_solution_id)
            .await?;
        let candidate = self
            .load_solution(request.scenario_id, request.candidate_solution_id)
            .await?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await?;
        let comparison = compare_stored_solutions(&base, &candidate)?;
        Ok(AppQueryResult::SolutionComparison(Box::new(
            SolutionComparisonDtoV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                current_revision,
                comparison,
            },
        )))
    }

    // Keeping the seven typed subjects together makes the evidence/reference binding auditable.
    #[allow(clippy::too_many_lines)]
    async fn query_solution_explain(
        &self,
        request: SolutionExplainRequestV1,
    ) -> Result<AppQueryResult, AppError> {
        ensure_solution_schema(request.schema_version)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        request
            .request
            .validate()
            .map_err(|_| solution_reference_error("/request"))?;
        let (document, revisions, payload) = match &request.request.subject {
            ExplanationRequestSubjectV1::Validation { .. } => {
                return Err(explanation_unavailable("validation"));
            }
            ExplanationRequestSubjectV1::Infeasibility { .. } => {
                return Err(explanation_unavailable("infeasibility"));
            }
            ExplanationRequestSubjectV1::Assignment {
                result,
                assignment_id,
            } => {
                let stored = self
                    .load_referenced_solution(request.scenario_id, result)
                    .await?;
                let accepted = &stored.portable.accepted_result;
                let assignment = accepted
                    .solution
                    .assignments
                    .iter()
                    .find(|assignment| &assignment.id == assignment_id)
                    .cloned()
                    .ok_or(AppError::NotFound(ResourceRef::Solution(
                        result.solution_id,
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
                let revision = solution_revision(&stored)?;
                (
                    stored.document,
                    vec![revision],
                    ExplanationEvidencePayloadV1::Assignment {
                        assignment: AssignmentEvidenceV1 {
                            assignment,
                            related_rules,
                            score_contributions: Vec::new(),
                            metrics: BTreeMap::new(),
                            lock_state: None,
                        },
                    },
                )
            }
            ExplanationRequestSubjectV1::Counterfactual { job_id, base } => {
                let stored = self
                    .load_referenced_solution(request.scenario_id, base)
                    .await?;
                let job = self
                    .store
                    .load_counterfactual_job(*job_id)
                    .await
                    .map_err(store_error)?;
                if job.request.semantics.scenario_id != request.scenario_id
                    || job.request.semantics.base != *base
                {
                    return Err(solution_reference_error("/request/subject"));
                }
                let result = job
                    .result
                    .ok_or_else(|| explanation_unavailable("counterfactual"))?;
                let revision = solution_revision(&stored)?;
                (
                    stored.document,
                    vec![revision],
                    ExplanationEvidencePayloadV1::Counterfactual {
                        result: Box::new(result),
                    },
                )
            }
            ExplanationRequestSubjectV1::SolutionDifference { left, right } => {
                let base = self
                    .load_referenced_solution(request.scenario_id, left)
                    .await?;
                let candidate = self
                    .load_referenced_solution(request.scenario_id, right)
                    .await?;
                let base_revision = solution_revision(&base)?;
                let candidate_revision = solution_revision(&candidate)?;
                let comparison = compare_stored_solutions(&base, &candidate)?;
                (
                    candidate.document,
                    vec![base_revision, candidate_revision],
                    ExplanationEvidencePayloadV1::SolutionDifference {
                        comparison: Box::new(comparison),
                    },
                )
            }
            ExplanationRequestSubjectV1::Repair { current, base } => {
                let current_result = self
                    .load_referenced_solution(request.scenario_id, current)
                    .await?;
                let base_result = self
                    .load_referenced_solution(request.scenario_id, base)
                    .await?;
                let base_revision = solution_revision(&base_result)?;
                let current_revision = solution_revision(&current_result)?;
                let comparison = compare_stored_solutions(&base_result, &current_result)?;
                (
                    current_result.document,
                    vec![current_revision, base_revision],
                    ExplanationEvidencePayloadV1::Repair {
                        repair: Box::new(RepairEvidenceV1 {
                            comparison,
                            causality: RepairCausalityV1::NotEstablished,
                        }),
                    },
                )
            }
            ExplanationRequestSubjectV1::OptimalityStatus {
                solve_run_id,
                run_manifest_checksum,
                result,
            } => {
                let result = result
                    .as_ref()
                    .ok_or_else(|| explanation_unavailable("optimality_status"))?;
                let stored = self
                    .load_referenced_solution(request.scenario_id, result)
                    .await?;
                if stored.portable.run_input.run_id != *solve_run_id
                    || stored.portable.run_manifest.checksum != *run_manifest_checksum
                {
                    return Err(solution_reference_error("/request/subject"));
                }
                let revision = solution_revision(&stored)?;
                (
                    stored.document,
                    vec![revision],
                    ExplanationEvidencePayloadV1::OptimalityStatus {
                        status: Box::new(OptimalityStatusEvidenceV1 {
                            run_input: stored.portable.run_input,
                            run_manifest: stored.portable.run_manifest,
                            result: Some(result.clone()),
                        }),
                    },
                )
            }
        };
        let evidence =
            ExplanationEvidenceV1::new(payload).map_err(|_| solution_contract_error())?;
        let render_request = EvidenceRenderRequestV1::new(evidence.clone())
            .map_err(|_| solution_contract_error())?;
        let pack = available_pack(&document, &self.pack_registry)
            .ok_or_else(|| unsupported_project_pack(&document.domain_pack))?;
        let rendered = pack
            .render_evidence(&document, &render_request)
            .map_err(|error| explanation_render_error(&error))?;
        let explanation =
            ExplanationResultV1::new(evidence, rendered).map_err(|_| solution_contract_error())?;
        let current_revision = self.current_scenario_revision(request.scenario_id).await?;
        Ok(AppQueryResult::SolutionExplanation(Box::new(
            SolutionExplanationDtoV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id: request.scenario_id,
                current_revision,
                scenario_revisions: revisions,
                explanation,
            },
        )))
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
        Ok(stored)
    }

    async fn query_preview_import(
        &self,
        bytes: Vec<u8>,
        options: ImportOptions,
    ) -> Result<AppQueryResult, AppError> {
        if options.restore_mode == RestoreMode::ImportScenario {
            self.preview_portable(bytes, options).await
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
    ) -> Result<AppQueryResult, AppError> {
        if options.restore_mode == RestoreMode::ImportScenario {
            Err(validation_error(
                "portable.restore_mode_invalid",
                "/restoreMode",
                "Backup restore requires add-backup or replace-library mode.",
            ))
        } else {
            self.preview_portable(bytes, options).await
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
        if title.trim().is_empty() {
            return Err(validation_error(
                "project.title_required",
                "/title",
                "Project title must not be empty.",
            ));
        }
        let Some(descriptor) = self
            .pack_registry
            .descriptors()
            .find(|descriptor| descriptor.id == domain_pack.id)
        else {
            return Err(unsupported_project_pack(&domain_pack));
        };
        if domain_pack.schema_version != descriptor.scenario_versions.latest {
            return Err(unsupported_project_pack(&domain_pack));
        }
        let pack = self
            .pack_registry
            .require(&domain_pack.id)
            .map_err(|_| project_initialization_error())?;
        let library = self.store.library_snapshot().await.map_err(store_error)?;
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
        let document = pack
            .new_document(shell.clone())
            .map_err(|_| project_initialization_error())?;
        if document.format != shell.format
            || document.format_version != shell.format_version
            || document.scenario_id != shell.scenario_id
            || document.domain_pack != shell.domain_pack
            || document.metadata != shell.metadata
            || document.settings != shell.settings
            || document.extensions != shell.extensions
        {
            return Err(project_initialization_error());
        }
        validate_document_shape(&document).map_err(|_| project_initialization_error())?;
        if pack
            .validate_full(&document)
            .issues
            .iter()
            .any(|issue| issue.severity == ValidationSeverity::Error)
        {
            return Err(project_initialization_error());
        }
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

    async fn apply_scenario(
        &self,
        request_id: RequestId,
        envelope: CommandEnvelope,
        truncate_redo: bool,
    ) -> Result<AppCommandResult, AppError> {
        let scenario_id = envelope.scenario_id;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let journal_envelope = envelope.clone();
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
                move |document| {
                    ensure_supported_document(document, &pack_registry)?;
                    let mut applied = apply_command_with_registry(
                        document,
                        envelope.expected_revision,
                        &envelope,
                        &pack_registry,
                    )
                    .map_err(|error| command_store_error(&error))?;
                    applied.document.metadata.updated_at = applied_at;
                    let result = applied.result.clone();
                    let command = serde_json::to_value(&journal_envelope.command)
                        .map_err(StoreError::Json)?;
                    let inverse = result
                        .inverse
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(StoreError::Json)?;
                    Ok(CommandWrite {
                        document: applied.document,
                        journal: JournalWrite {
                            command_type: applied.command_type,
                            command,
                            command_id: journal_envelope.command_id,
                            inverse,
                            actor: journal_envelope.actor.clone(),
                            source: journal_envelope.source,
                            summary: applied.summary,
                            created_at: applied_at,
                        },
                        output: result,
                    })
                },
            )
            .await
            .map_err(store_error)?;
        let mut output = result.output;
        output.new_revision = result.new_revision;
        self.publish_scenario_events(scenario_id, request_id, &output);
        Ok(AppCommandResult::ScenarioCommand(output))
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
        let apply = move |history: HistoryCommand| {
            history_apply(history, expected_revision, source, &pack_registry)
        };
        let committed = if undo {
            self.store
                .undo(scenario_id, expected_revision, applied_at, apply)
                .await
        } else {
            self.store
                .redo(scenario_id, expected_revision, applied_at, apply)
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
    ) -> Result<AppQueryResult, AppError> {
        let (metadata, bytes) = tokio::task::spawn_blocking(move || {
            let metadata = preflight_bundle_metadata(&bytes, &InspectionPolicy::default())?;
            Ok::<_, eutheto_import::ImportError>((metadata, bytes))
        })
        .await
        .map_err(join_error)?
        .map_err(|error| import_error(&error))?;
        self.check_cancelled()?;
        preflight_import_packs(&metadata, &self.pack_registry)?;
        let registry = Arc::clone(&self.pack_registry);
        let mut inspected =
            tokio::task::spawn_blocking(move || inspect_application_bundle(&bytes, &registry))
                .await
                .map_err(join_error)?
                .map_err(|error| import_error(&error))?;
        self.check_cancelled()?;
        let mut portable_settings =
            prepare_portable_inspection(&mut inspected, options.restore_mode, &self.pack_registry)?;
        let (local, local_settings) = self.local_library_snapshot().await?;
        let mut preview = eutheto_import::build_preview(&inspected, &options, &local)
            .map_err(|error| import_error(&error))?;
        self.check_cancelled()?;
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
        self.check_cancelled()?;
        let preview_id = RequestId::new(self.ids.as_ref()).map_err(id_error)?;
        self.check_cancelled()?;
        self.retain_portable_preview(
            preview_id,
            PendingImportPreview {
                inspected,
                preview: preview.clone(),
                options,
                portable_settings,
                safety_backup_failure: None,
                retained_bytes: 0,
            },
        )
        .await?;
        Ok(AppQueryResult::PortablePreview {
            preview_id,
            preview: Box::new(preview),
        })
    }
    async fn inspect_unopened_bundle(&self, bytes: Vec<u8>) -> Result<AppQueryResult, AppError> {
        let unopened = tokio::task::spawn_blocking(move || {
            inspect_unopened_bundle_for_exact_reexport(&bytes, &InspectionPolicy::default())
        })
        .await
        .map_err(join_error)?
        .map_err(|error| import_error(&error))?;
        self.check_cancelled()?;
        let retained_bytes = unopened.retained_memory_bytes();
        if retained_bytes > MAX_PENDING_PREVIEW_BYTES {
            return Err(protocol_error(
                "portable.preview_too_large",
                "The unopened bundle exceeds the retained preview limit.",
                false,
            ));
        }
        let metadata = unopened.metadata().clone();
        let preview_id = RequestId::new(self.ids.as_ref()).map_err(id_error)?;
        self.check_cancelled()?;
        let mut previews = self.previews.lock().await;
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
        previews.insert(preview_id, PendingPortablePreview::Unopened(unopened));
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
    ) -> Result<AppCommandResult, AppError> {
        let pending = self
            .previews
            .lock()
            .await
            .remove(&preview_id)
            .ok_or(protocol_error(
                "portable.preview_not_found",
                "The portable preview is no longer available.",
                false,
            ))?;
        let PendingPortablePreview::Unopened(unopened) = pending else {
            return Err(protocol_error(
                "portable.preview_capability_mismatch",
                "The opaque portable capability does not authorize exact unopened re-export.",
                false,
            ));
        };
        self.check_cancelled()?;
        let bytes = unopened.into_exact_bytes();
        let cancellation = self.cancellation.clone();
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
        let snapshot = self.store.library_snapshot().await.map_err(store_error)?;
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
    ) -> Result<AppCommandResult, AppError> {
        self.check_cancelled()?;
        let (pending, local) = self.take_pending_portable_preview(preview_id).await?;
        let staged = self.stage_portable_apply(&pending, &local, &collision_plan)?;
        self.check_cancelled()?;
        let committed = self
            .commit_staged_portable(
                preview_id,
                pending,
                local,
                &collision_plan,
                authorization,
                staged,
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
        })
    }

    async fn take_pending_portable_preview(
        &self,
        preview_id: RequestId,
    ) -> Result<(PendingImportPreview, LocalLibrarySnapshot), AppError> {
        let pending = self
            .previews
            .lock()
            .await
            .remove(&preview_id)
            .ok_or(protocol_error(
                "portable.preview_not_found",
                "The portable preview is no longer available.",
                false,
            ))?;
        let PendingPortablePreview::Import(pending) = pending else {
            return Err(protocol_error(
                "portable.preview_capability_mismatch",
                "The opaque portable capability cannot be used for import.",
                false,
            ));
        };
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

    fn stage_portable_apply(
        &self,
        pending: &PendingImportPreview,
        local: &LocalLibrarySnapshot,
        collision_plan: &CollisionPlan,
    ) -> Result<StagedPortableApply, AppError> {
        let import = eutheto_import::stage_import(
            &pending.inspected,
            &pending.preview,
            &pending.options,
            local,
            collision_plan,
        )
        .map_err(|error| import_error(&error))?;
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
            .collect();
        let events = portable_apply_events(&import.scenarios);
        Ok(StagedPortableApply {
            import,
            scenarios,
            events,
        })
    }

    async fn commit_staged_portable(
        &self,
        preview_id: RequestId,
        mut pending: PendingImportPreview,
        local: LocalLibrarySnapshot,
        collision_plan: &CollisionPlan,
        authorization: Option<RestoreAuthorization>,
        staged: StagedPortableApply,
    ) -> Result<CommittedPortableApply, AppError> {
        let StagedPortableApply {
            import,
            scenarios,
            mut events,
        } = staged;
        let applied_at = self.clock.now();
        let outcome = if let Some(authorization) = authorization {
            let restore_authorization = self
                .authorize_portable_restore(preview_id, &mut pending, collision_plan, authorization)
                .await?;
            let remove_scenario_ids = if pending.options.restore_mode == RestoreMode::ReplaceLibrary
            {
                local.scenario_ids.clone()
            } else {
                BTreeSet::new()
            };
            let imported = scenarios
                .iter()
                .map(|scenario| scenario.scenario_id)
                .collect::<BTreeSet<_>>();
            for removed in &local.scenarios {
                if remove_scenario_ids.contains(&removed.scenario_id)
                    && !imported.contains(&removed.scenario_id)
                {
                    events.push((removed.scenario_id, removed.revision, ChangeKind::Removed));
                }
            }
            self.check_cancelled()?;
            self.store
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
                )
                .await
                .map_err(store_error)?
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
            self.check_cancelled()?;
            self.store
                .apply_staged_library(StagedLibraryApply::Import(import), applied_at)
                .await
                .map_err(store_error)?
        };
        Ok(CommittedPortableApply {
            scenarios,
            events,
            library_changed: outcome.library_revision != local.revision,
        })
    }

    async fn authorize_portable_restore(
        &self,
        preview_id: RequestId,
        pending: &mut PendingImportPreview,
        collision_plan: &CollisionPlan,
        authorization: RestoreAuthorization,
    ) -> Result<RestoreAuthorization, AppError> {
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
                Ok(RestoreAuthorization {
                    destructive_action_confirmed: authorization.destructive_action_confirmed,
                    safety_backup: SafetyBackupEvidence::NotRequired,
                    prospective_failure_receipt_token: None,
                    collision_plan_sha256: None,
                })
            }
            RestoreMode::ReplaceLibrary => {
                self.authorize_replace_restore(preview_id, pending, collision_plan, authorization)
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
    ) -> Result<RestoreAuthorization, AppError> {
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
            );
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
                Ok(RestoreAuthorization {
                    destructive_action_confirmed: true,
                    safety_backup: SafetyBackupEvidence::FailedWithStrongConfirmation { proof },
                    prospective_failure_receipt_token: None,
                    collision_plan_sha256: Some(plan_sha256),
                })
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
    ) -> Result<RestoreAuthorization, AppError> {
        let proof = prospective_failure_proof(self.ids.as_ref(), prospective_token)?;
        match self.create_portable_safety_backup().await {
            Ok(bundle_sha256) => Ok(RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup: SafetyBackupEvidence::Verified { bundle_sha256 },
                prospective_failure_receipt_token: None,
                collision_plan_sha256: Some(collision_plan_sha256.to_owned()),
            }),
            Err(error) => {
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
                self.retain_portable_preview(preview_id, pending.clone())
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
        preview_id: RequestId,
        mut pending: PendingImportPreview,
    ) -> Result<(), AppError> {
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
        Ok(())
    }

    async fn create_portable_safety_backup(&self) -> Result<String, AppError> {
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
            )
            .await?;
        let digest = eutheto_export::sha256_hex(&bytes);
        let bundle_id = BundleId::new(self.ids.as_ref()).map_err(id_error)?;
        let destination = self
            .paths
            .safety_backups
            .join(format!("{bundle_id}.eutheto"));
        self.write_bundle(destination.clone(), bytes).await?;
        let registry = Arc::clone(&self.pack_registry);
        let verified_digest = tokio::task::spawn_blocking(move || {
            let published = std::fs::read(destination)?;
            inspect_application_bundle(&published, &registry).map_err(|_| {
                std::io::Error::other("published safety backup verification failed")
            })?;
            Ok::<_, std::io::Error>(eutheto_export::sha256_hex(&published))
        })
        .await
        .map_err(join_error)?
        .map_err(filesystem_error)?;
        if verified_digest != digest {
            return Err(protocol_error(
                "restore.safety_backup_verification_failed",
                "The published safety backup could not be verified.",
                false,
            ));
        }
        Ok(digest)
    }
    async fn cancel_portable_preview(
        &self,
        preview_id: RequestId,
    ) -> Result<AppCommandResult, AppError> {
        if self.previews.lock().await.remove(&preview_id).is_none() {
            return Err(protocol_error(
                "portable.preview_not_found",
                "The portable preview is no longer available.",
                false,
            ));
        }
        Ok(AppCommandResult::PortablePreviewCancelled)
    }

    async fn export_scenario(
        &self,
        scenario_id: ScenarioId,
    ) -> Result<(Vec<u8>, Revision, Revision), AppError> {
        let library = self.store.library_snapshot().await.map_err(store_error)?;
        let project = library
            .projects
            .iter()
            .find(|project| project.document.scenario_id == scenario_id)
            .ok_or(AppError::NotFound(ResourceRef::Scenario(scenario_id)))?;
        let scenario_revision = project.summary.revision;
        let library_revision = library.revision;
        let scenario = portable_scenario(project, false);
        let sections = scenario_export_sections(&library, &scenario)?;
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
            assemble_scenario_export(&snapshot, &|document| {
                encode_portable_domain(document, &registry)
            })
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        self.check_cancelled()?;
        Ok((bytes, scenario_revision, library_revision))
    }
    async fn export_backup(
        &self,
        title: String,
        selection: BackupSelection,
    ) -> Result<(Vec<u8>, BackupSummary, Revision), AppError> {
        if selection.include_audit {
            return Err(AppError::Unsupported(UnsupportedFeature {
                code: "backup.audit_unavailable".to_owned(),
                capability: "Portable backup audit history".to_owned(),
            }));
        }
        let library = self.store.library_snapshot().await.map_err(store_error)?;
        let library_revision = library.revision;
        let (sections, summary) = backup_sections(&library, selection)?;
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
            assemble_full_backup(&snapshot, &|document| {
                encode_portable_domain(document, &registry)
            })
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        self.check_cancelled()?;
        Ok((bytes, summary, library_revision))
    }

    async fn publish_prepared_portable(
        &self,
        destination: PathBuf,
        bytes: Vec<u8>,
        expected_sha256: &str,
        binding: PreparedPortableBinding,
    ) -> Result<AppCommandResult, AppError> {
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
        self.check_cancelled()?;
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
        let cancellation = self.cancellation.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_bundle_atomic_cancellable(&destination, &bytes, &cancellation)
        })
        .await
        .map_err(join_error)?
        .map_err(|error| export_error(&error))?;
        self.check_cancelled()?;
        let cancellation = self.cancellation.clone();
        self.store
            .with_publication_revision_lease(
                expected_library_revision,
                expected_scenario,
                move || prepared.publish_cancellable(&cancellation),
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

    async fn write_bundle(&self, destination: PathBuf, bytes: Vec<u8>) -> Result<(), AppError> {
        let cancellation = self.cancellation.clone();
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
    let migration_registry = Arc::clone(registry);
    let migrations = MigrationRegistries::new(
        Vec::new(),
        vec![PortableMigrationStep {
            from_version: 1,
            to_version: 2,
            name: "global-portable-v1-to-v2",
            migrate: Arc::new(move |value| migrate_global_portable_v1(value, &migration_registry)),
        }],
    )?;
    inspect_bundle(bytes, &InspectionPolicy::default(), &migrations, &|wire| {
        decode_portable_domain(wire, registry)
    })
}

fn portable_conversion_failure() -> MigrationFailure {
    MigrationFailure::Invalid(
        "The registered domain pack could not safely convert this scenario revision.".to_owned(),
    )
}

fn portable_conversion_error(error: AppError) -> MigrationFailure {
    match error {
        AppError::Validation(report) => MigrationFailure::Validation(report),
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
    let domain = encode_portable_domain(&snapshot.document, registry)
        .map_err(|_| portable_conversion_failure())?;
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
                .map_err(|_| portable_conversion_failure())?,
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
        .map_err(|_| portable_conversion_failure())?;
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
        migrated = pack.migrate_document(migrated).map_err(|_| {
            migration_contract_error("The imported scenario domain document could not be migrated.")
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
    let encoded = pack.export_portable(document).map_err(|_| failed())?;
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
) -> Result<(ScenarioDocument, CommandResult), StoreError> {
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
    let mut applied =
        apply_command_with_registry(&history.document, current_revision, &envelope, registry)
            .map_err(|error| command_store_error(&error))?;
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

fn project_summary(project: &eutheto_store::ProjectSummary) -> ProjectSummaryDto {
    ProjectSummaryDto {
        scenario_id: project.id,
        title: project.title.clone(),
        domain_pack_id: project.domain_pack_id.clone(),
        revision: project.revision,
        updated_at: project.updated_at,
        archived: project.archived_at.is_some(),
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

fn ensure_solution_schema(schema_version: u32) -> Result<(), AppError> {
    if schema_version == SOLUTION_API_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(validation_error(
            "solution.schema_version_unsupported",
            "/schemaVersion",
            "The solution request schema version is not supported.",
        ))
    }
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

fn solution_pack<'a>(
    stored: &StoredAcceptedResultV2,
    registry: &'a DomainPackRegistry,
) -> Result<&'a dyn eutheto_domain_api::DomainPack, AppError> {
    available_pack(&stored.document, registry)
        .ok_or_else(|| unsupported_project_pack(&stored.document.domain_pack))
}

fn compare_stored_solutions(
    base: &StoredAcceptedResultV2,
    candidate: &StoredAcceptedResultV2,
) -> Result<SolutionComparisonV1, AppError> {
    compare_accepted_results(
        &base.portable.accepted_result,
        &candidate.portable.accepted_result,
        Some(&ComparisonContext {
            locks: &[],
            manifests: Some(ComparisonRunManifests {
                base: &base.portable.run_manifest,
                candidate: &candidate.portable.run_manifest,
            }),
        }),
    )
    .map_err(|_| {
        validation_error(
            "solution.comparison_invalid",
            "/request",
            "The selected accepted solutions cannot be compared.",
        )
    })
}

fn reverify_accepted_solution(
    stored: &StoredAcceptedResultV2,
    registry: &DomainPackRegistry,
) -> Result<VerificationReport, AppError> {
    let accepted = &stored.portable.accepted_result;
    let solution = &accepted.solution;
    let pack = solution_pack(stored, registry)?;
    let document_hash = serde_json::to_vec(&stored.document)
        .map(|bytes| blake3_hex(&bytes))
        .map_err(|_| solution_contract_error())?;
    if document_hash != stored.portable.run_input.snapshot_document_hash
        || document_hash != accepted.verification.document_hash
        || solution.pack_id != stored.document.domain_pack.id
        || solution.scenario_id != stored.document.scenario_id
        || solution.scenario_revision != stored.portable.scenario_revision
    {
        return Err(solution_verification_mismatch());
    }
    let scope = pack
        .verification_scope(&stored.document, solution.scenario_revision)
        .map_err(|_| solution_verification_failed())?;
    if scope.checksum != accepted.verification.verification_scope_checksum {
        return Err(solution_verification_mismatch());
    }
    let context = VerificationContextV1::new(
        solution.scenario_id,
        solution.scenario_revision,
        document_hash,
        stored.portable.run_input.model_hash.clone(),
        solution
            .canonical_hash()
            .map_err(|_| solution_verification_mismatch())?,
        scope.checksum,
    )
    .map_err(|_| solution_verification_mismatch())?;
    let authoritative_score = pack
        .score(&stored.document, solution)
        .map_err(|_| solution_verification_failed())?;
    let report = pack
        .verify(&stored.document, solution, &context, &authoritative_score)
        .map_err(|_| solution_verification_failed())?;
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
    Ok(report)
}

fn solution_view_error(error: &DomainPackError) -> AppError {
    match error {
        DomainPackError::UnsupportedExplanationCapability(_) => explanation_unavailable("view"),
        _ => validation_error(
            "solution.view_invalid",
            "/viewId",
            "The selected result view is unavailable for this accepted solution.",
        ),
    }
}

fn explanation_render_error(error: &DomainPackError) -> AppError {
    match error {
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

fn solution_contract_error() -> AppError {
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
    })
}

fn solution_verification_mismatch() -> AppError {
    AppError::Verification(VerificationFailure {
        code: "solution.reverification_mismatch".to_owned(),
        message: "Fresh independent verification did not match the retained accepted result."
            .to_owned(),
        retryable: false,
        diagnostic_id: None,
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

fn command_store_error(error: &CommandError) -> StoreError {
    StoreError::CommandApplication {
        code: error.code().to_owned(),
        message: error.to_string(),
    }
}

fn store_error(error: StoreError) -> AppError {
    match error {
        StoreError::ScenarioNotFound(id) => AppError::NotFound(ResourceRef::Scenario(id)),
        StoreError::AcceptedResultNotFound(id) => AppError::NotFound(ResourceRef::Solution(id)),
        StoreError::CounterfactualJobNotFound(_) => validation_error(
            "solution.counterfactual_not_found",
            "/request/subject/jobId",
            "The selected completed counterfactual was not found.",
        ),
        StoreError::Conflict { expected, actual }
        | StoreError::LibraryConflict { expected, actual } => AppError::Conflict {
            expected_revision: expected,
            actual_revision: actual,
        },
        StoreError::ScenarioAlreadyExists(_) => validation_error(
            "project.scenario_already_exists",
            "/scenarioId",
            "A project with this scenario identity already exists.",
        ),
        StoreError::CommandApplication { code, message } => {
            if code == "command.unsupported" {
                AppError::Unsupported(UnsupportedFeature {
                    code,
                    capability: message,
                })
            } else {
                validation_error(&code, "/command", &message)
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
        DeferredCapability::Solution => ("capability.solution_unavailable", "Solution services"),
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

fn filesystem_error(_: std::io::Error) -> AppError {
    AppError::Storage(StorageFailure {
        code: "storage.directory_initialization_failed".to_owned(),
        message: "The application data directories could not be initialized.".to_owned(),
        retryable: false,
        diagnostic_id: None,
    })
}

fn export_error(error: &ExportError) -> AppError {
    match error {
        ExportError::Cancelled => {
            protocol_error("operation.cancelled", "The operation was cancelled.", false)
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
            filesystem_error(std::io::Error::other("blocked safety backup directory"));
        assert_eq!(
            safe_backup_failure_reason(&directory_failure).as_deref(),
            Some("The application data directories could not be initialized.")
        );
        let unsafe_private_path = store_error(StoreError::PrivatePath(std::io::Error::other(
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
