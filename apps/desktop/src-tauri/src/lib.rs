use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    BackendSupportColumn, BackupAssetSelection, BackupSelection, DeferredCapability, EuthetoApp,
    PreparedPortableBinding, ProjectScope, SolutionCancelCounterfactualRequestV1,
    SolutionCompareRequestV1, SolutionExplainRequestV1, SolutionListRequestV1,
    SolutionSelectRequestV1, SolutionStartCounterfactualRequestV1, SolutionSummaryRequestV1,
    SolutionVerifyRequestV1, SolutionViewRequestV1, SolverSupportMatrixMetadata, SupportCell,
    SupportFeature, SupportFeatureId,
};
use eutheto_export::{
    ApplicationMetadata, BackupSelectionScope, BundleKind, FixedExclusion, OmittedAssetReason,
    PORTABLE_LIMITS, PortableBackupAssetSelection,
};
use eutheto_import::{
    CollisionPlan, ImportOptions, ImportPreview, MigrationRegistryKind, MigrationSubject,
    PackMigrationVersionSpace, RestoreAuthorization, SafetyBackupEvidence,
};
use eutheto_types::{
    ActorRef, ApiErrorCategoryDto, ApiErrorDto, ApiResponseDto, AppError, BackendId,
    CancellationToken, CommandBatch, CommandEnvelope, CommandId, CommandResult, CommandSource,
    DomainPackRef, EventTopic, FieldErrorDto, FoundationStatus, PackId, ProjectMetadataDto,
    ProjectSummaryDto, RequestId, ResourceRef, Revision, Rfc3339Timestamp, SafeDiagnosticValue,
    ScenarioCommand, ScenarioId, ScenarioSettings, SupplementalIdentity, SupportPreviewDto,
    SystemClock, SystemIdGenerator, ValidationIssue, ValidationReport,
};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

#[cfg(feature = "bundled-ortools")]
mod bundled_solver;

#[macro_use]
mod generated_command_catalog;
mod operations;
use operations::{OperationClaim, OperationContextV1, OperationPhaseV1, OperationRegistry};
#[macro_use]
mod setup_boundary;
use setup_boundary::{
    operation_cancel, operation_prepare, operation_release, scenario_get_entity,
    scenario_get_rule_catalog, scenario_get_setup_status, scenario_get_summary, scenario_get_view,
    scenario_search_entities, scenario_validate, workforce_apply_reviewed_generation,
};

use generated_command_catalog::REGISTERED_COMMANDS;
const MAX_PREPARED_PORTABLE_OUTPUTS: usize = 3;

#[derive(Clone)]
enum PreparedPortableKind {
    Scenario {
        scenario_id: ScenarioId,
        revision: Revision,
        library_revision: Revision,
        title: String,
    },
    Backup {
        library_revision: Revision,
        title: String,
        summary: BackupSummaryDto,
    },
}

struct PreparedPortableOutput {
    bytes: Vec<u8>,
    sha256: String,
    kind: PreparedPortableKind,
}

#[derive(Default)]
struct PreparedPortableCache {
    entries: VecDeque<(RequestId, PreparedPortableOutput)>,
    total_bytes: usize,
}

impl PreparedPortableCache {
    fn insert(
        &mut self,
        preview_id: RequestId,
        output: PreparedPortableOutput,
    ) -> Result<(), ApiError> {
        let max_total_bytes =
            usize::try_from(PORTABLE_LIMITS.max_archive_bytes).map_err(|_| -> ApiError {
                boundary_error(
                    "portable.limit_unrepresentable",
                    "The portable publication limit cannot be represented on this platform.",
                    None,
                )
                .into()
            })?;
        let byte_length = output.bytes.len();
        if byte_length > max_total_bytes {
            return Err(boundary_error(
                "portable.preview_too_large",
                "The prepared portable output exceeds the publication limit.",
                None,
            )
            .into());
        }
        while self.entries.len() >= MAX_PREPARED_PORTABLE_OUTPUTS
            || self.total_bytes.saturating_add(byte_length) > max_total_bytes
        {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.bytes.len());
        }
        self.total_bytes = self.total_bytes.saturating_add(byte_length);
        self.entries.push_back((preview_id, output));
        Ok(())
    }

    fn get(&self, preview_id: RequestId) -> Option<&PreparedPortableOutput> {
        self.entries
            .iter()
            .find_map(|(candidate, output)| (*candidate == preview_id).then_some(output))
    }

    fn remove(&mut self, preview_id: RequestId) -> Option<PreparedPortableOutput> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _)| *candidate == preview_id)?;
        let (_, output) = self.entries.remove(index)?;
        self.total_bytes = self.total_bytes.saturating_sub(output.bytes.len());
        Some(output)
    }
}

fn prepared_output_error(code: &'static str) -> ApiErrorDto {
    boundary_error(
        code,
        "The prepared portable output is unavailable or no longer matches this request.",
        None,
    )
}

fn new_prepared_preview_id() -> Result<RequestId, ApiError> {
    RequestId::new(&SystemIdGenerator).map_err(|_| {
        boundary_error(
            "identity.generation_failed",
            "A unique portable preview identity could not be generated.",
            None,
        )
        .into()
    })
}

const API_SCHEMA_VERSION: u32 = 1;
const PORTABLE_EXTENSION: &str = ".eutheto";

macro_rules! make_tauri_handler {
    ($($command:ident),* $(,)?) => {
        tauri::generate_handler![$($command),*]
    };
}

#[derive(Clone)]
struct DesktopState {
    app: EuthetoApp,
    cache_dir: PathBuf,
    backup_dir: PathBuf,
    prepared_outputs: Arc<tokio::sync::Mutex<PreparedPortableCache>>,
    operations: Arc<OperationRegistry>,
}

impl DesktopState {
    fn new(app: EuthetoApp, cache_dir: PathBuf, backup_dir: PathBuf) -> Self {
        let operations = Arc::new(OperationRegistry::new(
            app.clone(),
            Arc::new(SystemClock),
            Arc::new(eutheto_types::SystemMonotonicClock::new()),
            Arc::new(SystemIdGenerator),
            ["main".to_owned()],
        ));
        Self {
            app,
            cache_dir,
            backup_dir,
            prepared_outputs: Arc::default(),
            operations,
        }
    }
}

type ApiError = Box<ApiErrorDto>;
type ApiResult<T> = Result<ApiResponseDto<T>, ApiError>;
type SolutionApiResult = Result<tauri::ipc::Response, ApiError>;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestOnly {
    request_id: RequestId,
}

#[derive(Clone, Debug)]
struct CorrelatedRequest<T> {
    request_id: RequestId,
    operation: T,
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for CorrelatedRequest<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let Value::Object(mut fields) = Value::deserialize(deserializer)? else {
            return Err(D::Error::custom("request must be an object"));
        };
        let request_id = fields
            .remove("requestId")
            .ok_or_else(|| D::Error::missing_field("requestId"))
            .and_then(|value| serde_json::from_value(value).map_err(D::Error::custom))?;
        let operation = serde_json::from_value(Value::Object(fields)).map_err(D::Error::custom)?;
        Ok(Self {
            request_id,
            operation,
        })
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackDescribeRequest {
    request_id: RequestId,
    pack_id: PackId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolverDescribeRequest {
    request_id: RequestId,
    backend_id: BackendId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectListRequest {
    request_id: RequestId,
    scope: ProjectScopeDto,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ProjectScopeDto {
    Active,
    Archived,
    All,
}

impl From<ProjectScopeDto> for ProjectScope {
    fn from(value: ProjectScopeDto) -> Self {
        match value {
            ProjectScopeDto::Active => Self::Active,
            ProjectScopeDto::Archived => Self::Archived,
            ProjectScopeDto::All => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenarioRequest {
    request_id: RequestId,
    scenario_id: ScenarioId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateProjectRequest {
    request_id: RequestId,
    title: String,
    description: String,
    domain_pack: DomainPackRef,
    settings: ScenarioSettings,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DuplicateProjectRequest {
    request_id: RequestId,
    source_id: ScenarioId,
    expected_revision: Revision,
    title: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectMutationRequest {
    request_id: RequestId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortablePreviewRequest {
    request_id: RequestId,
    options: ImportOptions,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortableApplyRequest {
    request_id: RequestId,
    preview_id: RequestId,
    collision_plan: CollisionPlan,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortableCancelRequest {
    request_id: RequestId,
    preview_id: RequestId,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_field_names)]
struct ExportCreateRequest {
    request_id: RequestId,
    scenario_id: ScenarioId,
    preview_id: RequestId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupPreviewRequest {
    request_id: RequestId,
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupCreateRequest {
    request_id: RequestId,
    title: String,
    preview_id: RequestId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreApplyRequest {
    request_id: RequestId,
    preview_id: RequestId,
    collision_plan: CollisionPlan,
    authorization: RestoreAuthorizationDto,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreAuthorizationDto {
    destructive_action_confirmed: bool,
    safety_backup_bypass_phrase: Option<String>,
}

impl From<RestoreAuthorizationDto> for RestoreAuthorization {
    fn from(value: RestoreAuthorizationDto) -> Self {
        let safety_backup = value
            .safety_backup_bypass_phrase
            .map_or(SafetyBackupEvidence::NotRequired, |proof| {
                SafetyBackupEvidence::FailedWithStrongConfirmation { proof }
            });
        Self {
            destructive_action_confirmed: value.destructive_action_confirmed,
            safety_backup,
            prospective_failure_receipt_token: None,
            collision_plan_sha256: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyScenarioCommandRequest {
    request_id: RequestId,
    command_id: CommandId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    actor: ActorRef,
    command: ScenarioCommand,
    truncate_redo: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyScenarioBatchRequest {
    request_id: RequestId,
    command_id: CommandId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    actor: ActorRef,
    label: Option<String>,
    commands: Vec<ScenarioCommand>,
    truncate_redo: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryMutationRequest {
    request_id: RequestId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingRequest {
    request_id: RequestId,
    key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingUpdateRequest {
    request_id: RequestId,
    key: String,
    value: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmptyDto {}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryRefreshRequiredDto {
    reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfoDto {
    name: &'static str,
    version: &'static str,
    foundation: FoundationStatus,
    portable_extension: &'static str,
    portable_extension_status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppCapabilitiesDto {
    available_commands: Vec<&'static str>,
    unavailable_commands: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppPathsSummaryDto {
    app_data_configured: bool,
    cache_configured: bool,
    backup_configured: bool,
}

impl From<State<'_, DesktopState>> for AppPathsSummaryDto {
    fn from(state: State<'_, DesktopState>) -> Self {
        Self {
            app_data_configured: true,
            cache_configured: state.cache_dir.is_absolute(),
            backup_configured: state.backup_dir.is_absolute(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DomainPackMetadataDto {
    descriptor: Value,
    catalog: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolverSupportCellDto {
    feature_id: SupportFeatureId,
    #[serde(flatten)]
    cell: SupportCell,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolverSupportBackendColumnDto {
    backend_id: BackendId,
    backend_version: String,
    adapter_version: String,
    cells: Vec<SolverSupportCellDto>,
}

impl From<BackendSupportColumn> for SolverSupportBackendColumnDto {
    fn from(column: BackendSupportColumn) -> Self {
        Self {
            backend_id: column.backend_id,
            backend_version: column.backend_version,
            adapter_version: column.adapter_version,
            cells: column
                .cells
                .into_iter()
                .map(|(feature_id, cell)| SolverSupportCellDto { feature_id, cell })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolverSupportMatrixDto {
    schema_version: u32,
    planning_ir_schema_version: u32,
    features: Vec<SupportFeature>,
    production_backend_ids: Vec<BackendId>,
    backend_columns: Vec<SolverSupportBackendColumnDto>,
}

fn solver_support_matrix_dto(matrix: SolverSupportMatrixMetadata) -> SolverSupportMatrixDto {
    SolverSupportMatrixDto {
        schema_version: matrix.schema_version,
        planning_ir_schema_version: matrix.planning_ir_schema_version,
        features: matrix.features,
        production_backend_ids: matrix.production_backend_ids,
        backend_columns: matrix
            .backend_columns
            .into_iter()
            .map(SolverSupportBackendColumnDto::from)
            .collect(),
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnopenedBundleScenarioDto {
    path: String,
    scenario_id: Option<String>,
    pack_id: Option<String>,
    internal_pack_schema_version: Option<u32>,
    portable_pack_schema_version: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnopenedBundleMetadataDto {
    file_sha256: String,
    format: String,
    format_version: u32,
    portable_schema_version: u32,
    bundle_kind: Option<String>,
    title: Option<String>,
    required_capabilities: Vec<PortableCapabilityDto>,
    scenarios: Vec<UnopenedBundleScenarioDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnopenedBundlePreviewDto {
    preview_id: RequestId,
    metadata: UnopenedBundleMetadataDto,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableCountsDto {
    scenarios: u64,
    scenario_revisions: u64,
    results: u64,
    shared_records: u64,
    preferences: u64,
    assets: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableCapabilityDto {
    id: String,
    version: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceBackupSelectionDto {
    include_results: bool,
    asset_selection: &'static str,
    threshold_version: Option<u32>,
    threshold_bytes: Option<u64>,
    excluded_asset_count: u64,
    excluded_asset_ids: Vec<String>,
    fixed_exclusions: Vec<&'static str>,
    scope: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OmittedAssetDto {
    asset_id: String,
    format: String,
    version: u32,
    reason: &'static str,
    original_media_type: String,
    original_size: u64,
    content_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableScenarioDto {
    scenario_id: ScenarioId,
    title: String,
    collides: bool,
    source_revision: Revision,
    same_identity_revision: Revision,
    same_identity_revision_warning: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemovedScenarioDto {
    scenario_id: ScenarioId,
    title: String,
    revision: Revision,
    archived: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppliedMigrationDto {
    registry: &'static str,
    name: String,
    from_version: u32,
    to_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    version_space: Option<PackMigrationVersionSpace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<MigrationSubject>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortablePreviewDto {
    preview_id: RequestId,
    bundle_id: String,
    bundle_kind: &'static str,
    title: String,
    created_at: String,
    source_application: ApplicationMetadata,
    source_format_version: u32,
    source_schema_version: u32,
    counts: PortableCountsDto,
    required_capabilities: Vec<PortableCapabilityDto>,
    preserved_extensions: Vec<String>,
    included_sections: Vec<String>,
    excluded_sections: Vec<String>,
    source_backup_selection: Option<SourceBackupSelectionDto>,
    omitted_assets: Vec<OmittedAssetDto>,
    scenarios: Vec<PortableScenarioDto>,
    supplemental_collisions: Vec<SupplementalIdentity>,
    removed_scenarios: Vec<RemovedScenarioDto>,
    removed_supplemental: Vec<SupplementalIdentity>,
    settings_changed: Vec<String>,
    settings_removed: Vec<String>,
    applied_migrations: Vec<AppliedMigrationDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableAppliedDto {
    scenario_ids: Vec<ScenarioId>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupSummaryDto {
    include_results: bool,
    asset_selection: &'static str,
    excluded_asset_count: u64,
    excluded_asset_ids: Vec<String>,
    exclusion_scope: Option<String>,
    threshold_version: Option<u32>,
    threshold_bytes: Option<u64>,
    fixed_exclusions: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableFilePreviewDto {
    title: String,
    byte_length: usize,
    backup_summary: Option<BackupSummaryDto>,
    preview_id: RequestId,
    digest: String,
    current_revision: Option<Revision>,
    library_revision: Option<Revision>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableArtifactDto {
    artifact_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioCommandCatalogDto {
    command_types: &'static [&'static str],
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntryDto {
    id: CommandId,
    revision_before: Revision,
    revision_after: Revision,
    command_type: String,
    command: Value,
    inverse: Option<Value>,
    actor: ActorRef,
    source: CommandSource,
    summary: String,
    created_at: Rfc3339Timestamp,
    history_sequence: u64,
    branch_generation: u64,
    applied: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryDto {
    entries: Vec<HistoryEntryDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingEntryDto {
    value: Value,
    updated_at: Rfc3339Timestamp,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingValueDto {
    setting: Option<SettingEntryDto>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingResetDto {
    existed: bool,
}

fn response<T>(
    request_id: RequestId,
    current_revision: Option<Revision>,
    warnings: Vec<ValidationIssue>,
    result: T,
) -> ApiResponseDto<T> {
    ApiResponseDto {
        schema_version: API_SCHEMA_VERSION,
        request_id,
        current_revision,
        warnings,
        result,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct JavascriptIntegerFormatter;

impl serde_json::ser::Formatter for JavascriptIntegerFormatter {
    fn write_i64<W>(&mut self, writer: &mut W, value: i64) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        write!(writer, "\"{value}\"")
    }

    fn write_u64<W>(&mut self, writer: &mut W, value: u64) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if value > 9_007_199_254_740_991 {
            write!(writer, "\"{value}\"")
        } else {
            write!(writer, "{value}")
        }
    }
}

fn solution_response<T: Serialize>(
    request_id: RequestId,
    current_revision: Revision,
    result: T,
) -> SolutionApiResult {
    let envelope = response(request_id, Some(current_revision), Vec::new(), result);
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, JavascriptIntegerFormatter);
    envelope
        .serialize(&mut serializer)
        .map_err(|_| -> ApiError {
            boundary_error(
                "protocol.response_serialization_failed",
                "The solution response could not be translated safely.",
                None,
            )
            .into()
        })?;
    let json = String::from_utf8(bytes).map_err(|_| -> ApiError {
        boundary_error(
            "protocol.response_serialization_failed",
            "The solution response could not be translated safely.",
            None,
        )
        .into()
    })?;
    Ok(tauri::ipc::Response::new(json))
}

fn invalid_solution_request() -> ApiErrorDto {
    boundary_error(
        "solution.request_invalid",
        "The solution request is missing or malformed.",
        Some("/request"),
    )
}

fn decode_solution_value<T: DeserializeOwned>(request: Value) -> Result<T, ApiError> {
    serde_json::from_value(request).map_err(|_| Box::new(invalid_solution_request()))
}

fn decode_solution_request<T: DeserializeOwned>(request: Option<Value>) -> Result<T, ApiError> {
    decode_solution_value(request.ok_or_else(|| Box::new(invalid_solution_request()))?)
}

fn normalize_i64_string(value: &mut Value) -> Result<(), ApiError> {
    let Value::String(decimal) = value else {
        return Err(Box::new(invalid_solution_request()));
    };
    let parsed = decimal
        .parse::<i64>()
        .map_err(|_| Box::new(invalid_solution_request()))?;
    *value = Value::Number(parsed.into());
    Ok(())
}

fn normalize_counterfactual_int64(request: &mut Value) -> Result<(), ApiError> {
    let Some(assignment_value) = request.pointer_mut("/condition/value") else {
        return Ok(());
    };
    let Value::Object(assignment) = assignment_value else {
        return Ok(());
    };
    match assignment.get("type").and_then(Value::as_str) {
        Some("integer") => assignment
            .get_mut("value")
            .ok_or_else(|| Box::new(invalid_solution_request()))
            .and_then(normalize_i64_string),
        Some("interval") => {
            let Some(Value::Object(interval)) = assignment.get_mut("value") else {
                return Err(Box::new(invalid_solution_request()));
            };
            for field in ["start", "duration", "end"] {
                normalize_i64_string(
                    interval
                        .get_mut(field)
                        .ok_or_else(|| Box::new(invalid_solution_request()))?,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn decode_counterfactual_start_request(
    request: Option<Value>,
) -> Result<SolutionStartCounterfactualRequestV1, ApiError> {
    let mut request = request.ok_or_else(|| Box::new(invalid_solution_request()))?;
    normalize_counterfactual_int64(&mut request)?;
    decode_solution_value(request)
}
fn metadata_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|_| {
        boundary_error(
            "protocol.metadata_serialization_failed",
            "The application metadata could not be translated safely.",
            None,
        )
        .into()
    })
}

fn revision_diagnostic_value(revision: u64) -> SafeDiagnosticValue {
    match i64::try_from(revision) {
        Ok(revision) => SafeDiagnosticValue::Integer(revision),
        Err(_) => SafeDiagnosticValue::Text(revision.to_string()),
    }
}

fn validation_error(report: ValidationReport) -> ApiErrorDto {
    let first = report.issues.first();
    ApiErrorDto {
        code: first.map_or_else(|| "validation.failed".to_owned(), |item| item.code.clone()),
        message: first.map_or_else(
            || "The request contains invalid values.".to_owned(),
            |item| item.message.clone(),
        ),
        category: ApiErrorCategoryDto::Validation,
        retryable: false,
        field_errors: report
            .issues
            .into_iter()
            .filter_map(|issue| {
                issue.field_path.map(|field| FieldErrorDto {
                    field,
                    code: issue.code,
                    message: issue.message,
                })
            })
            .collect(),
        details: None,
        diagnostic_id: None,
    }
}

fn map_app_error(error: AppError) -> ApiErrorDto {
    match error {
        AppError::Validation(report) => validation_error(report),
        AppError::Conflict {
            expected_revision,
            actual_revision,
        } => ApiErrorDto {
            code: "scenario.revision_conflict".to_owned(),
            message: "The scenario changed; reload authoritative state and try again.".to_owned(),
            category: ApiErrorCategoryDto::Conflict,
            retryable: false,
            field_errors: Vec::new(),
            details: Some(BTreeMap::from([
                (
                    "expectedRevision".to_owned(),
                    revision_diagnostic_value(expected_revision.value()),
                ),
                (
                    "currentRevision".to_owned(),
                    revision_diagnostic_value(actual_revision.value()),
                ),
            ])),
            diagnostic_id: None,
        },
        AppError::NotFound(resource) => ApiErrorDto {
            code: "resource.not_found".to_owned(),
            message: "The requested resource does not exist.".to_owned(),
            category: ApiErrorCategoryDto::NotFound,
            retryable: false,
            field_errors: Vec::new(),
            details: Some(BTreeMap::from([(
                "resource".to_owned(),
                SafeDiagnosticValue::Text(resource_label(&resource).to_owned()),
            )])),
            diagnostic_id: None,
        },
        AppError::Unsupported(failure) => ApiErrorDto {
            code: failure.code,
            message: format!("{} is not available in this phase.", failure.capability),
            category: ApiErrorCategoryDto::Unsupported,
            retryable: false,
            field_errors: Vec::new(),
            details: Some(BTreeMap::from([(
                "capability".to_owned(),
                SafeDiagnosticValue::Text(failure.capability),
            )])),
            diagnostic_id: None,
        },
        AppError::Solver(failure) => failure_error(
            failure.code,
            failure.message,
            ApiErrorCategoryDto::Solver,
            failure.retryable,
            failure.diagnostic_id,
        ),
        AppError::Verification(failure) => failure_error(
            failure.code,
            failure.message,
            ApiErrorCategoryDto::Verification,
            failure.retryable,
            failure.diagnostic_id,
        ),
        AppError::Storage(failure) => failure_error(
            failure.code,
            failure.message,
            ApiErrorCategoryDto::Storage,
            failure.retryable,
            failure.diagnostic_id,
        ),
        AppError::Protocol(failure) => failure_error(
            failure.code,
            failure.message,
            ApiErrorCategoryDto::Protocol,
            failure.retryable,
            failure.diagnostic_id,
        ),
        AppError::Ai(failure) => failure_error(
            failure.code,
            failure.message,
            ApiErrorCategoryDto::Ai,
            failure.retryable,
            failure.diagnostic_id,
        ),
        AppError::Internal { incident_id } => ApiErrorDto {
            code: "internal.unexpected".to_owned(),
            message: "An unexpected internal error occurred.".to_owned(),
            category: ApiErrorCategoryDto::Internal,
            retryable: false,
            field_errors: Vec::new(),
            details: None,
            diagnostic_id: Some(incident_id),
        },
    }
}

fn failure_error(
    code: String,
    message: String,
    category: ApiErrorCategoryDto,
    retryable: bool,
    diagnostic_id: Option<RequestId>,
) -> ApiErrorDto {
    ApiErrorDto {
        code,
        message,
        category,
        retryable,
        field_errors: Vec::new(),
        details: None,
        diagnostic_id,
    }
}

fn resource_label(resource: &ResourceRef) -> &'static str {
    match resource {
        ResourceRef::Scenario(_) => "scenario",
        ResourceRef::Person(_) => "person",
        ResourceRef::Rule(_) => "rule",
        ResourceRef::Assignment(_) => "assignment",
        ResourceRef::SolveRun(_) => "solveRun",
        ResourceRef::Solution(_) => "solution",
        ResourceRef::Pack(_) => "pack",
        ResourceRef::Backend(_) => "backend",
    }
}

fn boundary_error(code: &str, message: &str, field: Option<&str>) -> ApiErrorDto {
    ApiErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        category: if field.is_some() {
            ApiErrorCategoryDto::Validation
        } else {
            ApiErrorCategoryDto::Protocol
        },
        retryable: false,
        field_errors: field.map_or_else(Vec::new, |field| {
            vec![FieldErrorDto {
                field: field.to_owned(),
                code: code.to_owned(),
                message: message.to_owned(),
            }]
        }),
        details: None,
        diagnostic_id: None,
    }
}

fn unavailable_error(code: &str, capability: &str) -> ApiErrorDto {
    ApiErrorDto {
        code: code.to_owned(),
        message: format!("{capability} is not available in this phase."),
        category: ApiErrorCategoryDto::Unsupported,
        retryable: false,
        field_errors: Vec::new(),
        details: Some(BTreeMap::from([(
            "capability".to_owned(),
            SafeDiagnosticValue::Text(capability.to_owned()),
        )])),
        diagnostic_id: None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeFileError {
    Cancelled,
    Conversion,
    InvalidExtension,
    InvalidFileType,
    Unreadable,
    TooLarge,
    MissingBasename,
}

fn map_native_file_error(error: NativeFileError) -> ApiErrorDto {
    let (code, message, category, retryable) = match error {
        NativeFileError::Cancelled => (
            "operation.cancelled",
            "No file was selected.",
            ApiErrorCategoryDto::Protocol,
            true,
        ),
        NativeFileError::Conversion => (
            "portable.selection_invalid",
            "The selected file could not be accessed.",
            ApiErrorCategoryDto::Protocol,
            false,
        ),
        NativeFileError::InvalidExtension => (
            "portable.extension_invalid",
            "Select an Eutheto portable file.",
            ApiErrorCategoryDto::Validation,
            false,
        ),
        NativeFileError::InvalidFileType => (
            "portable.file_type_invalid",
            "The selected portable file must be a regular file and must not be a link.",
            ApiErrorCategoryDto::Validation,
            false,
        ),
        NativeFileError::Unreadable => (
            "portable.artifact_unreadable",
            "The selected portable file could not be read.",
            ApiErrorCategoryDto::Storage,
            false,
        ),
        NativeFileError::TooLarge => (
            "portable.archive_too_large",
            "The selected portable file exceeds the supported size limit.",
            ApiErrorCategoryDto::Storage,
            false,
        ),
        NativeFileError::MissingBasename => (
            "portable.destination_invalid",
            "The selected destination does not have a valid file name.",
            ApiErrorCategoryDto::Validation,
            false,
        ),
    };
    ApiErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        category,
        retryable,
        field_errors: Vec::new(),
        details: None,
        diagnostic_id: None,
    }
}

async fn native_file_task<T, F>(task: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, NativeFileError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|_| {
            boundary_error(
                "portable.dialog_failed",
                "The native file dialog could not be completed.",
                None,
            )
        })?
        .map_err(|error| Box::new(map_native_file_error(error)))
}

fn require_portable_path(path: PathBuf) -> Result<PathBuf, NativeFileError> {
    let is_portable = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(&PORTABLE_EXTENSION[1..]));
    if is_portable {
        Ok(path)
    } else {
        Err(NativeFileError::InvalidExtension)
    }
}

fn read_bounded_portable(path: &Path) -> Result<Vec<u8>, NativeFileError> {
    #[cfg(not(windows))]
    let selected_metadata =
        std::fs::symlink_metadata(path).map_err(|_| NativeFileError::Unreadable)?;
    #[cfg(not(windows))]
    if selected_metadata.file_type().is_symlink() || !selected_metadata.is_file() {
        return Err(NativeFileError::InvalidFileType);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the final selected directory entry itself rather than traversing a reparse point.
        // This is the Win32 FILE_FLAG_OPEN_REPARSE_POINT value.
        options.custom_flags(0x0020_0000);
    }
    let file = options
        .open(path)
        .map_err(|_| NativeFileError::Unreadable)?;
    let opened_metadata = file.metadata().map_err(|_| NativeFileError::Unreadable)?;
    if !opened_metadata.is_file() {
        return Err(NativeFileError::InvalidFileType);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if selected_metadata.dev() != opened_metadata.dev()
            || selected_metadata.ino() != opened_metadata.ino()
        {
            return Err(NativeFileError::Unreadable);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT covers symlinks and other name-surrogate final entries.
        // The native dialog returns only a path, so this single resolution is authoritative:
        // inspect and read the same handle without reopening the selected path.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if opened_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(NativeFileError::Unreadable);
        }
    }
    if opened_metadata.len() > PORTABLE_LIMITS.max_archive_bytes {
        return Err(NativeFileError::TooLarge);
    }
    let max_capacity = usize::try_from(PORTABLE_LIMITS.max_archive_bytes)
        .map_err(|_| NativeFileError::TooLarge)?;
    let capacity = usize::try_from(opened_metadata.len())
        .map_err(|_| NativeFileError::TooLarge)?
        .min(max_capacity);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(PORTABLE_LIMITS.max_archive_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| NativeFileError::Unreadable)?;
    if u64::try_from(bytes.len()).map_or(true, |byte_length| {
        byte_length > PORTABLE_LIMITS.max_archive_bytes
    }) {
        return Err(NativeFileError::TooLarge);
    }
    Ok(bytes)
}

fn selected_basename(path: &Path) -> Result<String, NativeFileError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or(NativeFileError::MissingBasename)
}

fn suggested_portable_filename(title: &str, fallback: &str) -> String {
    let mut stem = String::new();
    let mut pending_separator = false;
    for character in title.chars() {
        if character.is_alphanumeric() || matches!(character, '-' | '_') {
            if pending_separator && !stem.is_empty() && !stem.ends_with('-') {
                stem.push('-');
            }
            pending_separator = false;
            stem.push(character);
        } else {
            pending_separator = true;
        }
    }
    let mut stem = stem.trim_matches(['-', '_']).to_owned();
    if stem.is_empty()
        || matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
    {
        fallback.clone_into(&mut stem);
    }
    let max_stem_bytes = PORTABLE_LIMITS
        .max_path_bytes
        .saturating_sub(PORTABLE_EXTENSION.len());
    if stem.len() > max_stem_bytes {
        let mut boundary = max_stem_bytes;
        while !stem.is_char_boundary(boundary) {
            boundary -= 1;
        }
        stem.truncate(boundary);
        stem = stem.trim_end_matches(['-', '_']).to_owned();
    }
    format!("{stem}{PORTABLE_EXTENSION}")
}

fn source_backup_selection(selection: eutheto_export::BackupSelection) -> SourceBackupSelectionDto {
    SourceBackupSelectionDto {
        include_results: selection.include_results,
        asset_selection: match selection.asset_selection {
            PortableBackupAssetSelection::All => "all",
            PortableBackupAssetSelection::ExcludeAll => "exclude-all",
            PortableBackupAssetSelection::V1Threshold => "v1-threshold",
        },
        threshold_version: selection.threshold_version,
        threshold_bytes: selection.threshold_bytes,
        excluded_asset_count: selection.excluded_asset_count,
        excluded_asset_ids: selection.excluded_asset_ids.into_iter().collect(),
        fixed_exclusions: selection
            .fixed_exclusions
            .into_iter()
            .map(fixed_exclusion_label)
            .collect(),
        scope: match selection.scope {
            BackupSelectionScope::Scenario => "scenario",
            BackupSelectionScope::Library => "library",
        },
    }
}

fn portable_preview(preview_id: RequestId, preview: ImportPreview) -> PortablePreviewDto {
    PortablePreviewDto {
        preview_id,
        bundle_id: preview.bundle_id.to_string(),
        bundle_kind: match preview.bundle_kind {
            BundleKind::ScenarioExport => "scenario-export",
            BundleKind::FullBackup => "full-backup",
        },
        title: preview.title,
        created_at: preview.created_at,
        source_application: preview.source_application,
        source_format_version: preview.source_format_version,
        source_schema_version: preview.source_schema_version,
        counts: PortableCountsDto {
            scenarios: preview.counts.scenarios,
            scenario_revisions: preview.counts.scenario_revisions,
            results: preview.counts.results,
            shared_records: preview.counts.shared_records,
            preferences: preview.counts.preferences,
            assets: preview.counts.assets,
        },
        required_capabilities: preview
            .required_capabilities
            .into_iter()
            .map(|item| PortableCapabilityDto {
                id: item.id,
                version: item.version,
            })
            .collect(),
        preserved_extensions: preview.preserved_extensions.into_iter().collect(),
        included_sections: preview.included_sections.into_iter().collect(),
        excluded_sections: preview.excluded_sections.into_iter().collect(),
        source_backup_selection: preview.source_backup_selection.map(source_backup_selection),
        omitted_assets: preview
            .omitted_assets
            .into_iter()
            .map(|(asset_id, placeholder)| OmittedAssetDto {
                asset_id,
                format: placeholder.format,
                version: placeholder.version,
                reason: match placeholder.reason {
                    OmittedAssetReason::ExcludeAll => "exclude-all",
                    OmittedAssetReason::AboveV1Threshold => "above-v1-threshold",
                    OmittedAssetReason::ImportExcluded => "import-excluded",
                },
                original_media_type: placeholder.original_media_type,
                original_size: placeholder.original_size,
                content_sha256: placeholder.content_sha256,
            })
            .collect(),

        scenarios: preview
            .scenarios
            .into_iter()
            .map(|item| PortableScenarioDto {
                scenario_id: item.scenario_id,
                title: item.title,
                collides: item.collides,
                source_revision: item.source_revision,
                same_identity_revision: item.same_identity_revision,
                same_identity_revision_warning: item.same_identity_revision_warning,
            })
            .collect(),
        supplemental_collisions: preview.supplemental_collisions,
        removed_scenarios: preview
            .removed_scenarios
            .into_iter()
            .map(|item| RemovedScenarioDto {
                scenario_id: item.scenario_id,
                title: item.title,
                revision: item.revision,
                archived: item.archived,
            })
            .collect(),
        removed_supplemental: preview.removed_supplemental,
        settings_changed: preview.settings_changed,
        settings_removed: preview.settings_removed,
        applied_migrations: preview
            .applied_migrations
            .into_iter()
            .map(|item| AppliedMigrationDto {
                registry: match item.registry {
                    MigrationRegistryKind::Outer => "outer",
                    MigrationRegistryKind::Portable => "portable",
                },
                name: item.name,
                from_version: item.from_version,
                to_version: item.to_version,
                version_space: item.version_space,
                subject: item.subject,
            })
            .collect(),
    }
}
fn fixed_exclusion_label(exclusion: FixedExclusion) -> &'static str {
    match exclusion {
        FixedExclusion::LocalUndoAndAuditHistory => "local-undo-and-audit-history",
        FixedExclusion::SqliteAndDatabaseInternals => "sqlite-and-database-internals",
        FixedExclusion::CredentialsTokensAndKeychainReferences => {
            "credentials-tokens-and-keychain-references"
        }
        FixedExclusion::DeviceLocalPathsAndWindowState => "device-local-paths-and-window-state",
        FixedExclusion::LogsCachesAndTemporaryData => "logs-caches-and-temporary-data",
        FixedExclusion::RedistributionProhibitedProviderData => {
            "redistribution-prohibited-provider-data"
        }
        FixedExclusion::ExecutableContent => "executable-content",
    }
}

fn backup_summary(summary: eutheto_core::BackupSummary) -> BackupSummaryDto {
    let (asset_selection, threshold_version, threshold_bytes) = match summary.asset_selection {
        BackupAssetSelection::ExcludeAll => ("exclude-all", None, None),
        BackupAssetSelection::IncludeAll => ("all", None, None),
        BackupAssetSelection::IncludeUnderThreshold => (
            "v1-threshold",
            Some(1),
            Some(eutheto_types::PORTABLE_LARGE_ASSET_BYTES_V1 as u64),
        ),
    };
    BackupSummaryDto {
        include_results: summary.include_results,
        asset_selection,
        excluded_asset_count: summary.excluded_asset_count,
        excluded_asset_ids: summary.excluded_asset_ids,
        exclusion_scope: summary.exclusion_scope,
        fixed_exclusions: summary
            .fixed_exclusions
            .into_iter()
            .map(fixed_exclusion_label)
            .collect(),
        threshold_version,
        threshold_bytes,
    }
}

async fn project_list_impl(
    state: &DesktopState,
    request: ProjectListRequest,
) -> ApiResult<Vec<ProjectSummaryDto>> {
    match state
        .app
        .query(AppQuery::ListProjects(request.scope.into()))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Projects(projects) => {
            Ok(response(request.request_id, None, Vec::new(), projects))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected project-list result.",
            None,
        )
        .into()),
    }
}

async fn core_unavailable(
    state: &DesktopState,
    capability: DeferredCapability,
) -> ApiResult<EmptyDto> {
    state
        .app
        .query(AppQuery::Deferred(capability))
        .await
        .map_err(map_app_error)?;
    Err(boundary_error(
        "protocol.deferred_result",
        "A deferred capability unexpectedly returned a result.",
        None,
    )
    .into())
}

#[tauri::command]
fn app_get_info(request: RequestOnly) -> ApiResponseDto<AppInfoDto> {
    response(
        request.request_id,
        None,
        Vec::new(),
        AppInfoDto {
            name: "eutheto",
            version: env!("CARGO_PKG_VERSION"),
            foundation: eutheto_core::foundation_status(),
            portable_extension: PORTABLE_EXTENSION,
            portable_extension_status: "provisional-development-only",
        },
    )
}

#[tauri::command]
fn app_get_capabilities(request: RequestOnly) -> ApiResponseDto<AppCapabilitiesDto> {
    const AVAILABLE: &[&str] = &[
        "app_get_info",
        "app_get_capabilities",
        "app_get_paths_summary",
        "app_create_support_bundle_preview",
        "pack_list",
        "pack_describe",
        "solver_list",
        "solver_describe",
        "solver_get_support_matrix",
        "solver_get_deferred_gates",
        "project_list",
        "project_get_metadata",
        "project_create",
        "project_duplicate",
        "project_archive",
        "project_unarchive",
        "project_delete",
        "project_import_preview",
        "project_import_apply",
        "project_export_preview",
        "project_export_create",
        "project_backup_preview",
        "project_backup_create",
        "project_restore_preview",
        "project_restore_apply",
        "project_operation_cancel",
        "project_unopened_bundle_inspect",
        "project_unopened_bundle_reexport",
        "operation_prepare",
        "operation_cancel",
        "operation_release",
        "workforce_apply_reviewed_generation",
        "scenario_get_summary",
        "scenario_get_setup_status",
        "scenario_get_view",
        "scenario_get_entity",
        "scenario_search_entities",
        "scenario_get_rule_catalog",
        "scenario_get_command_catalog",
        "scenario_apply_command",
        "scenario_apply_batch",
        "scenario_validate",
        "scenario_undo",
        "scenario_redo",
        "scenario_get_history",
        "solution_list",
        "solution_get_summary",
        "solution_get_view",
        "solution_select",
        "solution_verify",
        "solution_compare",
        "solution_explain",
        "solution_start_counterfactual",
        "solution_cancel_counterfactual",
        "settings_get",
        "settings_update",
        "settings_reset_section",
    ];
    let unavailable_commands = REGISTERED_COMMANDS
        .iter()
        .copied()
        .filter(|command| !AVAILABLE.contains(command))
        .collect();
    response(
        request.request_id,
        None,
        Vec::new(),
        AppCapabilitiesDto {
            available_commands: AVAILABLE.to_vec(),
            unavailable_commands,
        },
    )
}

#[tauri::command]
fn app_get_paths_summary(
    state: State<'_, DesktopState>,
    request: RequestOnly,
) -> ApiResponseDto<AppPathsSummaryDto> {
    response(request.request_id, None, Vec::new(), state.into())
}

#[tauri::command]
async fn app_create_support_bundle_preview(
    state: State<'_, DesktopState>,
    request: RequestOnly,
) -> ApiResult<SupportPreviewDto> {
    match state
        .app
        .query(AppQuery::SupportPreview)
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SupportPreview(preview) => {
            Ok(response(request.request_id, None, Vec::new(), preview))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected support preview result.",
            None,
        )
        .into()),
    }
}
#[tauri::command]
async fn pack_list(state: State<'_, DesktopState>, request: RequestOnly) -> ApiResult<Vec<Value>> {
    match state
        .app
        .query(AppQuery::ListDomainPacks)
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::DomainPacks(descriptors) => {
            let descriptors = descriptors
                .iter()
                .map(metadata_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(response(request.request_id, None, Vec::new(), descriptors))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected domain-pack list result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn pack_describe(
    state: State<'_, DesktopState>,
    request: PackDescribeRequest,
) -> ApiResult<DomainPackMetadataDto> {
    match state
        .app
        .query(AppQuery::DescribeDomainPack(request.pack_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::DomainPack(metadata) => {
            let metadata = metadata.as_ref();
            Ok(response(
                request.request_id,
                None,
                Vec::new(),
                DomainPackMetadataDto {
                    descriptor: metadata_value(&metadata.descriptor)?,
                    catalog: metadata_value(&metadata.catalog)?,
                },
            ))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected domain-pack description result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solver_list(
    state: State<'_, DesktopState>,
    request: RequestOnly,
) -> ApiResult<Vec<Value>> {
    match state
        .app
        .query(AppQuery::ListSolvers)
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Solvers(descriptors) => {
            let descriptors = descriptors
                .iter()
                .map(metadata_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(response(request.request_id, None, Vec::new(), descriptors))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solver list result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solver_describe(
    state: State<'_, DesktopState>,
    request: SolverDescribeRequest,
) -> ApiResult<Value> {
    match state
        .app
        .query(AppQuery::DescribeSolver(request.backend_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Solver(descriptor) => Ok(response(
            request.request_id,
            None,
            Vec::new(),
            metadata_value(&descriptor)?,
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solver description result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solver_get_support_matrix(
    state: State<'_, DesktopState>,
    request: RequestOnly,
) -> ApiResult<SolverSupportMatrixDto> {
    match state
        .app
        .query(AppQuery::SolverSupportMatrix)
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolverSupportMatrix(matrix) => Ok(response(
            request.request_id,
            None,
            Vec::new(),
            solver_support_matrix_dto(matrix),
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solver support-matrix result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solver_get_deferred_gates(
    state: State<'_, DesktopState>,
    request: RequestOnly,
) -> ApiResult<Vec<Value>> {
    match state
        .app
        .query(AppQuery::DeferredSolverGates)
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::DeferredSolverGates(gates) => {
            let gates = gates
                .iter()
                .map(metadata_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(response(request.request_id, None, Vec::new(), gates))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected deferred solver-gate result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_list(
    state: State<'_, DesktopState>,
    request: ProjectListRequest,
) -> ApiResult<Vec<ProjectSummaryDto>> {
    project_list_impl(&state, request).await
}

#[tauri::command]
async fn project_get_metadata(
    state: State<'_, DesktopState>,
    request: ScenarioRequest,
) -> ApiResult<ProjectMetadataDto> {
    match state
        .app
        .query(AppQuery::ProjectMetadata(request.scenario_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Project(project) => Ok(response(
            request.request_id,
            Some(project.revision),
            Vec::new(),
            project,
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected project result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_create(
    state: State<'_, DesktopState>,
    request: CreateProjectRequest,
) -> ApiResult<ProjectMetadataDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::CreateProject {
            request_id,
            title: request.title,
            description: request.description,
            domain_pack: request.domain_pack,
            settings: request.settings,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::Project(project) => Ok(response(
            request_id,
            Some(project.revision),
            Vec::new(),
            project,
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected project result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_duplicate(
    state: State<'_, DesktopState>,
    request: DuplicateProjectRequest,
) -> ApiResult<ProjectMetadataDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::DuplicateProject {
            request_id,
            source_id: request.source_id,
            expected_revision: request.expected_revision,
            title: request.title,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::Project(project) => Ok(response(
            request_id,
            Some(project.revision),
            Vec::new(),
            project,
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected duplicate-project result.",
            None,
        )
        .into()),
    }
}

async fn set_project_archived(
    state: &DesktopState,
    request: ProjectMutationRequest,
    archived: bool,
) -> ApiResult<EmptyDto> {
    let command = if archived {
        AppCommand::ArchiveProject {
            request_id: request.request_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
        }
    } else {
        AppCommand::UnarchiveProject {
            request_id: request.request_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
        }
    };
    let current_revision = request.expected_revision;
    match state.app.execute(command).await.map_err(map_app_error)? {
        AppCommandResult::Deleted => Ok(response(
            request.request_id,
            Some(current_revision),
            Vec::new(),
            EmptyDto {},
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected archive result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_archive(
    state: State<'_, DesktopState>,
    request: ProjectMutationRequest,
) -> ApiResult<EmptyDto> {
    set_project_archived(&state, request, true).await
}

#[tauri::command]
async fn project_unarchive(
    state: State<'_, DesktopState>,
    request: ProjectMutationRequest,
) -> ApiResult<EmptyDto> {
    set_project_archived(&state, request, false).await
}

#[tauri::command]
async fn project_delete(
    state: State<'_, DesktopState>,
    request: ProjectMutationRequest,
) -> ApiResult<EmptyDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::DeleteProject {
            request_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::Deleted => Ok(response(request_id, None, Vec::new(), EmptyDto {})),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected delete result.",
            None,
        )
        .into()),
    }
}

async fn preview_portable_bytes(
    state: &DesktopState,
    request: PortablePreviewRequest,
    bytes: Vec<u8>,
    restore: bool,
) -> ApiResult<PortablePreviewDto> {
    let query = if restore {
        AppQuery::PreviewRestore {
            bytes,
            options: request.options,
        }
    } else {
        AppQuery::PreviewImport {
            bytes,
            options: request.options,
        }
    };
    match state.app.query(query).await.map_err(map_app_error)? {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => Ok(response(
            request.request_id,
            None,
            Vec::new(),
            portable_preview(preview_id, *preview),
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected portable preview result.",
            None,
        )
        .into()),
    }
}

async fn preview_portable(
    state: &DesktopState,
    app_handle: AppHandle,
    request: PortablePreviewRequest,
    restore: bool,
) -> ApiResult<PortablePreviewDto> {
    let dialog_title = if restore {
        "Choose an Eutheto backup to restore"
    } else {
        "Choose an Eutheto file to import"
    };
    let bytes = native_file_task(move || {
        let selected = app_handle
            .dialog()
            .file()
            .set_title(dialog_title)
            .add_filter("Eutheto portable file", &["eutheto"])
            .blocking_pick_file()
            .ok_or(NativeFileError::Cancelled)?;
        let path = selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)?;
        read_bounded_portable(&path)
    })
    .await?;
    preview_portable_bytes(state, request, bytes, restore).await
}

#[tauri::command]
async fn project_import_preview(
    state: State<'_, DesktopState>,
    app_handle: AppHandle,
    request: PortablePreviewRequest,
) -> ApiResult<PortablePreviewDto> {
    preview_portable(&state, app_handle, request, false).await
}

#[tauri::command]
async fn project_import_apply(
    state: State<'_, DesktopState>,
    request: PortableApplyRequest,
) -> ApiResult<PortableAppliedDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::ApplyImport {
            request_id,
            preview_id: request.preview_id,
            collision_plan: request.collision_plan,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::PortableApplied { scenarios } => Ok(response(
            request_id,
            None,
            Vec::new(),
            PortableAppliedDto {
                scenario_ids: scenarios
                    .into_iter()
                    .map(|scenario| scenario.scenario_id)
                    .collect(),
            },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected import result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_export_preview(
    state: State<'_, DesktopState>,
    request: ScenarioRequest,
) -> ApiResult<PortableFilePreviewDto> {
    let title = match state
        .app
        .query(AppQuery::ProjectMetadata(request.scenario_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Project(project) => project.title,
        _ => return Err(prepared_output_error("protocol.result_mismatch").into()),
    };
    let AppQueryResult::Bundle {
        bytes,
        scenario_revision,
        library_revision,
    } = state
        .app
        .query(AppQuery::ExportScenario(request.scenario_id))
        .await
        .map_err(map_app_error)?
    else {
        return Err(prepared_output_error("protocol.result_mismatch").into());
    };
    let preview_id = new_prepared_preview_id()?;
    let digest = eutheto_export::sha256_hex(&bytes);
    let byte_length = bytes.len();
    state.prepared_outputs.lock().await.insert(
        preview_id,
        PreparedPortableOutput {
            bytes,
            sha256: digest.clone(),
            kind: PreparedPortableKind::Scenario {
                scenario_id: request.scenario_id,
                revision: scenario_revision,
                library_revision,
                title,
            },
        },
    )?;
    Ok(response(
        request.request_id,
        Some(scenario_revision),
        Vec::new(),
        PortableFilePreviewDto {
            title: "Scenario export".to_owned(),
            byte_length,
            backup_summary: None,
            preview_id,
            digest,
            current_revision: Some(scenario_revision),
            library_revision: Some(library_revision),
        },
    ))
}

#[tauri::command]
async fn project_export_create(
    state: State<'_, DesktopState>,
    app_handle: AppHandle,
    request: ExportCreateRequest,
) -> ApiResult<PortableArtifactDto> {
    let title = {
        let cache = state.prepared_outputs.lock().await;
        match cache.get(request.preview_id).map(|output| &output.kind) {
            Some(PreparedPortableKind::Scenario {
                scenario_id, title, ..
            }) if scenario_id == &request.scenario_id => title.clone(),
            Some(_) => {
                return Err(prepared_output_error("portable.preview_kind_mismatch").into());
            }
            None => return Err(prepared_output_error("portable.preview_not_found").into()),
        }
    };
    let suggested_name = suggested_portable_filename(&title, "Eutheto-Export");
    let selected = native_file_task(move || {
        let selected = app_handle
            .dialog()
            .file()
            .set_title("Save Eutheto export")
            .set_file_name(suggested_name)
            .add_filter("Eutheto portable file", &["eutheto"])
            .blocking_save_file()
            .ok_or(NativeFileError::Cancelled)?;
        let destination = selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)?;
        let artifact_name = selected_basename(&destination)?;
        Ok((destination, artifact_name))
    })
    .await;
    let (destination, artifact_name) = match selected {
        Ok(selected) => selected,
        Err(error) => {
            state
                .prepared_outputs
                .lock()
                .await
                .remove(request.preview_id);
            return Err(error);
        }
    };
    let output = state
        .prepared_outputs
        .lock()
        .await
        .remove(request.preview_id)
        .ok_or_else(|| ApiError::from(prepared_output_error("portable.preview_not_found")))?;
    let PreparedPortableKind::Scenario {
        scenario_id,
        revision,
        library_revision,
        ..
    } = output.kind
    else {
        return Err(prepared_output_error("portable.preview_kind_mismatch").into());
    };
    match state
        .app
        .execute(AppCommand::PublishPreparedPortable {
            destination,
            bytes: output.bytes,
            expected_sha256: output.sha256,
            binding: PreparedPortableBinding::Scenario {
                scenario_id,
                expected_revision: revision,
                expected_library_revision: library_revision,
            },
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::BundleWritten => Ok(response(
            request.request_id,
            Some(revision),
            Vec::new(),
            PortableArtifactDto { artifact_name },
        )),
        _ => Err(prepared_output_error("protocol.result_mismatch").into()),
    }
}

#[tauri::command]
async fn project_backup_preview(
    state: State<'_, DesktopState>,
    request: BackupPreviewRequest,
) -> ApiResult<PortableFilePreviewDto> {
    let title = request.title;
    let AppQueryResult::BackupBundle {
        bytes,
        summary,
        library_revision,
    } = state
        .app
        .query(AppQuery::ExportBackup {
            title: title.clone(),
            selection: BackupSelection::default(),
        })
        .await
        .map_err(map_app_error)?
    else {
        return Err(prepared_output_error("protocol.result_mismatch").into());
    };
    let preview_id = new_prepared_preview_id()?;
    let digest = eutheto_export::sha256_hex(&bytes);
    let byte_length = bytes.len();
    let summary = backup_summary(summary);
    state.prepared_outputs.lock().await.insert(
        preview_id,
        PreparedPortableOutput {
            bytes,
            sha256: digest.clone(),
            kind: PreparedPortableKind::Backup {
                library_revision,
                title: title.clone(),
                summary: summary.clone(),
            },
        },
    )?;
    Ok(response(
        request.request_id,
        Some(library_revision),
        Vec::new(),
        PortableFilePreviewDto {
            title,
            byte_length,
            backup_summary: Some(summary),
            preview_id,
            digest,
            current_revision: None,
            library_revision: Some(library_revision),
        },
    ))
}

#[tauri::command]
async fn project_backup_create(
    state: State<'_, DesktopState>,
    app_handle: AppHandle,
    request: BackupCreateRequest,
) -> ApiResult<PortableArtifactDto> {
    {
        let cache = state.prepared_outputs.lock().await;
        match cache.get(request.preview_id).map(|output| &output.kind) {
            Some(PreparedPortableKind::Backup { title, summary, .. })
                if title == &request.title
                    && summary.include_results
                    && summary.asset_selection == "all" => {}
            Some(_) => {
                return Err(prepared_output_error("portable.preview_kind_mismatch").into());
            }
            None => return Err(prepared_output_error("portable.preview_not_found").into()),
        }
    }
    let suggested_name = suggested_portable_filename(&request.title, "Eutheto-Backup");
    let selected = native_file_task(move || {
        let selected = app_handle
            .dialog()
            .file()
            .set_title("Save Eutheto backup")
            .set_file_name(suggested_name)
            .add_filter("Eutheto portable file", &["eutheto"])
            .blocking_save_file()
            .ok_or(NativeFileError::Cancelled)?;
        let destination = selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)?;
        let artifact_name = selected_basename(&destination)?;
        Ok((destination, artifact_name))
    })
    .await;
    let (destination, artifact_name) = match selected {
        Ok(selected) => selected,
        Err(error) => {
            state
                .prepared_outputs
                .lock()
                .await
                .remove(request.preview_id);
            return Err(error);
        }
    };
    let output = state
        .prepared_outputs
        .lock()
        .await
        .remove(request.preview_id)
        .ok_or_else(|| ApiError::from(prepared_output_error("portable.preview_not_found")))?;
    let PreparedPortableKind::Backup {
        library_revision, ..
    } = output.kind
    else {
        return Err(prepared_output_error("portable.preview_kind_mismatch").into());
    };
    match state
        .app
        .execute(AppCommand::PublishPreparedPortable {
            destination,
            bytes: output.bytes,
            expected_sha256: output.sha256,
            binding: PreparedPortableBinding::Backup {
                expected_library_revision: library_revision,
            },
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::BundleWritten => Ok(response(
            request.request_id,
            Some(library_revision),
            Vec::new(),
            PortableArtifactDto { artifact_name },
        )),
        _ => Err(prepared_output_error("protocol.result_mismatch").into()),
    }
}

#[tauri::command]
async fn project_restore_preview(
    state: State<'_, DesktopState>,
    app_handle: AppHandle,
    request: PortablePreviewRequest,
) -> ApiResult<PortablePreviewDto> {
    preview_portable(&state, app_handle, request, true).await
}

#[tauri::command]
async fn project_restore_apply(
    state: State<'_, DesktopState>,
    request: RestoreApplyRequest,
) -> ApiResult<PortableAppliedDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::ApplyRestore {
            request_id,
            preview_id: request.preview_id,
            collision_plan: request.collision_plan,
            authorization: request.authorization.into(),
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::PortableApplied { scenarios } => Ok(response(
            request_id,
            None,
            Vec::new(),
            PortableAppliedDto {
                scenario_ids: scenarios
                    .into_iter()
                    .map(|scenario| scenario.scenario_id)
                    .collect(),
            },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected restore result.",
            None,
        )
        .into()),
    }
}

async fn cancel_portable_preview_impl(
    state: &DesktopState,
    request: PortableCancelRequest,
) -> ApiResult<EmptyDto> {
    if state
        .prepared_outputs
        .lock()
        .await
        .remove(request.preview_id)
        .is_some()
    {
        return Ok(response(request.request_id, None, Vec::new(), EmptyDto {}));
    }
    match state
        .app
        .execute(AppCommand::CancelPortablePreview {
            preview_id: request.preview_id,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::PortablePreviewCancelled => {
            Ok(response(request.request_id, None, Vec::new(), EmptyDto {}))
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected cancellation result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_operation_cancel(
    state: State<'_, DesktopState>,
    request: PortableCancelRequest,
) -> ApiResult<EmptyDto> {
    cancel_portable_preview_impl(&state, request).await
}

async fn inspect_unopened_bundle_bytes(
    state: &DesktopState,
    request_id: RequestId,
    bytes: Vec<u8>,
) -> ApiResult<UnopenedBundlePreviewDto> {
    match state
        .app
        .query(AppQuery::InspectUnopenedBundle { bytes })
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::UnopenedBundlePreview {
            preview_id,
            metadata,
        } => Ok(response(
            request_id,
            None,
            Vec::new(),
            UnopenedBundlePreviewDto {
                preview_id,
                metadata: UnopenedBundleMetadataDto {
                    file_sha256: metadata.file_sha256,
                    format: metadata.format,
                    format_version: metadata.format_version,
                    portable_schema_version: metadata.portable_schema_version,
                    bundle_kind: metadata.bundle_kind,
                    title: metadata.title,
                    required_capabilities: metadata
                        .required_capabilities
                        .into_iter()
                        .map(|capability| PortableCapabilityDto {
                            id: capability.id,
                            version: capability.version,
                        })
                        .collect(),
                    scenarios: metadata
                        .scenarios
                        .into_iter()
                        .map(|scenario| UnopenedBundleScenarioDto {
                            path: scenario.path,
                            scenario_id: scenario.scenario_id,
                            pack_id: scenario.pack_id,
                            internal_pack_schema_version: scenario.internal_pack_schema_version,
                            portable_pack_schema_version: scenario.portable_pack_schema_version,
                        })
                        .collect(),
                },
            },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected unopened-bundle inspection result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_unopened_bundle_inspect(
    state: State<'_, DesktopState>,
    app_handle: AppHandle,
    request: RequestOnly,
) -> ApiResult<UnopenedBundlePreviewDto> {
    let bytes = native_file_task(move || {
        let selected = app_handle
            .dialog()
            .file()
            .set_title("Choose an unopened Eutheto bundle to inspect")
            .add_filter("Eutheto portable file", &["eutheto"])
            .blocking_pick_file()
            .ok_or(NativeFileError::Cancelled)?;
        let path = selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)?;
        read_bounded_portable(&path)
    })
    .await?;
    inspect_unopened_bundle_bytes(&state, request.request_id, bytes).await
}

async fn exact_reexport_unopened_bundle_to_path(
    state: &DesktopState,
    request: PortableCancelRequest,
    destination: PathBuf,
    artifact_name: String,
) -> ApiResult<PortableArtifactDto> {
    match state
        .app
        .execute(AppCommand::ExactReexportUnopenedBundle {
            preview_id: request.preview_id,
            destination,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::UnopenedBundleReexported => Ok(response(
            request.request_id,
            None,
            Vec::new(),
            PortableArtifactDto { artifact_name },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected unopened-bundle re-export result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn project_unopened_bundle_reexport(
    state: State<'_, DesktopState>,
    app_handle: AppHandle,
    request: PortableCancelRequest,
) -> ApiResult<PortableArtifactDto> {
    let selected = native_file_task(move || {
        let selected = app_handle
            .dialog()
            .file()
            .set_title("Save exact unopened Eutheto bundle")
            .set_file_name("Eutheto-Unopened.eutheto")
            .add_filter("Eutheto portable file", &["eutheto"])
            .blocking_save_file()
            .ok_or(NativeFileError::Cancelled)?;
        let destination = selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)?;
        let artifact_name = selected_basename(&destination)?;
        Ok((destination, artifact_name))
    })
    .await;
    let (destination, artifact_name) = match selected {
        Ok(selected) => selected,
        Err(error) => {
            let _ = state
                .app
                .execute(AppCommand::CancelPortablePreview {
                    preview_id: request.preview_id,
                })
                .await;
            return Err(error);
        }
    };
    exact_reexport_unopened_bundle_to_path(&state, request, destination, artifact_name).await
}

#[tauri::command]
fn scenario_get_command_catalog(request: RequestOnly) -> ApiResponseDto<ScenarioCommandCatalogDto> {
    response(
        request.request_id,
        None,
        Vec::new(),
        ScenarioCommandCatalogDto {
            command_types: &[
                "addEntity",
                "updateEntity",
                "removeEntity",
                "addRule",
                "updateRule",
                "removeRule",
                "setPreference",
                "lockAssignment",
                "unlockAssignment",
                "setScenarioSettings",
                "applyDomainCommand",
                "applyBatch",
            ],
        },
    )
}

async fn execute_scenario(
    state: &DesktopState,
    request_id: RequestId,
    envelope: CommandEnvelope,
    truncate_redo: bool,
) -> ApiResult<CommandResult> {
    match state
        .app
        .execute(AppCommand::ApplyScenario {
            request_id,
            envelope,
            truncate_redo,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::ScenarioCommand(result) => Ok(response(
            request_id,
            Some(result.new_revision),
            Vec::new(),
            result,
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected scenario mutation result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn scenario_apply_command(
    state: State<'_, DesktopState>,
    request: ApplyScenarioCommandRequest,
) -> ApiResult<CommandResult> {
    let request_id = request.request_id;
    let envelope = CommandEnvelope {
        command_id: request.command_id,
        scenario_id: request.scenario_id,
        expected_revision: request.expected_revision,
        actor: request.actor,
        source: CommandSource::Desktop,
        command: request.command,
    };
    execute_scenario(&state, request_id, envelope, request.truncate_redo).await
}

#[tauri::command]
async fn scenario_apply_batch(
    state: State<'_, DesktopState>,
    request: ApplyScenarioBatchRequest,
) -> ApiResult<CommandResult> {
    let request_id = request.request_id;
    let envelope = CommandEnvelope {
        command_id: request.command_id,
        scenario_id: request.scenario_id,
        expected_revision: request.expected_revision,
        actor: request.actor,
        source: CommandSource::Desktop,
        command: ScenarioCommand::ApplyBatch(CommandBatch {
            label: request.label,
            commands: request.commands,
        }),
    };
    execute_scenario(&state, request_id, envelope, request.truncate_redo).await
}

async fn move_history(
    state: &DesktopState,
    request: HistoryMutationRequest,
    undo: bool,
) -> ApiResult<CommandResult> {
    let command = if undo {
        AppCommand::Undo {
            request_id: request.request_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
        }
    } else {
        AppCommand::Redo {
            request_id: request.request_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
        }
    };
    match state.app.execute(command).await.map_err(map_app_error)? {
        AppCommandResult::ScenarioCommand(result) => Ok(response(
            request.request_id,
            Some(result.new_revision),
            Vec::new(),
            result,
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected history mutation result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn scenario_undo(
    state: State<'_, DesktopState>,
    request: HistoryMutationRequest,
) -> ApiResult<CommandResult> {
    move_history(&state, request, true).await
}

#[tauri::command]
async fn scenario_redo(
    state: State<'_, DesktopState>,
    request: HistoryMutationRequest,
) -> ApiResult<CommandResult> {
    move_history(&state, request, false).await
}

#[tauri::command]
async fn scenario_get_history(
    state: State<'_, DesktopState>,
    request: ScenarioRequest,
) -> ApiResult<HistoryDto> {
    match state
        .app
        .query(AppQuery::History(request.scenario_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::History(entries) => Ok(response(
            request.request_id,
            None,
            Vec::new(),
            HistoryDto {
                entries: entries
                    .into_iter()
                    .map(|entry| HistoryEntryDto {
                        id: entry.id,
                        revision_before: entry.revision_before,
                        revision_after: entry.revision_after,
                        command_type: entry.command_type,
                        command: entry.command,
                        inverse: entry.inverse,
                        actor: entry.actor,
                        source: entry.source,
                        summary: entry.summary,
                        created_at: entry.created_at,
                        history_sequence: entry.history_sequence,
                        branch_generation: entry.branch_generation,
                        applied: entry.applied,
                    })
                    .collect(),
            },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected history result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn settings_get(
    state: State<'_, DesktopState>,
    request: SettingRequest,
) -> ApiResult<SettingValueDto> {
    match state
        .app
        .query(AppQuery::Setting(request.key))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Setting(setting) => Ok(response(
            request.request_id,
            None,
            Vec::new(),
            SettingValueDto {
                setting: setting.map(|setting| SettingEntryDto {
                    value: setting.value,
                    updated_at: setting.updated_at,
                }),
            },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected setting result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn settings_update(
    state: State<'_, DesktopState>,
    request: SettingUpdateRequest,
) -> ApiResult<EmptyDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::SetSetting {
            request_id,
            key: request.key,
            value: request.value,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::SettingUpdated => Ok(response(request_id, None, Vec::new(), EmptyDto {})),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected setting update result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn settings_reset_section(
    state: State<'_, DesktopState>,
    request: SettingRequest,
) -> ApiResult<SettingResetDto> {
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::DeleteSetting {
            request_id,
            key: request.key,
        })
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::SettingDeleted(existed) => Ok(response(
            request_id,
            None,
            Vec::new(),
            SettingResetDto { existed },
        )),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected setting reset result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_list(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionListRequestV1> = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionList(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionList(result) => {
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-list result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_get_summary(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionSummaryRequestV1> = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionGetSummary(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionSummary(result) => {
            let result = *result;
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-summary result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_get_view<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionViewRequestV1> = decode_solution_request(request)?;
    let request_id = request.request_id;
    let context = OperationContextV1::Scenario {
        scenario_id: request.operation.scenario_id,
        expected_revision: None,
    };
    let operation_id = state
        .operations
        .reserve_accepted(window.label(), context.clone())?
        .operation_id;
    let app = state.app.clone();
    state
        .operations
        .run(
            window.label(),
            OperationClaim {
                operation_id,
                request_id,
                purpose: None,
                context,
            },
            None,
            OperationPhaseV1::BuildingView,
            ((), None),
            move |(), mut execution| async move {
                let result = app
                    .solution_view_with_cancellation(request.operation, execution.cancellation())
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                execution.preparing_response();
                tauri::async_runtime::spawn_blocking(move || {
                    solution_response(request_id, result.current_revision, result)
                })
                .await
                .map_err(|_| {
                    Box::new(boundary_error(
                        "operation.execution_failed",
                        "The native operation could not finish safely.",
                        None,
                    ))
                })?
            },
        )
        .await
}

#[tauri::command]
async fn solution_select(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolutionSelectRequestV1 = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::SolutionSelect(request))
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::SolutionSelected(result) => {
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-selection result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_verify(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionVerifyRequestV1> = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionVerify(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionVerification(result) => {
            let result = *result;
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-verification result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_compare(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionCompareRequestV1> = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionCompare(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionComparison(result) => {
            let result = *result;
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-comparison result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_explain(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionExplainRequestV1> = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionExplain(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionExplanation(result) => {
            let result = *result;
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-explanation result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_start_counterfactual(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request = decode_counterfactual_start_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::SolutionStartCounterfactual(request))
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::CounterfactualStarted(result) => {
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected counterfactual-start result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_cancel_counterfactual(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolutionCancelCounterfactualRequestV1 = decode_solution_request(request)?;
    let request_id = request.cancel_request_id;
    match state
        .app
        .execute(AppCommand::SolutionCancelCounterfactual(request))
        .await
        .map_err(map_app_error)?
    {
        AppCommandResult::CounterfactualCancelled(result) => {
            let current_revision = result.current_revision;
            solution_response(request_id, current_revision, result)
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected counterfactual-cancellation result.",
            None,
        )
        .into()),
    }
}

macro_rules! unsupported_commands {
    ($($name:ident => ($code:literal, $capability:literal)),+ $(,)?) => {
        $(
            #[tauri::command]
            fn $name(request: RequestOnly) -> ApiResult<EmptyDto> {
                let _ = request;
                Err(unavailable_error($code, $capability).into())
            }
        )+
    };
}

macro_rules! core_deferred_commands {
    ($capability:expr; $($name:ident),+ $(,)?) => {
        $(
            #[tauri::command]
            async fn $name(
                state: State<'_, DesktopState>,
                request: RequestOnly,
            ) -> ApiResult<EmptyDto> {
                let _ = request;
                core_unavailable(&state, $capability).await
            }
        )+
    };
}

unsupported_commands!(
    app_open_data_folder => ("capability.data_folder_unavailable", "Opening the data folder"),
    app_create_support_bundle => ("capability.support_bundle_unavailable", "Support bundles"),
    app_check_for_update => ("capability.update_unavailable", "Application updates"),
    app_install_update => ("capability.update_unavailable", "Application updates"),
    app_get_license_inventory => ("capability.license_inventory_unavailable", "License inventory"),
    scenario_migrate_preview => ("capability.scenario_migration_unavailable", "Scenario migration previews"),
    settings_export_nonsecret => ("capability.settings_portable_unavailable", "Portable settings export"),
    settings_import_nonsecret => ("capability.settings_portable_unavailable", "Portable settings import"),
);

core_deferred_commands!(
    DeferredCapability::Solve;
    solve_get_backend_options,
    solve_estimate_model,
    solve_start,
    solve_cancel,
    solve_get_job,
    solve_list_runs,
    solve_get_diagnostics_summary,
);

core_deferred_commands!(
    DeferredCapability::Solution;
    solution_lock_assignment,
    solution_unlock_assignment,
    solution_create_repair_request,
    solution_export_preview,
    solution_export,
    solution_share_preview,
    solution_share_create,
    solution_export_cancel,
);

core_deferred_commands!(
    DeferredCapability::ArtificialIntelligence;
    ai_get_provider_catalog,
    ai_get_configuration,
    ai_store_credential,
    ai_delete_credential,
    ai_test_provider,
    ai_list_models,
    ai_list_conversations,
    ai_create_conversation,
    ai_get_conversation,
    ai_send_turn,
    ai_cancel_turn,
    ai_get_proposal,
    ai_apply_proposal,
    ai_reject_proposal,
    ai_delete_conversation,
);

const FORWARDED_EVENTS: &[(EventTopic, &str)] = &[
    (EventTopic::SolveProgress, "solve://progress"),
    (EventTopic::SolveCompleted, "solve://completed"),
    (EventTopic::ScenarioChanged, "scenario://changed"),
    (
        EventTopic::ScenarioValidationChanged,
        "scenario://validation-changed",
    ),
    (
        EventTopic::CounterfactualProgress,
        "counterfactual://progress",
    ),
    (EventTopic::AppNotification, "app://notification"),
];

fn spawn_event_forwarder(
    handle: AppHandle,
    app: EuthetoApp,
    topic: EventTopic,
    event_name: &'static str,
) {
    drop(tauri::async_runtime::spawn(async move {
        let Ok(mut subscription) = app.subscribe(topic).await else {
            return;
        };
        loop {
            match subscription.recv().await {
                Ok(event) => {
                    if handle.emit(event_name, &event.payload).is_err() {
                        break;
                    }
                }
                Err(AppError::Protocol(failure)) if failure.retryable => {
                    if handle
                        .emit(
                            "library://refresh-required",
                            LibraryRefreshRequiredDto {
                                reason: "event-subscription-lagged",
                            },
                        )
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }));
}

/// Runs the Tauri desktop application with one authoritative application service.
///
/// # Errors
///
/// Returns an error if application directories cannot be resolved or created, the
/// application service cannot be opened, or the Tauri runtime cannot start.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    let desktop = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|handle| {
            let app_data_dir = handle.path().app_data_dir()?;
            let cache_dir = app_data_dir.join("cache");
            let backup_dir = app_data_dir.join("backups");
            let dependencies = AppDependencies {
                paths: AppPaths {
                    database: app_data_dir.join("library.sqlite3"),
                    safety_backups: backup_dir.clone(),
                },
                clock: Arc::new(SystemClock),
                monotonic_clock: Arc::new(eutheto_types::SystemMonotonicClock::new()),
                ids: Arc::new(SystemIdGenerator),
                cancellation: CancellationToken::default(),
            };
            #[cfg(feature = "bundled-ortools")]
            let open_result = {
                let artifact =
                    tauri::async_runtime::block_on(bundled_solver::load(handle.handle()))?;
                let solvers =
                    eutheto_solver_ortools::registry_with_ortools(artifact).map_err(|_| {
                        std::io::Error::other("bundled OR-Tools registry metadata is invalid")
                    })?;
                tauri::async_runtime::block_on(EuthetoApp::open_with_solver_registry(
                    dependencies,
                    solvers,
                ))
            };
            #[cfg(not(feature = "bundled-ortools"))]
            let open_result = tauri::async_runtime::block_on(EuthetoApp::open(dependencies));
            let app = open_result.map_err(|error| {
                std::io::Error::other(format!("application startup failed: {error:?}"))
            })?;
            std::fs::create_dir_all(&cache_dir)?;
            for &(topic, event_name) in FORWARDED_EVENTS {
                spawn_event_forwarder(handle.handle().clone(), app.clone(), topic, event_name);
            }
            handle.manage(DesktopState::new(app, cache_dir, backup_dir));
            Ok(())
        })
        .invoke_handler(with_tauri_commands!(make_tauri_handler))
        .build(tauri::generate_context!())?;
    desktop.run(|handle, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } => {
            handle
                .state::<DesktopState>()
                .operations
                .cancel_window(&label);
        }
        tauri::RunEvent::Exit => handle.state::<DesktopState>().operations.shutdown(),
        _ => {}
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use eutheto_export::FixedExclusion;
    use eutheto_import::{CollisionAction, SafetyBackupEvidence, SupplementalCollisionAction};
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::sync::Arc;

    use eutheto_core::{
        AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
        BackendSupportColumn, BackupAssetSelection, CapabilityMatrix, DeferredCapability,
        EuthetoApp, ScenarioSummaryV2, SolverSupportMatrixMetadata, SupportCell, SupportFeature,
        SupportFeatureCategory, SupportFeatureGate, SupportFeatureId,
    };
    use eutheto_types::{
        ActorRef, AddEntity, ApiErrorCategoryDto, ApiErrorDto, ApiResponseDto, AppError, BackendId,
        CancellationToken, CommandEnvelope, CommandId, CommandResult, CommandSource, DomainPackRef,
        EntityId, EventTopic, ProjectMetadataDto, ProjectSummaryDto, REVISION_MAX_V1, RequestId,
        Revision, SafeDiagnosticValue, ScenarioCommand, ScenarioSettings, ScenarioViewDto,
        SystemClock, SystemIdGenerator,
    };
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use tauri::Manager;

    use super::{
        BackupCreateRequest, BackupSummaryDto, CorrelatedRequest, DesktopState,
        ExportCreateRequest, FORWARDED_EVENTS, NativeFileError, PortableCancelRequest,
        PortablePreviewRequest, PreparedPortableCache, PreparedPortableKind,
        PreparedPortableOutput, ProjectListRequest, ProjectScopeDto, RequestOnly,
        SolutionExplainRequestV1, SolutionStartCounterfactualRequestV1, app_get_capabilities,
        backup_summary, cancel_portable_preview_impl, core_unavailable,
        decode_counterfactual_start_request, exact_reexport_unopened_bundle_to_path,
        inspect_unopened_bundle_bytes, map_app_error, map_native_file_error, native_file_task,
        normalize_counterfactual_int64, operation_prepare, pack_describe, pack_list,
        preview_portable_bytes, project_archive, project_create, project_delete,
        project_import_apply, project_list, project_list_impl, project_restore_apply,
        project_unarchive, read_bounded_portable, revision_diagnostic_value,
        scenario_apply_command, scenario_get_summary, scenario_redo, scenario_undo,
        selected_basename, solution_explain, solution_list, solution_response, solver_describe,
        solver_get_deferred_gates, solver_get_support_matrix, solver_list,
        solver_support_matrix_dto, suggested_portable_filename,
    };

    type IpcResult = Result<tauri::ipc::InvokeResponseBody, Value>;

    fn invoke_ipc(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        command: &str,
        request: &Value,
    ) -> Result<IpcResult, Box<dyn Error>> {
        invoke_ipc_args(webview, command, json!({ "request": request }))
    }

    pub(super) fn invoke_ipc_args(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        command: &str,
        arguments: Value,
    ) -> Result<IpcResult, Box<dyn Error>> {
        let origin = if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        Ok(tauri::test::get_ipc_response(
            webview,
            tauri::webview::InvokeRequest {
                cmd: command.to_owned(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: origin.parse()?,
                body: arguments.into(),
                headers: tauri::http::HeaderMap::default(),

                invoke_key: tauri::test::INVOKE_KEY.to_owned(),
            },
        ))
    }

    pub(super) fn invoke_ok<T: DeserializeOwned>(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        command: &str,
        request: &Value,
    ) -> Result<T, Box<dyn Error>> {
        match invoke_ipc(webview, command, request)? {
            Ok(body) => Ok(body.deserialize()?),
            Err(error) => Err(format!("{command} IPC failed: {error}").into()),
        }
    }

    // Native reads expose bounded summaries. Exact persistence assertions inspect Rust state,
    // not a reconstructed raw-document IPC response or a replacement desktop authority.
    async fn native_summary_and_core_snapshot(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        request_id: RequestId,
        scenario_id: eutheto_types::ScenarioId,
    ) -> Result<(ApiResponseDto<ScenarioSummaryV2>, ScenarioViewDto), Box<dyn Error>> {
        let prepared: ApiResponseDto<Value> = invoke_ok(
            webview,
            "operation_prepare",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "purpose": {"kind": "scenarioSummary"},
                "context": {"kind": "scenario", "scenarioId": scenario_id, "expectedRevision": null}
            }),
        )?;
        let summary: ApiResponseDto<ScenarioSummaryV2> = match invoke_ipc_args(
            webview,
            "scenario_get_summary",
            json!({
                "onProgress": "__CHANNEL__:42",
                "request": {
                    "requestId": request_id, "schemaVersion": 2, "scenarioId": scenario_id,
                    "expectedRevision": null, "operationId": prepared.result["operationId"]
                }
            }),
        )? {
            Ok(body) => body.deserialize()?,
            Err(error) => return Err(format!("summary IPC failed: {error}").into()),
        };
        let snapshot = match webview
            .state::<DesktopState>()
            .app
            .query(AppQuery::ScenarioView(scenario_id))
            .await
            .map_err(|error| format!("core persistence snapshot failed: {error:?}"))?
        {
            AppQueryResult::Scenario(snapshot) => *snapshot,
            _ => return Err("unexpected core persistence snapshot".into()),
        };
        assert_eq!(summary.current_revision, Some(snapshot.revision));
        Ok((summary, snapshot))
    }

    fn portable_options() -> Value {
        json!({
            "restoreMode": "import-scenario",
            "includeResults": true,
            "includeAssets": true
        })
    }

    fn all_fixed_exclusions() -> Vec<&'static str> {
        vec![
            "local-undo-and-audit-history",
            "sqlite-and-database-internals",
            "credentials-tokens-and-keychain-references",
            "device-local-paths-and-window-state",
            "logs-caches-and-temporary-data",
            "redistribution-prohibited-provider-data",
            "executable-content",
        ]
    }

    #[test]
    fn conflict_error_includes_current_revision() -> Result<(), Box<dyn Error>> {
        let error = map_app_error(AppError::Conflict {
            expected_revision: Revision::new(2),
            actual_revision: Revision::new(3),
        });
        assert_eq!(error.category, ApiErrorCategoryDto::Conflict);
        assert_eq!(error.code, "scenario.revision_conflict");
        let details = error.details.ok_or("missing conflict details")?;
        assert!(details.contains_key("expectedRevision"));
        assert!(details.contains_key("currentRevision"));
        assert_eq!(
            details.get("expectedRevision"),
            Some(&SafeDiagnosticValue::Integer(2))
        );
        assert_eq!(
            details.get("currentRevision"),
            Some(&SafeDiagnosticValue::Integer(3))
        );
        Ok(())
    }

    #[test]
    fn large_revision_diagnostic_uses_decimal_text() {
        assert_eq!(
            revision_diagnostic_value(u64::MAX),
            SafeDiagnosticValue::Text(u64::MAX.to_string())
        );
    }

    #[test]
    fn prepared_output_cache_preserves_exact_bytes_and_evicts_oldest() -> Result<(), Box<dyn Error>>
    {
        let ids = SystemIdGenerator;
        let mut cache = PreparedPortableCache::default();
        let summary = BackupSummaryDto {
            include_results: true,
            asset_selection: "all",
            excluded_asset_count: 1,
            excluded_asset_ids: vec!["inherited-placeholder.png".to_owned()],
            exclusion_scope: Some("inherited-placeholder".to_owned()),
            threshold_version: None,
            threshold_bytes: None,
            fixed_exclusions: all_fixed_exclusions(),
        };
        let mut preview_ids = Vec::new();
        for marker in 0_u8..4 {
            let preview_id = RequestId::new(&ids)?;
            preview_ids.push(preview_id);
            let inserted = cache.insert(
                preview_id,
                PreparedPortableOutput {
                    bytes: vec![marker, marker.saturating_add(1)],
                    sha256: format!("{marker:064x}"),
                    kind: PreparedPortableKind::Backup {
                        library_revision: Revision::new(u64::from(marker)),
                        title: "Prepared backup".to_owned(),
                        summary: summary.clone(),
                    },
                },
            );
            assert!(inserted.is_ok());
        }
        assert!(cache.get(preview_ids[0]).is_none());
        let output = cache
            .remove(preview_ids[3])
            .ok_or("newest prepared output was missing")?;
        assert_eq!(output.bytes, vec![3, 4]);
        assert_eq!(output.sha256.len(), 64);
        assert!(output.sha256.ends_with('3'));
        assert!(cache.get(preview_ids[3]).is_none());
        Ok(())
    }

    #[test]
    fn revision_request_dto_preserves_cap_and_rejects_unsafe_values() -> Result<(), Box<dyn Error>>
    {
        let request_id = "018f47f8-62a1-7a2a-aa2a-2a2a2a2a2a2a";
        let at_cap: super::HistoryMutationRequest = serde_json::from_value(json!({
            "requestId": request_id,
            "scenarioId": request_id,
            "expectedRevision": REVISION_MAX_V1
        }))?;
        assert_eq!(at_cap.expected_revision.value(), REVISION_MAX_V1);
        assert!(
            serde_json::from_value::<super::HistoryMutationRequest>(json!({
                "requestId": request_id,
                "scenarioId": request_id,
                "expectedRevision": REVISION_MAX_V1 + 1
            }))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn portable_preview_dtos_serialize_revision_and_exclusion_scope() -> Result<(), Box<dyn Error>>
    {
        let counts = serde_json::to_value(super::PortableCountsDto {
            scenarios: 1,
            scenario_revisions: 2,
            results: 3,
            shared_records: 4,
            preferences: 5,
            assets: 6,
        })?;
        assert_eq!(counts["scenarioRevisions"], 2);
        let tombstoned = serde_json::to_value(super::PortableScenarioDto {
            scenario_id: eutheto_types::ScenarioId::new(&SystemIdGenerator)?,
            title: "Previously deleted roster".to_owned(),
            collides: true,
            source_revision: Revision::new(2),
            same_identity_revision: Revision::new(6),
            same_identity_revision_warning: Some(
                "This ID was tombstoned; importing resumes at revision 6.".to_owned(),
            ),
        })?;
        assert_eq!(tombstoned["sourceRevision"], 2);
        assert_eq!(tombstoned["sameIdentityRevision"], 6);
        assert!(
            tombstoned["sameIdentityRevisionWarning"]
                .as_str()
                .is_some_and(|warning| warning.contains("tombstoned"))
        );
        let omission = serde_json::to_value((
            super::SourceBackupSelectionDto {
                include_results: false,
                asset_selection: "v1-threshold",
                threshold_version: Some(1),
                threshold_bytes: Some(16_777_216),
                excluded_asset_count: 1,
                excluded_asset_ids: vec!["large-video.mp4".to_owned()],
                fixed_exclusions: all_fixed_exclusions(),
                scope: "library",
            },
            super::OmittedAssetDto {
                asset_id: "large-video.mp4".to_owned(),
                format: "eutheto/omitted-asset".to_owned(),
                version: 1,
                reason: "above-v1-threshold",
                original_media_type: "video/mp4".to_owned(),
                original_size: 20_000_000,
                content_sha256: "a".repeat(64),
            },
        ))?;
        assert_eq!(omission[0]["includeResults"], false);
        assert_eq!(omission[0]["excludedAssetIds"][0], "large-video.mp4");
        assert_eq!(omission[1]["reason"], "above-v1-threshold");
        assert_eq!(
            omission[0]["fixedExclusions"].as_array().map(Vec::len),
            Some(7)
        );
        assert_eq!(omission[1]["originalMediaType"], "video/mp4");
        let inherited_summary =
            serde_json::to_value(backup_summary(eutheto_core::BackupSummary {
                include_results: true,
                asset_selection: BackupAssetSelection::IncludeAll,
                excluded_asset_count: 1,
                excluded_asset_ids: vec!["inherited-placeholder.png".to_owned()],
                exclusion_scope: Some("inherited-placeholder".to_owned()),
                fixed_exclusions: FixedExclusion::ALL.into_iter().collect(),
            }))?;
        assert_eq!(inherited_summary["assetSelection"], "all");
        assert_eq!(
            inherited_summary["excludedAssetIds"][0],
            "inherited-placeholder.png"
        );
        assert!(inherited_summary["thresholdBytes"].is_null());
        assert_eq!(
            inherited_summary["fixedExclusions"]
                .as_array()
                .map(Vec::len),
            Some(7)
        );
        Ok(())
    }

    #[test]
    fn portable_request_dtos_bind_sections_collisions_and_restore_authorization()
    -> Result<(), Box<dyn Error>> {
        let request_id = "018f47f8-62a1-7a2a-aa2a-2a2a2a2a2a2a";
        let options = portable_options();
        let preview: PortablePreviewRequest = serde_json::from_value(json!({
            "requestId": request_id,
            "options": options.clone()
        }))?;
        assert!(preview.options.include_results);
        assert!(preview.options.include_assets);

        let apply: super::PortableApplyRequest = serde_json::from_value(json!({
            "requestId": request_id,
            "previewId": "018f47f8-62a1-7a2a-aa2a-2a2a2a2a2a2b",
            "collisionPlan": {
                "scenarios": {
                    "018f47f8-62a1-7a2a-aa2a-2a2a2a2a2a2a": "create-copy"
                },
                "supplementalChoices": [{
                    "section": "preferences",
                    "key": "view.json",
                    "action": "skip"
                }]
            }
        }))?;
        assert!(matches!(
            apply.collision_plan.supplemental.values().next(),
            Some(SupplementalCollisionAction::Skip)
        ));

        let restore: super::RestoreApplyRequest = serde_json::from_value(json!({
            "requestId": request_id,
            "previewId": "018f47f8-62a1-7a2a-aa2a-2a2a2a2a2a2b",
            "collisionPlan": {"scenarios": {}, "supplementalChoices": []},
            "authorization": {
                "destructiveActionConfirmed": true,
                "safetyBackupBypassPhrase": "REPLACE WITHOUT BACKUP"
            }
        }))?;
        let authorization: eutheto_import::RestoreAuthorization = restore.authorization.into();
        assert!(authorization.destructive_action_confirmed);
        assert!(matches!(
            authorization.safety_backup,
            SafetyBackupEvidence::FailedWithStrongConfirmation { proof }
                if proof == "REPLACE WITHOUT BACKUP"
        ));
        assert!(authorization.prospective_failure_receipt_token.is_none());
        assert!(authorization.collision_plan_sha256.is_none());
        assert!(
            serde_json::from_value::<super::RestoreAuthorizationDto>(json!({
                "destructiveActionConfirmed": true,
                "safetyBackupBypassPhrase": null,
                "prospectiveFailureReceiptToken": "01900000-0000-7000-8000-000000000099"
            }))
            .is_err()
        );

        assert!(
            serde_json::from_value::<PortablePreviewRequest>(json!({
                "requestId": request_id,
                "sourceArtifact": "private/path.eutheto",
                "options": options.clone()
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PortablePreviewRequest>(json!({
                "requestId": request_id,
                "path": "/private/library.eutheto",
                "options": options
            }))
            .is_err()
        );

        serde_json::from_value::<ExportCreateRequest>(json!({
            "requestId": request_id,
            "scenarioId": request_id,
            "previewId": request_id,
        }))?;
        assert!(
            serde_json::from_value::<ExportCreateRequest>(json!({
                "requestId": request_id,
                "scenarioId": request_id,
                "previewId": request_id,
                "fileName": "renderer-chosen.eutheto"
            }))
            .is_err()
        );

        serde_json::from_value::<BackupCreateRequest>(json!({
            "requestId": request_id,
            "title": "Before migration",
            "previewId": request_id,
        }))?;
        assert!(
            serde_json::from_value::<BackupCreateRequest>(json!({
                "requestId": request_id,
                "title": "Before migration",
                "previewId": request_id,
                "fileName": "renderer-chosen.eutheto"
            }))
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn native_dialog_seam_maps_cancellation_without_opening_a_dialog()
    -> Result<(), Box<dyn Error>> {
        let Err(error) = native_file_task::<(), _>(|| Err(NativeFileError::Cancelled)).await else {
            return Err("cancelled dialog unexpectedly succeeded".into());
        };
        assert_eq!(error.code, "operation.cancelled");
        assert_eq!(error.category, ApiErrorCategoryDto::Protocol);
        assert!(error.retryable);
        assert!(error.field_errors.is_empty());
        assert!(error.details.is_none());
        assert!(error.diagnostic_id.is_none());

        let conversion = map_native_file_error(NativeFileError::Conversion);
        assert_eq!(conversion.code, "portable.selection_invalid");
        assert_eq!(conversion.category, ApiErrorCategoryDto::Protocol);
        assert!(!conversion.retryable);

        let unreadable = map_native_file_error(NativeFileError::Unreadable);
        assert_eq!(unreadable.code, "portable.artifact_unreadable");
        assert_eq!(unreadable.category, ApiErrorCategoryDto::Storage);
        Ok(())
    }

    #[test]
    fn suggested_portable_names_are_sanitized_and_bounded() {
        assert_eq!(
            suggested_portable_filename("../../Quarter: One?", "Eutheto-Export"),
            "Quarter-One.eutheto"
        );
        assert_eq!(
            suggested_portable_filename("CON", "Eutheto-Export"),
            "Eutheto-Export.eutheto"
        );
        assert!(
            suggested_portable_filename(&"é".repeat(500), "Eutheto-Export").len()
                <= eutheto_export::PORTABLE_LIMITS.max_path_bytes
        );
        assert_eq!(
            selected_basename(std::path::Path::new("/private/folder/export.eutheto")),
            Ok("export.eutheto".to_owned())
        );
    }

    #[test]
    fn native_portable_reads_reject_links_and_non_regular_files() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let regular = directory.path().join("regular.eutheto");
        std::fs::write(&regular, b"portable")?;
        assert_eq!(read_bounded_portable(&regular), Ok(b"portable".to_vec()));
        assert_eq!(
            read_bounded_portable(directory.path()),
            Err(NativeFileError::InvalidFileType)
        );
        let oversized = directory.path().join("oversized.eutheto");
        let oversized_file = std::fs::File::create(&oversized)?;
        oversized_file.set_len(eutheto_export::PORTABLE_LIMITS.max_archive_bytes + 1)?;
        assert_eq!(
            read_bounded_portable(&oversized),
            Err(NativeFileError::TooLarge)
        );
        let oversized_error = map_native_file_error(NativeFileError::TooLarge);
        assert_eq!(oversized_error.code, "portable.archive_too_large");
        assert_eq!(oversized_error.category, ApiErrorCategoryDto::Storage);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = directory.path().join("link.eutheto");
            symlink(&regular, &link)?;

            assert_eq!(
                read_bounded_portable(&link),
                Err(NativeFileError::InvalidFileType)
            );
        }
        Ok(())
    }
    #[test]
    fn solver_support_matrix_dto_preserves_exact_validated_cells() -> Result<(), Box<dyn Error>> {
        let unsupported_id = SupportFeatureId::new("primitive.fixture-unsupported")?;
        let degraded_id = SupportFeatureId::new("solve.fixture-degraded")?;
        let backend_id = BackendId::new("solver.fixture")?;
        let matrix = CapabilityMatrix::new(
            1,
            2,
            vec![
                SupportFeature {
                    id: degraded_id.clone(),
                    category: SupportFeatureCategory::Solve,
                    gate: SupportFeatureGate::Enabled("phase.fixture".to_owned()),
                },
                SupportFeature {
                    id: unsupported_id.clone(),
                    category: SupportFeatureCategory::Primitive,
                    gate: SupportFeatureGate::Unconditional,
                },
            ],
            vec![BackendSupportColumn {
                backend_id,
                backend_version: "0.0-fixture".to_owned(),
                adapter_version: "adapter-fixture-v2".to_owned(),
                cells: vec![
                    (
                        degraded_id,
                        SupportCell::Degraded {
                            restriction_id: "restriction.fixture-cap".to_owned(),
                            reason: "Fixture degradation reason".to_owned(),
                            remediation: "Use the unrestricted fixture mode".to_owned(),
                            fixture_id: "fixture.degraded-exact".to_owned(),
                        },
                    ),
                    (
                        unsupported_id,
                        SupportCell::Unsupported {
                            reason: "Fixture unsupported reason".to_owned(),
                            remediation: "Choose the fixture alternative".to_owned(),
                            fixture_id: "fixture.unsupported-exact".to_owned(),
                        },
                    ),
                ],
            }],
            Vec::new(),
        )?;
        let metadata = SolverSupportMatrixMetadata {
            schema_version: matrix.schema_version(),
            planning_ir_schema_version: matrix.planning_ir_schema_version(),
            features: matrix.features().cloned().collect(),
            production_backend_ids: matrix.production_backend_ids().cloned().collect(),
            backend_columns: matrix.backend_columns().collect(),
        };

        let dto = serde_json::to_value(solver_support_matrix_dto(metadata))?;
        assert_eq!(
            dto["backendColumns"],
            json!([
                {
                    "backendId": "solver.fixture",
                    "backendVersion": "0.0-fixture",
                    "adapterVersion": "adapter-fixture-v2",
                    "cells": [
                        {
                            "featureId": "primitive.fixture-unsupported",
                            "support": "unsupported",
                            "reason": "Fixture unsupported reason",
                            "remediation": "Choose the fixture alternative",
                            "fixtureId": "fixture.unsupported-exact"
                        },
                        {
                            "featureId": "solve.fixture-degraded",
                            "support": "degraded",
                            "restrictionId": "restriction.fixture-cap",
                            "reason": "Fixture degradation reason",
                            "remediation": "Use the unrestricted fixture mode",
                            "fixtureId": "fixture.degraded-exact"
                        }
                    ]
                }
            ])
        );
        Ok(())
    }

    #[test]
    fn phase_04_solution_capabilities_and_event_topics_are_exact() -> Result<(), Box<dyn Error>> {
        let capabilities = app_get_capabilities(RequestOnly {
            request_id: RequestId::new(&SystemIdGenerator)?,
        })
        .result;
        for command in [
            "solution_list",
            "solution_get_summary",
            "solution_get_view",
            "solution_select",
            "solution_verify",
            "solution_compare",
            "solution_explain",
            "solution_start_counterfactual",
            "solution_cancel_counterfactual",
        ] {
            assert!(capabilities.available_commands.contains(&command));
            assert!(!capabilities.unavailable_commands.contains(&command));
        }
        for command in [
            "solution_lock_assignment",
            "solution_unlock_assignment",
            "solution_create_repair_request",
            "solution_export_preview",
            "solution_export",
            "solution_share_preview",
            "solution_share_create",
            "solution_export_cancel",
        ] {
            assert!(!capabilities.available_commands.contains(&command));
            assert!(capabilities.unavailable_commands.contains(&command));
        }

        for expected in [
            (EventTopic::SolveProgress, "solve://progress"),
            (EventTopic::SolveCompleted, "solve://completed"),
            (
                EventTopic::ScenarioValidationChanged,
                "scenario://validation-changed",
            ),
            (
                EventTopic::CounterfactualProgress,
                "counterfactual://progress",
            ),
        ] {
            assert_eq!(
                FORWARDED_EVENTS
                    .iter()
                    .filter(|mapping| **mapping == expected)
                    .count(),
                1
            );
        }
        Ok(())
    }

    #[tokio::test]
    // The successful command, full safe error envelope, and malformed matrix stay auditable together.
    #[allow(clippy::too_many_lines)]
    async fn solution_list_ipc_is_strict_and_serializes_the_core_dto() -> Result<(), Box<dyn Error>>
    {
        let directory = tempfile::tempdir()?;
        let app = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("solutions.sqlite3"),
                safety_backups: directory.path().join("solution-backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        })
        .await
        .map_err(|error| format!("solution app setup failed: {error:?}"))?;
        let settings: ScenarioSettings = serde_json::from_value(json!({
            "timeZone": "UTC",
            "locale": "en-US",
            "units": "metric",
            "horizon": {
                "start": "2026-09-01T00:00:00Z",
                "end": "2026-10-01T00:00:00Z"
            },
            "gapPolicy": "reject",
            "overlapPolicy": "earlier"
        }))?;
        let scenario_id = match app
            .execute(AppCommand::CreateProject {
                request_id: RequestId::new(&SystemIdGenerator)?,
                title: "Solution adapter fixture".to_owned(),
                description: String::new(),
                domain_pack: DomainPackRef {
                    id: "official.test".parse()?,
                    schema_version: 1,
                },
                settings,
            })
            .await
            .map_err(|error| format!("solution project setup failed: {error:?}"))?
        {
            AppCommandResult::Project(project) => project.scenario_id,
            result => return Err(format!("unexpected solution project result: {result:?}").into()),
        };
        let state = DesktopState::new(
            app,
            directory.path().join("solution-cache"),
            directory.path().join("solution-backups"),
        );
        let desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![solution_explain, solution_list])
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let webview =
            tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default())
                .build()?;
        let result: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "solution_list",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "scenarioId": scenario_id,
            }),
        )?;
        assert_eq!(result.current_revision, Some(Revision::INITIAL));
        assert_eq!(
            result.result,
            json!({
                "schemaVersion": 1,
                "scenarioId": scenario_id,
                "currentRevision": 0,
                "solutions": [],
            })
        );
        let explanation_request = json!({
            "schemaVersion": 1,
            "subject": {
                "kind": "validation",
                "issueId": null,
            },
        });
        let explanation_envelope = json!({
            "requestId": RequestId::new(&SystemIdGenerator)?,
            "schemaVersion": 1,
            "scenarioId": scenario_id,
            "request": explanation_request,
        });
        let _: CorrelatedRequest<SolutionExplainRequestV1> =
            serde_json::from_value(explanation_envelope.clone())?;
        let explanation = invoke_ipc(&webview, "solution_explain", &explanation_envelope)?;
        let Err(explanation) = explanation else {
            return Err("unavailable validation explanation unexpectedly succeeded".into());
        };
        assert_eq!(
            explanation["code"],
            "solution.explanation_validation_unavailable"
        );

        let malformed = invoke_ipc(
            &webview,
            "solution_list",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "scenarioId": scenario_id,
                "unexpected": true,
            }),
        )?;
        let Err(malformed) = malformed else {
            return Err("malformed solution request unexpectedly succeeded".into());
        };
        let expected_error = json!({
            "code": "solution.request_invalid",
            "message": "The solution request is missing or malformed.",
            "category": "validation",
            "retryable": false,
            "fieldErrors": [{
                "field": "/request",
                "code": "solution.request_invalid",
                "message": "The solution request is missing or malformed.",
            }],
            "details": null,
            "diagnosticId": null,
        });
        assert_eq!(malformed, expected_error);
        for malformed_request in [
            json!("not an object"),
            json!({
                "schemaVersion": 1,
                "scenarioId": scenario_id,
            }),
            json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "scenarioId": "not-a-uuid",
            }),
        ] {
            let malformed = invoke_ipc(&webview, "solution_list", &malformed_request)?;
            let Err(malformed) = malformed else {
                return Err("malformed solution request unexpectedly succeeded".into());
            };
            assert_eq!(malformed, expected_error);
        }
        Ok(())
    }

    #[test]
    fn solution_response_preserves_every_wide_integer_losslessly() -> Result<(), Box<dyn Error>> {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct IntegerFixture {
            minimum: i64,
            beyond_safe_integer: i64,
            maximum: i64,
            safe_unsigned: u64,
            wide_unsigned: u64,
        }

        let response = solution_response(
            RequestId::new(&SystemIdGenerator)?,
            Revision::INITIAL,
            IntegerFixture {
                minimum: i64::MIN,
                beyond_safe_integer: 9_007_199_254_740_993,
                maximum: i64::MAX,
                safe_unsigned: 9_007_199_254_740_991,
                wide_unsigned: u64::MAX,
            },
        )
        .map_err(|error| format!("integer fixture serialization failed: {}", error.message))?;
        let body = tauri::ipc::IpcResponse::body(response)?;
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Err("solution response was not emitted as JSON".into());
        };
        let value: Value = serde_json::from_str(&json)?;
        assert_eq!(value["result"]["minimum"], i64::MIN.to_string());
        assert_eq!(value["result"]["beyondSafeInteger"], "9007199254740993");
        assert_eq!(value["result"]["maximum"], i64::MAX.to_string());
        assert_eq!(value["result"]["safeUnsigned"], 9_007_199_254_740_991_u64);
        assert_eq!(value["result"]["wideUnsigned"], u64::MAX.to_string());
        Ok(())
    }

    #[test]
    fn counterfactual_start_normalizes_lossless_signed_interval_strings()
    -> Result<(), Box<dyn Error>> {
        let request_id = RequestId::new(&SystemIdGenerator)?;
        let scenario_id = eutheto_types::ScenarioId::new(&SystemIdGenerator)?;
        let solution_id = eutheto_types::SolutionId::new(&SystemIdGenerator)?;
        let request = json!({
            "schemaVersion": 1,
            "requestId": request_id,
            "scenarioId": scenario_id,
            "expectedRevision": 0,
            "baseSolutionId": solution_id,
            "condition": {
                "type": "forceAssignmentValue",
                "assignmentId": "assignment.fixture",
                "value": {
                    "type": "interval",
                    "value": {
                        "start": i64::MIN.to_string(),
                        "duration": "9007199254740993",
                        "end": i64::MAX.to_string(),
                    },
                },
            },
            "totalBudgetMilliseconds": 1000,
        });
        let mut normalized = request.clone();
        normalize_counterfactual_int64(&mut normalized)
            .map_err(|error| format!("integer normalization failed: {}", error.message))?;
        let direct: SolutionStartCounterfactualRequestV1 = serde_json::from_value(normalized)?;
        let decoded = decode_counterfactual_start_request(Some(request))
            .map_err(|error| format!("counterfactual request decode failed: {}", error.message))?;
        assert_eq!(decoded, direct);
        let decoded = serde_json::to_value(decoded)?;
        assert_eq!(
            decoded.pointer("/condition/value/value/start"),
            Some(&json!(i64::MIN))
        );
        assert_eq!(
            decoded.pointer("/condition/value/value/duration"),
            Some(&json!(9_007_199_254_740_993_i64))
        );
        assert_eq!(
            decoded.pointer("/condition/value/value/end"),
            Some(&json!(i64::MAX))
        );

        let invalid = decode_counterfactual_start_request(Some(json!({
            "schemaVersion": 1,
            "requestId": request_id,
            "scenarioId": scenario_id,
            "expectedRevision": 0,
            "baseSolutionId": solution_id,
            "condition": {
                "type": "forbidAssignmentValue",
                "assignmentId": "assignment.fixture",
                "value": {
                    "type": "integer",
                    "value": "9223372036854775808",
                },
            },
            "totalBudgetMilliseconds": 1000,
        })));
        let Err(invalid) = invalid else {
            return Err("out-of-range signed integer unexpectedly decoded".into());
        };
        assert_eq!(invalid.code, "solution.request_invalid");
        assert_eq!(invalid.category, ApiErrorCategoryDto::Validation);
        assert_eq!(invalid.field_errors[0].field, "/request");
        Ok(())
    }
    // One adapter flow ties every Phase 02 metadata view to the same registry-backed state.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn phase_02_metadata_commands_are_registry_derived_without_solver_claims()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let app = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("metadata.sqlite3"),
                safety_backups: directory.path().join("metadata-backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        })
        .await
        .map_err(|error| format!("metadata app setup failed: {error:?}"))?;
        let state = DesktopState::new(
            app,
            directory.path().join("metadata-cache"),
            directory.path().join("metadata-backups"),
        );
        let desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                pack_list,
                pack_describe,
                solver_list,
                solver_describe,
                solver_get_support_matrix,
                solver_get_deferred_gates,
            ])
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let webview =
            tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default())
                .build()?;
        let ids = SystemIdGenerator;

        let packs: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "pack_list",
            &json!({ "requestId": RequestId::new(&ids)? }),
        )?;
        let pack_items = packs
            .result
            .as_array()
            .ok_or("pack list result must be an array")?;
        assert_eq!(pack_items.len(), 2);
        assert_eq!(pack_items[0]["id"], "official.test");
        assert_eq!(pack_items[0]["syntheticTestOnly"], true);
        assert_eq!(pack_items[1]["id"], "official.workforce");
        assert_eq!(pack_items[1]["syntheticTestOnly"], false);

        let described: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "pack_describe",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "packId": "official.test"
            }),
        )?;
        assert_eq!(described.result["descriptor"], pack_items[0]);
        assert_eq!(described.result["catalog"]["packId"], "official.test");
        assert!(
            described.result["catalog"]["commands"]
                .as_array()
                .is_some_and(|commands| !commands.is_empty())
        );

        let solvers: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "solver_list",
            &json!({ "requestId": RequestId::new(&ids)? }),
        )?;
        assert_eq!(solvers.result, json!([]));

        let matrix: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "solver_get_support_matrix",
            &json!({ "requestId": RequestId::new(&ids)? }),
        )?;
        assert_eq!(
            matrix.result["productionBackendIds"],
            json!(["solver.ortools-cp-sat"])
        );
        let backend_columns = matrix.result["backendColumns"]
            .as_array()
            .ok_or("support matrix omitted backend columns")?;
        assert_eq!(backend_columns.len(), 1);
        assert_eq!(backend_columns[0]["backendId"], "solver.ortools-cp-sat");
        assert_eq!(backend_columns[0]["backendVersion"], "9.15.6755");
        assert_eq!(backend_columns[0]["adapterVersion"], "0.1.0");
        assert_eq!(
            backend_columns[0]["cells"]
                .as_array()
                .ok_or("OR-Tools matrix column omitted cells")?
                .len(),
            35
        );
        assert!(
            matrix.result["features"]
                .as_array()
                .is_some_and(|features| !features.is_empty())
        );

        let gates: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "solver_get_deferred_gates",
            &json!({ "requestId": RequestId::new(&ids)? }),
        )?;
        assert_eq!(
            gates.result,
            json!([
                {
                    "backendId": "solver.pumpkin",
                    "candidateVersion": "0.5.0",
                    "owningPhase": 8
                }
            ])
        );

        let unavailable = invoke_ipc(
            &webview,
            "solver_describe",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "backendId": "solver.ortools-cp-sat"
            }),
        )?;
        let Err(unavailable) = unavailable else {
            return Err("deferred solver unexpectedly described as available".into());
        };
        assert_eq!(unavailable["code"], "resource.not_found");
        assert_eq!(unavailable["category"], "notFound");
        Ok(())
    }

    // One sequential flow proves exact-byte publication and every single-use capability outcome.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn unopened_bundle_adapter_preserves_exact_bytes_and_consumes_capabilities()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let app = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("unopened.sqlite3"),
                safety_backups: directory.path().join("unopened-backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        })
        .await
        .map_err(|error| format!("unopened app setup failed: {error:?}"))?;
        let state = DesktopState::new(
            app,
            directory.path().join("unopened-cache"),
            directory.path().join("unopened-backups"),
        );
        let original = match state
            .app
            .query(AppQuery::ExportBackup {
                title: "Exact unopened fixture".to_owned(),
                selection: eutheto_core::BackupSelection::default(),
            })
            .await
            .map_err(|error| format!("fixture export failed: {error:?}"))?
        {
            AppQueryResult::BackupBundle { bytes, .. } => bytes,
            other => return Err(format!("unexpected fixture export result: {other:?}").into()),
        };
        let ids = SystemIdGenerator;

        let inspected =
            inspect_unopened_bundle_bytes(&state, RequestId::new(&ids)?, original.clone())
                .await
                .map_err(|error| format!("unopened inspection failed: {error:?}"))?;
        assert_eq!(
            inspected.result.metadata.title.as_deref(),
            Some("Exact unopened fixture")
        );
        assert_eq!(
            inspected.result.metadata.file_sha256,
            eutheto_export::sha256_hex(&original)
        );
        let exposed = serde_json::to_value(&inspected.result)?;
        assert!(exposed.get("bytes").is_none());
        assert!(exposed["metadata"].get("bytes").is_none());

        let destination = directory.path().join("exact-copy.eutheto");
        exact_reexport_unopened_bundle_to_path(
            &state,
            PortableCancelRequest {
                request_id: RequestId::new(&ids)?,
                preview_id: inspected.result.preview_id,
            },
            destination.clone(),
            "exact-copy.eutheto".to_owned(),
        )
        .await
        .map_err(|error| format!("exact re-export failed: {error:?}"))?;
        assert_eq!(std::fs::read(&destination)?, original);

        let consumed = exact_reexport_unopened_bundle_to_path(
            &state,
            PortableCancelRequest {
                request_id: RequestId::new(&ids)?,
                preview_id: inspected.result.preview_id,
            },
            directory.path().join("second-copy.eutheto"),
            "second-copy.eutheto".to_owned(),
        )
        .await;
        let Err(consumed) = consumed else {
            return Err("single-use preview unexpectedly re-exported twice".into());
        };
        assert_eq!(consumed.code, "portable.preview_not_found");

        let no_clobber =
            inspect_unopened_bundle_bytes(&state, RequestId::new(&ids)?, original.clone())
                .await
                .map_err(|error| format!("second inspection failed: {error:?}"))?;
        let occupied = directory.path().join("occupied.eutheto");
        std::fs::write(&occupied, b"sentinel")?;
        let publication = exact_reexport_unopened_bundle_to_path(
            &state,
            PortableCancelRequest {
                request_id: RequestId::new(&ids)?,
                preview_id: no_clobber.result.preview_id,
            },
            occupied.clone(),
            "occupied.eutheto".to_owned(),
        )
        .await;
        let Err(publication) = publication else {
            return Err("exact re-export unexpectedly replaced an existing file".into());
        };
        assert_eq!(publication.category, ApiErrorCategoryDto::Storage);
        assert_eq!(std::fs::read(&occupied)?, b"sentinel");
        let after_no_clobber = exact_reexport_unopened_bundle_to_path(
            &state,
            PortableCancelRequest {
                request_id: RequestId::new(&ids)?,
                preview_id: no_clobber.result.preview_id,
            },
            directory.path().join("after-no-clobber.eutheto"),
            "after-no-clobber.eutheto".to_owned(),
        )
        .await;
        let Err(after_no_clobber) = after_no_clobber else {
            return Err("failed publication unexpectedly retained its preview".into());
        };
        assert_eq!(after_no_clobber.code, "portable.preview_not_found");

        let cancelled = inspect_unopened_bundle_bytes(&state, RequestId::new(&ids)?, original)
            .await
            .map_err(|error| format!("third inspection failed: {error:?}"))?;
        cancel_portable_preview_impl(
            &state,
            PortableCancelRequest {
                request_id: RequestId::new(&ids)?,
                preview_id: cancelled.result.preview_id,
            },
        )
        .await
        .map_err(|error| format!("unopened cancellation failed: {error:?}"))?;
        let after_cancel = exact_reexport_unopened_bundle_to_path(
            &state,
            PortableCancelRequest {
                request_id: RequestId::new(&ids)?,
                preview_id: cancelled.result.preview_id,
            },
            directory.path().join("cancelled.eutheto"),
            "cancelled.eutheto".to_owned(),
        )
        .await;
        let Err(after_cancel) = after_cancel else {
            return Err("cancelled unopened preview unexpectedly re-exported".into());
        };
        assert_eq!(after_cancel.code, "portable.preview_not_found");

        let malformed =
            inspect_unopened_bundle_bytes(&state, RequestId::new(&ids)?, b"not a zip".to_vec())
                .await;
        let Err(malformed) = malformed else {
            return Err("malformed unopened bundle unexpectedly inspected".into());
        };
        assert_eq!(malformed.code, "portable.content_invalid");
        assert_eq!(malformed.category, ApiErrorCategoryDto::Validation);
        Ok(())
    }

    #[tokio::test]
    async fn project_list_delegates_and_round_trips_request_id() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let app = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("library.sqlite3"),
                safety_backups: directory.path().join("backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        })
        .await
        .map_err(|error| format!("app setup failed: {error:?}"))?;
        let state = DesktopState::new(
            app,
            directory.path().join("cache"),
            directory.path().join("backups"),
        );
        let request_id = RequestId::new(&SystemIdGenerator)?;
        let result = project_list_impl(
            &state,
            ProjectListRequest {
                request_id,
                scope: ProjectScopeDto::Active,
            },
        )
        .await
        .map_err(|error| format!("project list failed: {error:?}"))?;
        assert_eq!(result.request_id, request_id);
        assert!(result.result.is_empty());
        let Err(deferred) = core_unavailable(&state, DeferredCapability::Solve).await else {
            return Err("solve unexpectedly became available".into());
        };
        assert_eq!(deferred.category, ApiErrorCategoryDto::Unsupported);
        assert_eq!(deferred.code, "capability.solve_unavailable");
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn native_portable_commands_replace_by_identity_and_persist() -> Result<(), Box<dyn Error>>
    {
        let directory = tempfile::tempdir()?;
        let ids = SystemIdGenerator;
        let settings: ScenarioSettings = serde_json::from_value(json!({
            "timeZone": "UTC",
            "locale": "en-US",
            "units": "metric",
            "horizon": {
                "start": "2026-09-01T00:00:00Z",
                "end": "2026-10-01T00:00:00Z"
            },
            "gapPolicy": "reject",
            "overlapPolicy": "earlier"
        }))?;
        let title = "Portable identity collision";

        let source = EuthetoApp::open(AppDependencies {
            paths: AppPaths {
                database: directory.path().join("source.sqlite3"),
                safety_backups: directory.path().join("source-backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        })
        .await
        .map_err(|error| format!("source app setup failed: {error:?}"))?;
        let source_scenario_id = match source
            .execute(AppCommand::CreateProject {
                request_id: RequestId::new(&ids)?,
                title: title.to_owned(),
                description: "Portable command boundary source".to_owned(),
                domain_pack: DomainPackRef {
                    id: "official.test".parse()?,
                    schema_version: 1,
                },
                settings: settings.clone(),
            })
            .await
            .map_err(|error| format!("source project setup failed: {error:?}"))?
        {
            AppCommandResult::Project(project) => project.scenario_id,
            result => return Err(format!("unexpected source project result: {result:?}").into()),
        };
        let initial_bytes = match source
            .query(AppQuery::ExportScenario(source_scenario_id))
            .await
            .map_err(|error| format!("initial source export failed: {error:?}"))?
        {
            AppQueryResult::Bundle { bytes, .. } => bytes,
            result => return Err(format!("unexpected initial export result: {result:?}").into()),
        };

        let target_dependencies = AppDependencies {
            paths: AppPaths {
                database: directory.path().join("target.sqlite3"),
                safety_backups: directory.path().join("target-backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        };
        let target_app = EuthetoApp::open(target_dependencies.clone())
            .await
            .map_err(|error| format!("target app setup failed: {error:?}"))?;
        let target_state = DesktopState::new(
            target_app,
            directory.path().join("target-cache"),
            directory.path().join("target-backups"),
        );
        let target_desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                project_create,
                project_import_apply,
                project_list,
                operation_prepare,
                scenario_get_summary,
            ])
            .manage(target_state.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let target_webview =
            tauri::WebviewWindowBuilder::new(&target_desktop, "main", tauri::WebviewUrl::default())
                .build()?;

        let unrelated: ApiResponseDto<ProjectMetadataDto> = invoke_ok(
            &target_webview,
            "project_create",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "title": title,
                "description": "Same title, different identity",
                "domainPack": {
                    "id": "official.test",
                    "schemaVersion": 1
                },
                "settings": settings
            }),
        )?;
        let unrelated_scenario_id = unrelated.result.scenario_id;
        assert_ne!(unrelated_scenario_id, source_scenario_id);

        let selected_path = directory.path().join("selected.eutheto");
        std::fs::write(&selected_path, initial_bytes)?;
        let first_preview_request_id = RequestId::new(&ids)?;
        let first_preview_request: PortablePreviewRequest = serde_json::from_value(json!({
            "requestId": first_preview_request_id,
            "options": {
                "restoreMode": "import-scenario",
                "includeResults": false,
                "includeAssets": false
            }
        }))?;
        let first_preview_bytes = read_bounded_portable(&selected_path)
            .map_err(|error| format!("initial portable read failed: {error:?}"))?;
        let first_preview = preview_portable_bytes(
            &target_state,
            first_preview_request,
            first_preview_bytes,
            false,
        )
        .await
        .map_err(|error| format!("initial portable preview failed: {error:?}"))?;
        assert_eq!(first_preview.request_id, first_preview_request_id);
        assert_eq!(first_preview.result.scenarios.len(), 1);
        assert_eq!(
            first_preview.result.scenarios[0].scenario_id,
            source_scenario_id
        );
        assert!(!first_preview.result.scenarios[0].collides);
        let first_preview_id = first_preview.result.preview_id;

        let first_apply_request_id = RequestId::new(&ids)?;
        let first_applied: ApiResponseDto<Value> = invoke_ok(
            &target_webview,
            "project_import_apply",
            &json!({
                "requestId": first_apply_request_id,
                "previewId": first_preview_id,
                "collisionPlan": eutheto_import::CollisionPlan::default()
            }),
        )?;
        assert_eq!(first_applied.request_id, first_apply_request_id);
        assert_eq!(
            first_applied.result["scenarioIds"],
            json!([source_scenario_id])
        );

        let source_entity_id = EntityId::new(&ids)?;
        let source_entity = json!({
            "id": source_entity_id.to_string(),
            "name": "Authoritative portable entity"
        });
        let source_mutation = source
            .execute(AppCommand::ApplyScenario {
                request_id: RequestId::new(&ids)?,
                envelope: CommandEnvelope {
                    command_id: CommandId::new(&ids)?,
                    scenario_id: source_scenario_id,
                    expected_revision: Revision::INITIAL,
                    actor: ActorRef {
                        actor_id: Some("desktop.portable.test".to_owned()),
                        display_name: "Desktop Portable Test".to_owned(),
                    },
                    source: CommandSource::System,
                    command: ScenarioCommand::AddEntity(AddEntity {
                        entity_id: source_entity_id,
                        value: source_entity.clone(),
                    }),
                },
                truncate_redo: false,
            })
            .await
            .map_err(|error| format!("source mutation failed: {error:?}"))?;
        assert!(matches!(
            &source_mutation,
            AppCommandResult::ScenarioCommand(result)
                if result.new_revision == Revision::new(1)
        ));
        let replacement_bytes = match source
            .query(AppQuery::ExportScenario(source_scenario_id))
            .await
            .map_err(|error| format!("replacement source export failed: {error:?}"))?
        {
            AppQueryResult::Bundle { bytes, .. } => bytes,
            result => {
                return Err(format!("unexpected replacement export result: {result:?}").into());
            }
        };

        std::fs::write(&selected_path, replacement_bytes)?;
        let replace_preview_request_id = RequestId::new(&ids)?;
        let replace_preview_request: PortablePreviewRequest = serde_json::from_value(json!({
            "requestId": replace_preview_request_id,
            "options": {
                "restoreMode": "import-scenario",
                "includeResults": false,
                "includeAssets": false
            }
        }))?;
        let replace_preview_bytes = read_bounded_portable(&selected_path)
            .map_err(|error| format!("replacement portable read failed: {error:?}"))?;
        let replace_preview = preview_portable_bytes(
            &target_state,
            replace_preview_request,
            replace_preview_bytes,
            false,
        )
        .await
        .map_err(|error| format!("replace portable preview failed: {error:?}"))?;
        assert_eq!(replace_preview.request_id, replace_preview_request_id);
        assert_eq!(replace_preview.result.scenarios.len(), 1);
        let collision = &replace_preview.result.scenarios[0];
        assert_eq!(collision.scenario_id, source_scenario_id);
        assert!(collision.collides);
        assert_eq!(collision.source_revision, Revision::new(1));
        assert_eq!(collision.same_identity_revision, Revision::new(1));
        assert!(collision.same_identity_revision_warning.is_none());
        let replace_preview_id = replace_preview.result.preview_id;
        let replace_request_id = RequestId::new(&ids)?;
        let replaced: ApiResponseDto<Value> = invoke_ok(
            &target_webview,
            "project_import_apply",
            &json!({
                "requestId": replace_request_id,
                "previewId": replace_preview_id,
                "collisionPlan": eutheto_import::CollisionPlan {
                    scenarios: BTreeMap::from([(
                        source_scenario_id,
                        CollisionAction::Replace
                    )]),
                    supplemental: BTreeMap::new()
                }
            }),
        )?;
        assert_eq!(replaced.request_id, replace_request_id);
        assert_eq!(replaced.result["scenarioIds"], json!([source_scenario_id]));

        let consumed = invoke_ipc(
            &target_webview,
            "project_import_apply",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "previewId": replace_preview_id,
                "collisionPlan": eutheto_import::CollisionPlan::default()
            }),
        )?;
        let consumed_error: ApiErrorDto = match consumed {
            Ok(_) => return Err("consumed portable preview unexpectedly applied twice".into()),
            Err(error) => serde_json::from_value(error)?,
        };
        assert_eq!(consumed_error.code, "portable.preview_not_found");

        drop(target_webview);
        drop(target_desktop);
        drop(target_state);

        let reopened_app = EuthetoApp::open(target_dependencies)
            .await
            .map_err(|error| format!("target reopen failed: {error:?}"))?;
        let reopened_desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                project_list,
                operation_prepare,
                scenario_get_summary
            ])
            .manage(DesktopState::new(
                reopened_app,
                directory.path().join("reopened-cache"),
                directory.path().join("target-backups"),
            ))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let reopened_webview = tauri::WebviewWindowBuilder::new(
            &reopened_desktop,
            "main",
            tauri::WebviewUrl::default(),
        )
        .build()?;

        let listed: ApiResponseDto<Vec<ProjectSummaryDto>> = invoke_ok(
            &reopened_webview,
            "project_list",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "scope": "active"
            }),
        )?;
        assert_eq!(listed.result.len(), 2);
        assert!(
            listed
                .result
                .iter()
                .any(|project| project.scenario_id == source_scenario_id)
        );
        assert!(
            listed
                .result
                .iter()
                .any(|project| project.scenario_id == unrelated_scenario_id)
        );

        let (imported, imported_snapshot) = native_summary_and_core_snapshot(
            &reopened_webview,
            RequestId::new(&ids)?,
            source_scenario_id,
        )
        .await?;
        assert_eq!(imported.result.title, title);
        assert_eq!(
            imported_snapshot
                .document
                .domain
                .entities
                .get(&source_entity_id),
            Some(&source_entity)
        );

        let (same_title, _) = native_summary_and_core_snapshot(
            &reopened_webview,
            RequestId::new(&ids)?,
            unrelated_scenario_id,
        )
        .await?;
        assert_eq!(same_title.result.title, title);
        assert_eq!(same_title.result.revision, Revision::INITIAL);
        assert_eq!(same_title.result.structure.entities, 0);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn desktop_commands_persist_across_reopen_and_reject_stale_mutations()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let dependencies = AppDependencies {
            paths: AppPaths {
                database: directory.path().join("library.sqlite3"),
                safety_backups: directory.path().join("backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::default(),
        };
        let first_app = EuthetoApp::open(dependencies.clone())
            .await
            .map_err(|error| format!("initial app setup failed: {error:?}"))?;
        let first_desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                project_create,
                project_list,
                operation_prepare,
                scenario_get_summary,
                scenario_apply_command
            ])
            .manage(DesktopState::new(
                first_app,
                directory.path().join("cache"),
                directory.path().join("backups"),
            ))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let first_webview =
            tauri::WebviewWindowBuilder::new(&first_desktop, "main", tauri::WebviewUrl::default())
                .build()?;
        let ids = SystemIdGenerator;
        let create_request_id = RequestId::new(&ids)?;
        let created: ApiResponseDto<ProjectMetadataDto> = invoke_ok(
            &first_webview,
            "project_create",
            &json!({
                "requestId": create_request_id,
                "title": "Persisted clinic roster",
                "description": "Desktop command boundary regression",
                "domainPack": {
                    "id": "official.test",
                    "schemaVersion": 1
                },
                "settings": {
                    "timeZone": "UTC",
                    "locale": "en-US",
                    "units": "metric",
                    "horizon": {
                        "start": "2026-09-01T00:00:00Z",
                        "end": "2026-10-01T00:00:00Z"
                    },
                    "gapPolicy": "reject",
                    "overlapPolicy": "earlier"
                }
            }),
        )?;
        assert_eq!(created.request_id, create_request_id);
        assert_eq!(created.current_revision, Some(Revision::INITIAL));
        let scenario_id = created.result.scenario_id;
        drop(first_webview);
        drop(first_desktop);

        let reopened_app = EuthetoApp::open(dependencies)
            .await
            .map_err(|error| format!("reopened app setup failed: {error:?}"))?;
        let reopened_desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                project_create,
                project_list,
                operation_prepare,
                scenario_get_summary,
                scenario_apply_command,
                scenario_undo,
                scenario_redo,
                project_import_apply,
                project_restore_apply,
                project_archive,
                project_unarchive,
                project_delete,
            ])
            .manage(DesktopState::new(
                reopened_app,
                directory.path().join("cache"),
                directory.path().join("backups"),
            ))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let reopened_webview = tauri::WebviewWindowBuilder::new(
            &reopened_desktop,
            "main",
            tauri::WebviewUrl::default(),
        )
        .build()?;

        let list_request_id = RequestId::new(&ids)?;
        let listed: ApiResponseDto<Vec<ProjectSummaryDto>> = invoke_ok(
            &reopened_webview,
            "project_list",
            &json!({
                "requestId": list_request_id,
                "scope": "active"
            }),
        )?;
        assert_eq!(listed.request_id, list_request_id);
        assert_eq!(listed.result.len(), 1);
        assert_eq!(listed.result[0].scenario_id, scenario_id);
        assert_eq!(listed.result[0].revision, Revision::INITIAL);

        let open_request_id = RequestId::new(&ids)?;
        let (opened, _) =
            native_summary_and_core_snapshot(&reopened_webview, open_request_id, scenario_id)
                .await?;
        assert_eq!(opened.request_id, open_request_id);
        assert_eq!(opened.current_revision, Some(Revision::INITIAL));
        assert_eq!(opened.result.title, "Persisted clinic roster");
        assert_eq!(opened.result.structure.entities, 0);

        for (command, request) in [
            (
                "project_import_apply",
                json!({
                    "requestId": RequestId::new(&ids)?,
                    "previewId": RequestId::new(&ids)?,
                    "collisionPlan": {
                        "scenarios": {},
                        "supplementalChoices": [{
                            "section": "preferences",
                            "key": "view.json",
                            "action": "skip"
                        }]
                    }
                }),
            ),
            (
                "project_restore_apply",
                json!({
                    "requestId": RequestId::new(&ids)?,
                    "previewId": RequestId::new(&ids)?,
                    "collisionPlan": {"scenarios": {}, "supplementalChoices": []},
                    "authorization": {
                        "destructiveActionConfirmed": true,
                        "safetyBackupBypassPhrase": null
                    }
                }),
            ),
        ] {
            let response = invoke_ipc(&reopened_webview, command, &request)?;
            let error: ApiErrorDto = match response {
                Ok(_) => {
                    return Err(format!("{command} unexpectedly accepted a missing preview").into());
                }
                Err(error) => serde_json::from_value(error)?,
            };
            assert_eq!(error.category, ApiErrorCategoryDto::Protocol);
            assert_eq!(error.code, "portable.preview_not_found");
        }

        let entity_id = EntityId::new(&ids)?;
        let mutation_request_id = RequestId::new(&ids)?;
        let committed: ApiResponseDto<CommandResult> = invoke_ok(
            &reopened_webview,
            "scenario_apply_command",
            &json!({
                "requestId": mutation_request_id,
                "commandId": CommandId::new(&ids)?,
                "scenarioId": scenario_id,
                "expectedRevision": Revision::INITIAL,
                "actor": {
                    "actorId": "desktop.test",
                    "displayName": "Desktop Test"
                },
                "command": {
                    "type": "addEntity",
                    "payload": {
                        "entityId": entity_id,
                        "value": {
                            "id": entity_id.to_string(),
                            "name": "Ada"
                        }
                    }
                },
                "truncateRedo": false
            }),
        )?;
        assert_eq!(committed.request_id, mutation_request_id);
        assert_eq!(committed.current_revision, Some(Revision::new(1)));
        assert_eq!(committed.result.new_revision, Revision::new(1));

        let (committed_view, committed_snapshot) =
            native_summary_and_core_snapshot(&reopened_webview, RequestId::new(&ids)?, scenario_id)
                .await?;
        assert_eq!(committed_view.result.revision, Revision::new(1));
        assert_eq!(
            committed_snapshot.document.domain.entities.get(&entity_id),
            Some(&json!({"id": entity_id.to_string(), "name": "Ada"}))
        );
        let authoritative_document = committed_snapshot.document;

        let undo_request_id = RequestId::new(&ids)?;
        let undone: ApiResponseDto<CommandResult> = invoke_ok(
            &reopened_webview,
            "scenario_undo",
            &json!({
                "requestId": undo_request_id,
                "scenarioId": scenario_id,
                "expectedRevision": 1
            }),
        )?;
        assert_eq!(undone.request_id, undo_request_id);
        assert_eq!(undone.current_revision, Some(Revision::new(2)));
        assert_eq!(undone.result.new_revision, Revision::new(2));
        let (undone_view, undone_snapshot) =
            native_summary_and_core_snapshot(&reopened_webview, RequestId::new(&ids)?, scenario_id)
                .await?;
        assert_eq!(undone_view.result.structure.entities, 0);
        assert!(
            !undone_snapshot
                .document
                .domain
                .entities
                .contains_key(&entity_id)
        );

        let redo_request_id = RequestId::new(&ids)?;
        let redone: ApiResponseDto<CommandResult> = invoke_ok(
            &reopened_webview,
            "scenario_redo",
            &json!({
                "requestId": redo_request_id,
                "scenarioId": scenario_id,
                "expectedRevision": 2
            }),
        )?;
        assert_eq!(redone.request_id, redo_request_id);
        assert_eq!(redone.current_revision, Some(Revision::new(3)));
        assert_eq!(redone.result.new_revision, Revision::new(3));
        let (redone_view, redone_snapshot) =
            native_summary_and_core_snapshot(&reopened_webview, RequestId::new(&ids)?, scenario_id)
                .await?;
        assert_eq!(redone_view.result.revision, Revision::new(3));
        assert_eq!(
            redone_snapshot.document.domain.entities,
            authoritative_document.domain.entities
        );
        let authoritative_document = redone_snapshot.document;

        let stale_entity_id = EntityId::new(&ids)?;
        let stale_response = invoke_ipc(
            &reopened_webview,
            "scenario_apply_command",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "commandId": CommandId::new(&ids)?,
                "scenarioId": scenario_id,
                "expectedRevision": Revision::INITIAL,
                "actor": {
                    "actorId": "desktop.test",
                    "displayName": "Desktop Test"
                },
                "command": {
                    "type": "addEntity",
                    "payload": {
                        "entityId": stale_entity_id,
                        "value": {
                            "id": stale_entity_id.to_string(),
                            "name": "Grace"
                        }
                    }
                },
                "truncateRedo": false
            }),
        )?;
        let stale_error: ApiErrorDto = match stale_response {
            Ok(_) => return Err("stale mutation unexpectedly committed".into()),
            Err(error) => serde_json::from_value(error)?,
        };
        assert_eq!(stale_error.category, ApiErrorCategoryDto::Conflict);
        assert_eq!(stale_error.code, "scenario.revision_conflict");
        let details = stale_error
            .details
            .ok_or("missing stale conflict details")?;
        assert_eq!(
            details.get("expectedRevision"),
            Some(&SafeDiagnosticValue::Integer(0))
        );
        assert_eq!(
            details.get("currentRevision"),
            Some(&SafeDiagnosticValue::Integer(3))
        );

        let (after_stale, after_stale_snapshot) =
            native_summary_and_core_snapshot(&reopened_webview, RequestId::new(&ids)?, scenario_id)
                .await?;
        assert_eq!(after_stale.current_revision, Some(Revision::new(3)));
        assert_eq!(after_stale.result.revision, Revision::new(3));
        assert_eq!(after_stale_snapshot.document, authoritative_document);
        assert!(
            !after_stale_snapshot
                .document
                .domain
                .entities
                .contains_key(&stale_entity_id)
        );
        let archived: ApiResponseDto<Value> = invoke_ok(
            &reopened_webview,
            "project_archive",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "scenarioId": scenario_id,
                "expectedRevision": 3
            }),
        )?;
        assert_eq!(archived.current_revision, Some(Revision::new(3)));
        let unarchived: ApiResponseDto<Value> = invoke_ok(
            &reopened_webview,
            "project_unarchive",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "scenarioId": scenario_id,
                "expectedRevision": 3
            }),
        )?;
        assert_eq!(unarchived.current_revision, Some(Revision::new(3)));
        let deleted: ApiResponseDto<Value> = invoke_ok(
            &reopened_webview,
            "project_delete",
            &json!({
                "requestId": RequestId::new(&ids)?,
                "scenarioId": scenario_id,
                "expectedRevision": 3
            }),
        )?;
        assert_eq!(deleted.current_revision, None);
        Ok(())
    }
}
