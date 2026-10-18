use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    BackendSupportColumn, BackupAssetSelection, DeferredCapability, EuthetoApp, ExistingSolveRunV1,
    HISTORY_PAGE_MAX_REQUEST_BYTES, HISTORY_PAGE_MAX_RESPONSE_BYTES, HistoryPageRequestV1,
    MAX_SOLVE_RUN_LIST_LIMIT, ProjectScope, SolutionAdoptBaseRequestV1,
    SolutionCancelCounterfactualRequestV1, SolutionCompareRequestV3, SolutionExplainRequestV1,
    SolutionGetCounterfactualRequestV1, SolutionListCounterfactualsRequestV1,
    SolutionListRequestV1, SolutionSelectRequestV1, SolutionStartCounterfactualRequestV1,
    SolutionSummaryRequestV1, SolutionVerifyRequestV1, SolutionViewRequestV1,
    SolveEstimateModelRequestV1, SolveRunListRequestV1, SolverSupportMatrixMetadata,
    StoredSolveFailure, StoredSolveRequest, SupportCell, SupportFeature, SupportFeatureId,
    bounded_json_size,
};
use eutheto_export::{
    ApplicationMetadata, BackupSelectionScope, BundleKind, FixedExclusion, OmittedAssetReason,
    PORTABLE_LIMITS, PortableBackupAssetSelection,
};
use eutheto_import::{
    ImportPreview, MigrationRegistryKind, MigrationSubject, PackMigrationVersionSpace,
    RestoreAuthorization, SafetyBackupEvidence,
};
use eutheto_solver_api::{ProgressSink, SolveProgressEvent};
use eutheto_types::{
    ActorRef, ApiErrorCategoryDto, ApiErrorDto, ApiResponseDto, AppError, BackendId,
    BackendSelection, CancellationToken, CommandBatch, CommandEnvelope, CommandId, CommandResult,
    CommandSource, DomainPackRef, DurationMillis, EventTopic, ExplanationMode, FieldErrorDto,
    FoundationStatus, GapPolicy, Horizon, IanaTimeZone, KnownVerificationAlarmV1,
    KnownVerificationSecondaryCauseCode, OperationControl, OperationInterruption, OverlapPolicy,
    PackId, PreservationPolicy, ProjectListItemV1, ProjectMetadataDto, ReproducibilityMode,
    RequestId, ResourceLimits, ResourceRef, Revision, SafeDiagnosticValue, ScenarioCommand,
    ScenarioId, ScenarioSettings, SolveMode, SolveOptions, SolvePhase, SolveStatus,
    SupplementalIdentity, SupportPreviewDto, SystemClock, SystemIdGenerator, UnitSystem,
    ValidationIssue, ValidationReport, VerificationAlarmTerminalization, VerificationFailure,
    WorkerThreadPolicy, resolve_local_midnight,
};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(feature = "bundled-ortools")]
mod bundled_solver;

#[macro_use]
mod generated_command_catalog;
mod diagnostic_actions;
use diagnostic_actions::{solution_create_diagnostic_copy, solution_export_diagnostic};
mod alternatives;
use alternatives::{alternatives_get_group, alternatives_start};
mod manual_repair;
use manual_repair::{
    solution_create_repair_request, solution_lock_assignment, solution_unlock_assignment,
};
mod native_file;
mod operations;
use operations::{
    OperationClaim, OperationContextV1, OperationPhaseV1, OperationPurposeV1, OperationRegistry,
};
#[macro_use]
mod setup_boundary;
use setup_boundary::{
    operation_cancel, operation_prepare, operation_release, scenario_get_entity,
    scenario_get_rule_catalog, scenario_get_setup_status, scenario_get_summary, scenario_get_view,
    scenario_search_entities, scenario_validate, workforce_apply_reviewed_generation,
};
#[macro_use]
mod people_csv;
use people_csv::{
    CsvCustody, assignment_csv_preview, availability_csv_preview, csv_apply, csv_detect,
    csv_preview_discard, csv_record_sample, csv_report, csv_report_save, csv_source_close,
    csv_source_open, eligibility_csv_preview, eligibility_matrix_preview,
    eligibility_matrix_source_open, people_csv_preview, shift_csv_preview,
};
#[macro_use]
mod settings;
use settings::{
    SettingsCustody, settings_export_nonsecret, settings_get, settings_import_nonsecret,
    settings_reset_section, settings_update,
};
#[macro_use]
mod portable;
use portable::{
    PortableCustody, project_backup_create, project_backup_preview, project_export_create,
    project_export_preview, project_import_apply, project_import_preview, project_operation_cancel,
    project_restore_apply, project_restore_preview, project_unopened_bundle_inspect,
    project_unopened_bundle_reexport,
};
mod recipient_report;
use recipient_report::{solution_share_create, solution_share_preview, solution_share_release};
mod schedule_export;
use schedule_export::{
    solution_export, solution_export_cancel, solution_export_preview, solution_export_release,
};
mod license_inventory;

use generated_command_catalog::REGISTERED_COMMANDS;

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
    },
}

struct PreparedPortableOutput {
    bytes: Vec<u8>,
    sha256: String,
    kind: PreparedPortableKind,
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
    portable: Arc<PortableCustody>,
    operations: Arc<OperationRegistry>,
    csv: Arc<CsvCustody>,
    settings: Arc<SettingsCustody>,
    recipient_reports: Arc<recipient_report::custody::ReportCustody>,
}

impl DesktopState {
    fn new(app: EuthetoApp, cache_dir: PathBuf, backup_dir: PathBuf) -> Self {
        let monotonic_clock: Arc<dyn eutheto_types::MonotonicClock> =
            Arc::new(eutheto_types::SystemMonotonicClock::new());
        let operations = Arc::new(OperationRegistry::new(
            app.clone(),
            Arc::new(SystemClock),
            Arc::clone(&monotonic_clock),
            Arc::new(SystemIdGenerator),
            ["main".to_owned()],
        ));
        let csv = Arc::new(CsvCustody::new(app.clone(), Arc::new(SystemIdGenerator)));
        let settings = Arc::new(SettingsCustody::new(app.clone()));
        let portable = Arc::new(PortableCustody::new(app.clone()));
        let recipient_reports = Arc::new(recipient_report::custody::ReportCustody::new(
            monotonic_clock,
            Arc::new(SystemIdGenerator),
            ["main".to_owned()],
        ));
        Self {
            app,
            cache_dir,
            backup_dir,
            portable,
            operations,
            csv,
            settings,
            recipient_reports,
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

const SOLVE_API_SCHEMA_VERSION: u32 = 2;
const SOLVE_START_REQUEST_SCHEMA_VERSION: u32 = 3;
const SOLVE_REQUEST_MAX_BYTES: usize = 64 * 1024;
const SOLVE_RESPONSE_MAX_BYTES: usize = 512 * 1024;
// Complete domain jobs are bounded to 16 MiB; lossless integer strings and the
// response envelope require additional wire space. Compact lists keep the solve cap.
const COUNTERFACTUAL_RESPONSE_MAX_BYTES: usize = 32 * 1024 * 1024;
const SOLVE_ESTIMATE_REQUEST_MAX_BYTES: usize = 8 * 1024;
const SOLVE_ESTIMATE_RESPONSE_MAX_BYTES: usize = 64 * 1024;
const SOLVE_ESTIMATE_TIME_LIMIT_MILLISECONDS: u64 = 30_000;
const EXPLANATION_RESPONSE_MAX_BYTES: usize = 2 * 1024 * 1024;
const EXPLANATION_TIME_LIMIT_MILLISECONDS: u64 = 30_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolutionExplainOperationRequestV2 {
    schema_version: u32,
    operation_id: eutheto_types::OperationId,
    request_id: RequestId,
    scenario_id: ScenarioId,
    source_revision: Revision,
    request: eutheto_domain_ir::ExplanationRequestV1,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolveEstimateModelRequest {
    request_id: RequestId,
    operation_id: eutheto_types::OperationId,
    schema_version: u32,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolveBackendOptionsRequest {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SolveIntent {
    Optimize,
    Repair,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolveStartRequest {
    request_id: RequestId,
    operation_id: eutheto_types::OperationId,
    schema_version: u32,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    mode: SolveMode,
    intent: SolveIntent,
    #[serde(default)]
    baseline_source: eutheto_core::StoredBaselineSourceV1,
    #[serde(default)]
    maximum_time_milliseconds: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolveCancelRequest {
    cancel_request_id: RequestId,
    operation_id: eutheto_types::OperationId,
    schema_version: u32,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolveGetJobRequest {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    run_id: eutheto_types::SolveRunId,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SolveListRunsRequest {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    revision: Option<Revision>,
    limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveBackendOptionDto {
    backend_id: BackendId,
    backend_version: String,
    adapter_version: String,
    display_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveBackendOptionsDto {
    schema_version: u32,
    scenario_id: ScenarioId,
    revision: Revision,
    options: Vec<SolveBackendOptionDto>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum SolveJobStatusDto {
    Running,
    Optimal,
    Feasible,
    Infeasible,
    Unbounded,
    NoSolutionWithinLimit,
    Cancelled,
    InvalidModel,
    BackendUnavailable,
    BackendFailed,
    VerificationAlarm,
    Interrupted,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveVerifiedCheckpointDto {
    result: eutheto_domain_ir::AcceptedResultRefV1,
    verified_after_milliseconds: DurationMillis,
    accepted_at: eutheto_types::Rfc3339Timestamp,
    checkpoint_checksum: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveJobDto {
    schema_version: u32,
    request_id: RequestId,
    run_id: eutheto_types::SolveRunId,
    scenario_id: ScenarioId,
    scenario_revision: Revision,
    status: SolveJobStatusDto,
    started_at: eutheto_types::Rfc3339Timestamp,
    finished_at: Option<eutheto_types::Rfc3339Timestamp>,
    solution_id: Option<eutheto_types::SolutionId>,
    verification_checksum: Option<String>,
    checkpoint: Option<SolveVerifiedCheckpointDto>,
    cancel_operation_id: Option<eutheto_types::OperationId>,
    stale: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveDiagnosticsSummaryDto {
    schema_version: u32,
    job: SolveJobDto,
    elapsed_milliseconds: Option<DurationMillis>,
    first_incumbent_milliseconds: Option<DurationMillis>,
    first_verified_feasible_milliseconds: Option<DurationMillis>,
    phase_timings: Option<eutheto_domain_ir::RunPhaseTimingsV1>,
    diagnostic_code: Option<String>,
    manifest_checksum: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveStartDto {
    schema_version: u32,
    current_revision: Revision,
    reused: bool,
    job: SolveJobDto,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveListRunsDto {
    schema_version: u32,
    scenario_id: ScenarioId,
    current_revision: Revision,
    runs: Vec<SolveJobDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveCancelDto {
    schema_version: u32,
    cancel_request_id: RequestId,
    acknowledgement: &'static str,
}

fn decode_solve_request<T: DeserializeOwned>(request: Option<Value>) -> Result<T, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, SOLVE_REQUEST_MAX_BYTES).is_err())
    {
        return Err(boundary_error(
            "solve.request_too_large",
            "The solve request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    setup_boundary::decode(request)
}
fn decode_solve_estimate_request(
    request: Option<Value>,
) -> Result<SolveEstimateModelRequest, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, SOLVE_ESTIMATE_REQUEST_MAX_BYTES).is_err())
    {
        return Err(boundary_error(
            "solve.request_too_large",
            "The model estimate request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    setup_boundary::decode(request)
}

fn require_solve_version(version: u32) -> Result<(), ApiError> {
    if version == SOLVE_API_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(boundary_error(
            "solve.version_unsupported",
            "This solve request version is not supported.",
            Some("/schemaVersion"),
        )
        .into())
    }
}

fn solve_options(
    mode: SolveMode,
    maximum_time_milliseconds: Option<u32>,
) -> Result<SolveOptions, ApiError> {
    let default_milliseconds = match mode {
        SolveMode::Quick => 2_000,
        SolveMode::Balanced => 5_000,
        SolveMode::Deep => 30_000,
        SolveMode::Custom => {
            return Err(boundary_error(
                "solve.mode_unsupported",
                "Choose Quick, Balanced or Deep for desktop optimization.",
                Some("/mode"),
            )
            .into());
        }
    };
    let duration = maximum_time_milliseconds.unwrap_or(default_milliseconds);
    if !(1..=30_000).contains(&duration) {
        return Err(boundary_error(
            "solve.time_limit_invalid",
            "The maximum optimization time must be between 1 and 30000 milliseconds.",
            Some("/maximumTimeMilliseconds"),
        )
        .into());
    }
    let time_limit_milliseconds = DurationMillis::new(u64::from(duration)).map_err(|_| {
        boundary_error(
            "solve.options_invalid",
            "The selected optimization mode has an invalid bounded deadline.",
            Some("/maximumTimeMilliseconds"),
        )
    })?;
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode,
        time_limit_milliseconds,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 1,
        solution_limit: None,
        stop_after_first_feasible: mode == SolveMode::Quick,
        collect_intermediate_solutions: true,
        explanation_mode: ExplanationMode::Standard,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 100_000,
            max_rules: 100_000,
            max_variables: 100_000,
            max_constraints: 500_000,
        },
    })
}

fn solve_job(
    retained: &ExistingSolveRunV1,
    current_revision: Revision,
    checkpoint: Option<&eutheto_domain_ir::VerifiedCheckpoint>,
) -> SolveJobDto {
    let (status, finished_at, solution_id, verification_checksum) = match &retained.manifest {
        None => (SolveJobStatusDto::Running, None, None, None),
        Some(manifest) => match &manifest.outcome {
            eutheto_domain_ir::RunTerminalOutcomeV1::Accepted {
                status,
                solution_id,
                verification_checksum,
                ..
            } => (
                match status {
                    SolveStatus::Optimal => SolveJobStatusDto::Optimal,
                    SolveStatus::Feasible => SolveJobStatusDto::Feasible,
                    _ => SolveJobStatusDto::BackendFailed,
                },
                Some(manifest.finished_at),
                Some(*solution_id),
                Some(verification_checksum.clone()),
            ),
            eutheto_domain_ir::RunTerminalOutcomeV1::NoResult { status } => (
                match status {
                    SolveStatus::Infeasible => SolveJobStatusDto::Infeasible,
                    SolveStatus::Unbounded => SolveJobStatusDto::Unbounded,
                    SolveStatus::NoSolutionWithinLimit => SolveJobStatusDto::NoSolutionWithinLimit,
                    SolveStatus::Cancelled => SolveJobStatusDto::Cancelled,
                    SolveStatus::InvalidModel => SolveJobStatusDto::InvalidModel,
                    SolveStatus::BackendUnavailable => SolveJobStatusDto::BackendUnavailable,
                    SolveStatus::BackendFailed => SolveJobStatusDto::BackendFailed,
                    SolveStatus::Optimal | SolveStatus::Feasible => {
                        SolveJobStatusDto::BackendFailed
                    }
                },
                Some(manifest.finished_at),
                None,
                None,
            ),
            eutheto_domain_ir::RunTerminalOutcomeV1::VerificationAlarm { .. } => (
                SolveJobStatusDto::VerificationAlarm,
                Some(manifest.finished_at),
                None,
                None,
            ),
            eutheto_domain_ir::RunTerminalOutcomeV1::Interrupted => (
                SolveJobStatusDto::Interrupted,
                Some(manifest.finished_at),
                None,
                None,
            ),
        },
    };
    SolveJobDto {
        schema_version: SOLVE_API_SCHEMA_VERSION,
        request_id: retained.input.request_id,
        run_id: retained.input.run_id,
        scenario_id: retained.input.scenario_id,
        scenario_revision: Revision::new(retained.input.scenario_revision),
        status,
        started_at: retained.started_at,
        finished_at,
        solution_id,
        checkpoint: checkpoint.map(|checkpoint| SolveVerifiedCheckpointDto {
            result: eutheto_domain_ir::AcceptedResultRefV1 {
                solution_id: checkpoint.accepted_result.solution.solution_id,
                result_checksum: checkpoint.accepted_result.checksum.clone(),
            },
            verified_after_milliseconds: checkpoint.verified_after_milliseconds,
            accepted_at: checkpoint.accepted_at,
            checkpoint_checksum: checkpoint.checksum.clone(),
        }),
        cancel_operation_id: None,
        verification_checksum,
        stale: retained.input.scenario_revision != current_revision.value(),
    }
}

async fn durable_solve_job(
    app: &EuthetoApp,
    retained: &ExistingSolveRunV1,
    current_revision: Revision,
) -> Result<SolveJobDto, ApiError> {
    let checkpoint = app
        .solve_get_verified_checkpoint(retained.input.scenario_id, retained.input.run_id)
        .await
        .map_err(map_app_error)?;
    Ok(solve_job(retained, current_revision, checkpoint.as_ref()))
}

fn attach_live_solve_owner(
    operations: &OperationRegistry,
    window_label: &str,
    job: &mut SolveJobDto,
) -> Result<(), ApiError> {
    if matches!(job.status, SolveJobStatusDto::Running) {
        job.cancel_operation_id = operations.active_solve_operation(
            window_label,
            job.request_id,
            &OperationContextV1::Scenario {
                scenario_id: job.scenario_id,
                expected_revision: Some(job.scenario_revision),
            },
        )?;
    }
    Ok(())
}

fn verification_diagnostic_id(error: &AppError) -> Option<RequestId> {
    match error {
        AppError::Verification(failure) => failure.diagnostic_id,
        _ => None,
    }
}

fn known_secondary_cause(error: &AppError) -> Option<KnownVerificationSecondaryCauseCode> {
    let code = match error {
        AppError::Storage(failure) => Some(failure.code.as_str()),
        AppError::Protocol(failure) => Some(failure.code.as_str()),
        _ => None,
    }?;
    KnownVerificationSecondaryCauseCode::from_safe_code(code)
}

fn alarm_manifest_matches(
    alarm: &KnownVerificationAlarmV1,
    manifest: &eutheto_domain_ir::RunManifest,
) -> bool {
    manifest.run_id == alarm.run_id()
        && manifest.run_input_checksum == alarm.input_checksum()
        && matches!(
            &manifest.outcome,
            eutheto_domain_ir::RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }
                if diagnostic_code == alarm.diagnostic_code().as_str()
        )
}

fn known_alarm_app_error(error: &AppError, alarm: KnownVerificationAlarmV1) -> AppError {
    AppError::Verification(VerificationFailure::from_known_alarm(
        alarm,
        verification_diagnostic_id(error),
    ))
}

fn map_stored_solve_failure(failure: StoredSolveFailure) -> ApiErrorDto {
    match failure {
        StoredSolveFailure::Preparation(failure) => map_app_error(failure.error),
        StoredSolveFailure::BeforeStart(error) => map_app_error(error),
        StoredSolveFailure::AfterStart {
            run_id,
            error,
            cleanup,
            known_alarm,
        } => {
            if let Some(alarm) = known_alarm {
                let terminalization = if run_id == alarm.run_id()
                    && cleanup
                        .as_ref()
                        .is_ok_and(|manifest| alarm_manifest_matches(&alarm, manifest))
                {
                    VerificationAlarmTerminalization::Confirmed
                } else {
                    VerificationAlarmTerminalization::Unconfirmed
                };
                let secondary = cleanup
                    .as_ref()
                    .err()
                    .and_then(known_secondary_cause)
                    .or_else(|| known_secondary_cause(&error))
                    .or_else(|| alarm.secondary_cause_code());
                return map_app_error(known_alarm_app_error(
                    &error,
                    alarm
                        .with_terminalization(terminalization)
                        .with_secondary_cause(secondary),
                ));
            }
            map_app_error(error)
        }
        StoredSolveFailure::FinalizationUnconfirmed {
            run_id: _,
            result_id: _,
            error,
            known_alarm,
        } => {
            if let Some(alarm) = known_alarm {
                return map_app_error(known_alarm_app_error(
                    &error,
                    alarm.with_terminalization(VerificationAlarmTerminalization::Unconfirmed),
                ));
            }
            map_app_error(error)
        }
    }
}

fn should_publish_progress(
    last: Option<(SolvePhase, std::time::Instant)>,
    phase: SolvePhase,
    now: std::time::Instant,
) -> bool {
    !last.is_some_and(|(prior, at)| {
        prior == phase && now.duration_since(at) < std::time::Duration::from_millis(300)
    })
}

struct SolveProgressForwarder {
    app: EuthetoApp,
    request_id: RequestId,
    scenario_id: ScenarioId,
    revision: Revision,
    run_id: Arc<std::sync::Mutex<Option<eutheto_types::SolveRunId>>>,
    last: Option<(SolvePhase, std::time::Instant)>,
}

impl SolveProgressForwarder {
    fn publish(&mut self, phase: SolvePhase, percent: Option<u8>) {
        let run_id = {
            let Ok(guard) = self.run_id.lock() else {
                return;
            };
            let Some(run_id) = *guard else {
                return;
            };
            run_id
        };
        let now = std::time::Instant::now();
        if !should_publish_progress(self.last, phase, now) {
            return;
        }
        self.app.publish_solve_progress(
            self.request_id,
            self.scenario_id,
            self.revision,
            run_id,
            phase,
            percent,
        );
        self.last = Some((phase, now));
    }
}

impl ProgressSink for SolveProgressForwarder {
    // Finite progress values are clamped to [0, 100] before fractional truncation.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), eutheto_solver_api::OutputError> {
        match event {
            SolveProgressEvent::Queued => self.publish(SolvePhase::Queued, None),
            SolveProgressEvent::Compiling { percent, .. } => self.publish(
                SolvePhase::Compiling,
                percent.and_then(|value| value.is_finite().then(|| value.clamp(0.0, 100.0) as u8)),
            ),
            SolveProgressEvent::BackendStarted { .. }
            | SolveProgressEvent::PresolveSummary(_)
            | SolveProgressEvent::IncumbentFound(_)
            | SolveProgressEvent::BoundImproved(_)
            | SolveProgressEvent::LogLine(_) => self.publish(SolvePhase::Solving, None),
            SolveProgressEvent::Verifying => self.publish(SolvePhase::Verifying, None),
            SolveProgressEvent::Explaining => self.publish(SolvePhase::Explaining, None),
            SolveProgressEvent::Completed(_) => {}
        }
        Ok(())
    }
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
    schema_version: u32,
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
struct ProjectOpenRequest {
    schema_version: u32,
    request_id: RequestId,
    scenario_id: ScenarioId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateProjectRequest {
    request_id: RequestId,
    schema_version: u32,
    title: String,
    description: String,
    domain_pack: DomainPackRef,
    settings: CalendarSettingsRequest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CalendarSettingsRequest {
    time_zone: String,
    locale: String,
    units: UnitSystem,
    first_date: String,
    last_date: String,
    gap_policy: GapPolicy,
    overlap_policy: OverlapPolicy,
}

impl CalendarSettingsRequest {
    fn into_settings(self) -> Result<ScenarioSettings, ApiError> {
        let time_zone: IanaTimeZone = self.time_zone.parse().map_err(|_| {
            boundary_error(
                "project.time_zone_invalid",
                "Choose a valid IANA time zone.",
                Some("/settings/timeZone"),
            )
        })?;
        let locale = self.locale.parse().map_err(|_| {
            boundary_error(
                "project.locale_invalid",
                "Enter a valid locale tag.",
                Some("/settings/locale"),
            )
        })?;
        let first = self.first_date.parse().map_err(|_| {
            boundary_error(
                "project.date_invalid",
                "Enter a valid first date.",
                Some("/settings/firstDate"),
            )
        })?;
        let last = self.last_date.parse().map_err(|_| {
            boundary_error(
                "project.date_invalid",
                "Enter a valid last date.",
                Some("/settings/lastDate"),
            )
        })?;
        let start = resolve_local_midnight(first, &time_zone, self.gap_policy, self.overlap_policy)
            .map_err(|_| {
                boundary_error(
                    "project.midnight_invalid",
                    "The first date has no exact midnight under the selected time-zone policies.",
                    Some("/settings/firstDate"),
                )
            })?;
        if first > last {
            return Err(boundary_error(
                "project.date_range_invalid",
                "The last date must not precede the first date.",
                Some("/settings/lastDate"),
            )
            .into());
        }
        let end_exclusive = last.tomorrow().map_err(|_| {
            boundary_error(
                "project.date_overflow",
                "The last date cannot form a supported exclusive end.",
                Some("/settings/lastDate"),
            )
        })?;
        let end = resolve_local_midnight(end_exclusive, &time_zone, self.gap_policy, self.overlap_policy)
            .map_err(|_| boundary_error(
                "project.midnight_invalid",
                "The date after the last date has no exact midnight under the selected time-zone policies.",
                Some("/settings/lastDate"),
            ))?;
        let horizon = Horizon::new(start, end).map_err(|_| {
            boundary_error(
                "project.date_range_invalid",
                "The dates must form a supported nonempty planning horizon.",
                Some("/settings/lastDate"),
            )
        })?;
        Ok(ScenarioSettings {
            time_zone,
            locale,
            units: self.units,
            horizon,
            gap_policy: self.gap_policy,
            overlap_policy: self.overlap_policy,
        })
    }
}

fn decode_project_request<T: DeserializeOwned>(request: Option<Value>) -> Result<T, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, 64 * 1024).is_err())
    {
        return Err(boundary_error(
            "project.request_too_large",
            "The project request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    setup_boundary::decode(request)
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
    schema_version: u32,
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
    schema_version: u32,
    library_revision: Revision,
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
    schema_version: u32,
    library_revision: Revision,
    safety_backup: SafetyBackupOutcomeDto,
    scenario_ids: Vec<ScenarioId>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum SafetyBackupOutcomeDto {
    NotRequired,
    CreatedAndVerified { artifact_name: String },
    ConfirmedBypass,
}

impl From<eutheto_core::SafetyBackupOutcome> for SafetyBackupOutcomeDto {
    fn from(outcome: eutheto_core::SafetyBackupOutcome) -> Self {
        match outcome {
            eutheto_core::SafetyBackupOutcome::NotRequired => Self::NotRequired,
            eutheto_core::SafetyBackupOutcome::CreatedAndVerified { artifact_name } => {
                Self::CreatedAndVerified { artifact_name }
            }
            eutheto_core::SafetyBackupOutcome::ConfirmedBypass => Self::ConfirmedBypass,
        }
    }
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
    schema_version: u32,
    title: String,
    byte_length: usize,
    backup_summary: Option<BackupSummaryDto>,
    preview_id: RequestId,
    digest: String,
    current_revision: Option<Revision>,
    library_revision: Revision,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableArtifactDto {
    schema_version: u32,
    current_revision: Option<Revision>,
    library_revision: Option<Revision>,
    artifact_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioCommandCatalogDto {
    command_types: &'static [&'static str],
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

struct LimitedResponseWriter<'a> {
    bytes: Vec<u8>,
    maximum: Option<usize>,
    exceeded: bool,
    control: Option<&'a OperationControl>,
    interrupted: Option<OperationInterruption>,
}

impl std::io::Write for LimitedResponseWriter<'_> {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if let Some(control) = self.control
            && let Err(reason) = control.check()
        {
            self.interrupted = Some(reason);
            return Err(std::io::Error::other("solution response was interrupted"));
        }
        if self.maximum.is_some_and(|maximum| {
            self.bytes
                .len()
                .checked_add(input.len())
                .is_none_or(|length| length > maximum)
        }) {
            self.exceeded = true;
            return Err(std::io::Error::other("solution response exceeds its limit"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn solution_response<T: Serialize>(
    request_id: RequestId,
    current_revision: Revision,
    result: T,
) -> SolutionApiResult {
    solution_response_bounded(request_id, current_revision, result, None)
}

fn solution_response_bounded<T: Serialize>(
    request_id: RequestId,
    current_revision: Revision,
    result: T,
    max_bytes: Option<usize>,
) -> SolutionApiResult {
    solution_response_controlled(request_id, current_revision, result, max_bytes, None)
}

fn solution_response_controlled<T: Serialize>(
    request_id: RequestId,
    current_revision: Revision,
    result: T,
    max_bytes: Option<usize>,
    control: Option<&OperationControl>,
) -> SolutionApiResult {
    let envelope = response(request_id, Some(current_revision), Vec::new(), result);
    let mut writer = LimitedResponseWriter {
        bytes: Vec::new(),
        maximum: max_bytes,
        exceeded: false,
        control,
        interrupted: None,
    };
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut writer, JavascriptIntegerFormatter);
    let serialization = envelope.serialize(&mut serializer);
    if let Some(reason) = writer
        .interrupted
        .or_else(|| control.and_then(|control| control.check().err()))
    {
        return Err(match reason {
            OperationInterruption::Cancelled => operations::cancelled(),
            OperationInterruption::DeadlineExceeded => operations::deadline_exceeded(),
        }
        .into());
    }
    if writer.exceeded {
        return Err(boundary_error(
            "solution.response_too_large",
            "The solution response exceeds its native admission limit.",
            None,
        )
        .into());
    }
    serialization.map_err(|_| -> ApiError {
        boundary_error(
            "protocol.response_serialization_failed",
            "The solution response could not be translated safely.",
            None,
        )
        .into()
    })?;
    let json = String::from_utf8(writer.bytes).map_err(|_| -> ApiError {
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
    let request = request.ok_or_else(|| Box::new(invalid_solution_request()))?;
    if bounded_json_size(&request, SOLVE_REQUEST_MAX_BYTES).is_err() {
        return Err(boundary_error(
            "solution.request_too_large",
            "The solution request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    decode_solution_value(request)
}

// Leave room for fixed invoke keys and the Tauri channel identifier.
const OPERATION_REQUEST_ENVELOPE_OVERHEAD_BYTES: usize = 1024;

struct NativeOperationRequest(Option<Value>);

fn operation_request_error(command: &str, too_large: bool) -> ApiErrorDto {
    match (command, too_large) {
        ("solution_explain", true) => boundary_error(
            "solution.request_too_large",
            "The solution request exceeds its native admission limit.",
            None,
        ),
        ("solution_create_diagnostic_copy" | "solution_export_diagnostic", true) => boundary_error(
            "diagnostic_action.request_too_large",
            "The diagnostic action request exceeds its native admission limit.",
            None,
        ),
        ("solution_share_preview" | "solution_share_create" | "solution_share_release", true) => {
            boundary_error(
                "recipient.request_too_large",
                "The recipient request exceeds its native admission limit.",
                None,
            )
        }
        (
            "solution_export_preview"
            | "solution_export"
            | "solution_export_cancel"
            | "solution_export_release",
            true,
        ) => boundary_error(
            "schedule_export.request_too_large",
            "The schedule export request exceeds its native admission limit.",
            None,
        ),
        ("solution_create_diagnostic_copy" | "solution_export_diagnostic", false) => {
            boundary_error(
                "setup.invalid_request",
                "The setup request does not match its supported contract.",
                None,
            )
        }
        _ => invalid_solution_request(),
    }
}

impl<'de, R: tauri::Runtime> tauri::ipc::CommandArg<'de, R> for NativeOperationRequest {
    fn from_command(
        command: tauri::ipc::CommandItem<'de, R>,
    ) -> Result<Self, tauri::ipc::InvokeError> {
        let maximum = match command.name {
            "solution_explain" => SOLVE_REQUEST_MAX_BYTES,
            "solution_create_diagnostic_copy" | "solution_export_diagnostic" => {
                diagnostic_actions::REQUEST_MAX_BYTES
            }
            "solution_share_preview" | "solution_share_create" | "solution_share_release" => {
                recipient_report::FRAME_BYTES
            }
            "solution_export_preview"
            | "solution_export"
            | "solution_export_cancel"
            | "solution_export_release" => schedule_export::FRAME_BYTES,
            _ => return Err(operation_request_error(command.name, false).into()),
        };
        let tauri::ipc::InvokeBody::Json(arguments) = command.message.payload() else {
            return Err(operation_request_error(command.name, false).into());
        };
        let request = arguments.get(command.key).filter(|value| !value.is_null());
        if request.is_some_and(|value| bounded_json_size(value, maximum).is_err())
            || bounded_json_size(
                arguments,
                maximum + OPERATION_REQUEST_ENVELOPE_OVERHEAD_BYTES,
            )
            .is_err()
        {
            return Err(operation_request_error(command.name, true).into());
        }
        Ok(Self(request.cloned()))
    }
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
    if bounded_json_size(&request, SOLVE_REQUEST_MAX_BYTES).is_err() {
        return Err(boundary_error(
            "solution.request_too_large",
            "The solution request exceeds its native admission limit.",
            None,
        )
        .into());
    }
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
fn known_alarm_details(alarm: &KnownVerificationAlarmV1) -> BTreeMap<String, SafeDiagnosticValue> {
    let mut details = BTreeMap::from([
        (
            "runId".to_owned(),
            SafeDiagnosticValue::Text(alarm.run_id().to_string()),
        ),
        (
            "inputChecksum".to_owned(),
            SafeDiagnosticValue::Text(alarm.input_checksum().to_owned()),
        ),
        (
            "diagnosticCode".to_owned(),
            SafeDiagnosticValue::Text(alarm.diagnostic_code().as_str().to_owned()),
        ),
        (
            "terminalization".to_owned(),
            SafeDiagnosticValue::Text(alarm.terminalization().as_str().to_owned()),
        ),
    ]);
    if let Some(code) = alarm.secondary_cause_code() {
        details.insert(
            "secondaryCauseCode".to_owned(),
            SafeDiagnosticValue::Text(code.as_str().to_owned()),
        );
    }
    details
}

fn known_verification_error(failure: VerificationFailure) -> ApiErrorDto {
    let Some(alarm) = failure.known_alarm else {
        return failure_error(
            failure.code,
            failure.message,
            ApiErrorCategoryDto::Verification,
            failure.retryable,
            failure.diagnostic_id,
        );
    };
    ApiErrorDto {
        code: alarm.diagnostic_code().as_str().to_owned(),
        message: VerificationFailure::KNOWN_ALARM_MESSAGE.to_owned(),
        category: ApiErrorCategoryDto::Verification,
        retryable: false,
        field_errors: Vec::new(),
        details: Some(known_alarm_details(&alarm)),
        diagnostic_id: failure.diagnostic_id,
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
        AppError::Verification(failure) => known_verification_error(failure),
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
            "The file operation was cancelled.",
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
                "The native file operation could not be completed.",
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

fn read_bounded_portable(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, NativeFileError> {
    let maximum = usize::try_from(PORTABLE_LIMITS.max_archive_bytes)
        .map_err(|_| NativeFileError::TooLarge)?;
    native_file::read_bounded_file(path, maximum, cancellation)
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
        schema_version: 1,
        library_revision: preview.binding.local_library_revision,
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
) -> ApiResult<Vec<ProjectListItemV1>> {
    operations::require_version(request.schema_version)?;
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

// The explicit implemented-command catalog is intentionally kept together.
#[allow(clippy::too_many_lines)]
#[tauri::command]
fn app_get_capabilities(request: RequestOnly) -> ApiResponseDto<AppCapabilitiesDto> {
    const AVAILABLE: &[&str] = &[
        "app_get_info",
        "app_get_capabilities",
        "app_get_paths_summary",
        "app_get_license_inventory",
        "app_create_support_bundle_preview",
        "pack_list",
        "pack_describe",
        "solver_list",
        "solver_describe",
        "solver_get_support_matrix",
        "solver_get_deferred_gates",
        "project_list",
        "project_open",
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
        "csv_source_open",
        "eligibility_matrix_source_open",
        "csv_source_close",
        "csv_detect",
        "csv_record_sample",
        "people_csv_preview",
        "assignment_csv_preview",
        "eligibility_csv_preview",
        "eligibility_matrix_preview",
        "shift_csv_preview",
        "availability_csv_preview",
        "csv_apply",
        "csv_preview_discard",
        "csv_report",
        "csv_report_save",
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
        "scenario_get_history_page",
        "solution_list",
        "solution_get_summary",
        "solution_get_view",
        "solution_select",
        "solution_adopt_base",
        "solution_verify",
        "solution_compare",
        "solution_create_diagnostic_copy",
        "solution_export_diagnostic",
        "solution_share_preview",
        "solution_share_create",
        "solution_share_release",
        "solution_export_preview",
        "solution_export",
        "solution_export_cancel",
        "solution_export_release",
        "solution_start_counterfactual",
        "solution_cancel_counterfactual",
        "solution_get_counterfactual",
        "solution_list_counterfactuals",
        "solution_lock_assignment",
        "solution_unlock_assignment",
        "solution_create_repair_request",
        "solve_estimate_model",
        "solve_cancel",
        "solve_get_job",
        "alternatives_get_group",
        "solve_list_runs",
        "solve_get_diagnostics_summary",
        "settings_get",
        "settings_update",
        "settings_reset_section",
        "settings_export_nonsecret",
        "settings_import_nonsecret",
    ];
    let mut available_commands = AVAILABLE.to_vec();
    if cfg!(feature = "bundled-ortools") {
        available_commands.extend([
            "solve_get_backend_options",
            "solve_start",
            "alternatives_start",
        ]);
    }
    let unavailable_commands = REGISTERED_COMMANDS
        .iter()
        .copied()
        .filter(|command| !available_commands.contains(command))
        .collect();
    response(
        request.request_id,
        None,
        Vec::new(),
        AppCapabilitiesDto {
            available_commands,
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
async fn app_get_license_inventory(request: RequestOnly) -> SolutionApiResult {
    let inventory = tauri::async_runtime::spawn_blocking(license_inventory::read_inventory)
        .await
        .map_err(|_| {
            boundary_error(
                "license_inventory.invalid",
                "The build-owned license inventory could not be read safely.",
                None,
            )
        })??;
    setup_boundary::encode_response(
        request.request_id,
        None,
        inventory,
        license_inventory::MAX_LICENSE_INVENTORY_COMPACT_BYTES,
        None,
    )
    .await
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
    request: Option<Value>,
) -> ApiResult<Vec<ProjectListItemV1>> {
    project_list_impl(&state, decode_project_request(request)?).await
}

#[tauri::command]
async fn project_open(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<ProjectListItemV1> {
    let request: ProjectOpenRequest = decode_project_request(request)?;
    operations::require_version(request.schema_version)?;
    let project = state
        .app
        .open_project_metadata(request.scenario_id)
        .await
        .map_err(map_app_error)?;
    Ok(response(
        request.request_id,
        Some(project.revision),
        Vec::new(),
        project,
    ))
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
    request: Option<Value>,
) -> ApiResult<ProjectMetadataDto> {
    let request: CreateProjectRequest = decode_project_request(request)?;
    operations::require_version(request.schema_version)?;
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::CreateProject {
            request_id,
            title: request.title,
            description: request.description,
            domain_pack: request.domain_pack,
            settings: request.settings.into_settings()?,
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

fn decode_history_request(
    request: Option<Value>,
) -> Result<CorrelatedRequest<HistoryPageRequestV1>, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, HISTORY_PAGE_MAX_REQUEST_BYTES).is_err())
    {
        return Err(boundary_error(
            "history.request_too_large",
            "The history request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    let request: CorrelatedRequest<HistoryPageRequestV1> = request
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            Box::new(boundary_error(
                "history.request_invalid",
                "The history request is missing or malformed.",
                None,
            ))
        })?;
    request.operation.validate().map_err(map_app_error)?;
    Ok(request)
}

#[tauri::command]
async fn scenario_get_history_page(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request = decode_history_request(request)?;
    let AppQueryResult::HistoryPage(page) = state
        .app
        .query(AppQuery::HistoryPage(request.operation))
        .await
        .map_err(map_app_error)?
    else {
        return Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected history result.",
            None,
        )
        .into());
    };
    tauri::async_runtime::spawn_blocking(move || {
        setup_boundary::encode(
            &response(request.request_id, Some(page.revision), Vec::new(), page),
            HISTORY_PAGE_MAX_RESPONSE_BYTES,
            None,
        )
    })
    .await
    .map_err(|_| {
        Box::new(boundary_error(
            "history.response_failed",
            "The history response could not be prepared safely.",
            None,
        ))
    })?
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
                let cancellation = execution.cancellation();
                tauri::async_runtime::spawn_blocking(move || {
                    setup_boundary::encode(
                        &response(
                            request_id,
                            Some(result.current_revision),
                            Vec::new(),
                            result,
                        ),
                        SOLVE_RESPONSE_MAX_BYTES,
                        Some(cancellation),
                    )
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
async fn solution_adopt_base(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<CommandResult> {
    let request: SolutionAdoptBaseRequestV1 = decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .execute(AppCommand::AdoptBaseSchedule(request))
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
            "The application returned an unexpected base-adoption result.",
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
    let request: CorrelatedRequest<SolutionCompareRequestV3> = decode_solution_request(request)?;
    let scenario_id = request.operation.scenario_id;
    let base_id = request.operation.base_solution_id;
    let candidate_id = request.operation.candidate_solution_id;
    let result = state
        .app
        .query(AppQuery::SolutionCompareV3(request.operation))
        .await
        .map_err(map_app_error)?;
    match result {
        AppQueryResult::SolutionComparisonV3(result)
            if result.schema_version == eutheto_core::SOLUTION_COMPARE_SCHEMA_VERSION
                && result.scenario_id == scenario_id
                && result.comparison.base.scenario_id == scenario_id
                && result.comparison.candidate.scenario_id == scenario_id
                && result.comparison.base.accepted_result.solution_id == base_id
                && result.comparison.candidate.accepted_result.solution_id == candidate_id =>
        {
            let result = *result;
            solution_response_bounded(
                request.request_id,
                result.current_revision,
                result,
                Some(SOLVE_RESPONSE_MAX_BYTES),
            )
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected solution-comparison result.",
            None,
        )
        .into()),
    }
}

// Keep the seven closed subject cases together so native source admission remains auditable.
#[allow(clippy::too_many_lines)]
fn validate_solution_explanation_source(
    scenario_id: ScenarioId,
    source_revision: Revision,
    subject: &eutheto_domain_ir::ExplanationRequestSubjectV1,
    result: &eutheto_core::SolutionExplanationDtoV2,
) -> Result<(), ApiError> {
    use eutheto_domain_ir::{
        ExplanationEvidencePayloadV2 as Payload, ExplanationRequestSubjectV1 as Subject,
    };
    let selected_revision = source_revision.value();
    let single_revision = result.scenario_revisions.as_slice() == [source_revision];
    let comparison_matches = |comparison: &eutheto_domain_ir::SolutionComparisonV2,
                              base: &eutheto_domain_ir::AcceptedResultRefV1,
                              candidate: &eutheto_domain_ir::AcceptedResultRefV1,
                              repair: bool| {
        let revisions = if repair {
            [
                comparison.candidate.scenario_revision,
                comparison.base.scenario_revision,
            ]
        } else {
            [
                comparison.base.scenario_revision,
                comparison.candidate.scenario_revision,
            ]
        };
        comparison.schema_version == eutheto_domain_ir::SOLUTION_COMPARISON_V2_SCHEMA_VERSION
            && comparison.base.scenario_id == scenario_id
            && comparison.candidate.scenario_id == scenario_id
            && &comparison.base.accepted_result == base
            && &comparison.candidate.accepted_result == candidate
            && comparison.candidate.scenario_revision == selected_revision
            && result
                .scenario_revisions
                .iter()
                .map(|revision| revision.value())
                .eq(revisions)
            && [
                &comparison.runs.base.outcome,
                &comparison.runs.candidate.outcome,
            ]
            .into_iter()
            .zip([base, candidate])
            .all(|(outcome, expected)| {
                matches!(
                    outcome,
                    eutheto_domain_ir::RunTerminalOutcomeV1::Accepted {
                        solution_id, accepted_result_checksum, ..
                    } if *solution_id == expected.solution_id
                        && *accepted_result_checksum == expected.result_checksum
                )
            })
    };
    let subject_matches = match (subject, &result.explanation.evidence.evidence) {
        (Subject::Validation { issue_id }, Payload::Validation { issue }) => {
            single_revision
                && issue_id
                    .as_ref()
                    .is_none_or(|expected| expected == &issue.issue_id)
        }
        (Subject::Assignment { assignment_id, .. }, Payload::Assignment { assignment }) => {
            single_revision && assignment.assignment.id == *assignment_id
        }
        (
            Subject::Infeasibility {
                solve_run_id,
                run_manifest_checksum,
                conflict_id,
            },
            Payload::Infeasibility { infeasibility, .. },
        ) => {
            single_revision
                && match &result.infeasibility_context {
                    Some(context) => {
                        context.run_id == *solve_run_id
                            && context.run_manifest_checksum == *run_manifest_checksum
                            && context.source_revision == source_revision
                            && context.conflict_id.as_str().strip_prefix("infeasibility.")
                                == Some(context.evidence_checksum.as_str())
                            && conflict_id
                                .as_ref()
                                .is_none_or(|expected| expected == &context.conflict_id)
                    }
                    None => {
                        conflict_id.is_none()
                            && matches!(
                                infeasibility,
                                eutheto_domain_ir::InfeasibilityEvidenceV1::Unavailable { .. }
                            )
                    }
                }
        }
        (
            Subject::Counterfactual { job_id, base },
            Payload::Counterfactual {
                result: counterfactual,
            },
        ) => {
            let semantics = &counterfactual.request.semantics;
            single_revision
                && counterfactual.request.job_id == *job_id
                && &semantics.base == base
                && semantics.scenario_id == scenario_id
                && semantics.scenario_revision == selected_revision
                && counterfactual.base_run_input.scenario_id == scenario_id
                && counterfactual.base_run_input.scenario_revision == selected_revision
                && counterfactual.run_input.scenario_id == scenario_id
                && counterfactual.run_input.scenario_revision == selected_revision
                && counterfactual.base_run_manifest.run_id == semantics.base_run_id
                && counterfactual.base_run_input.run_id == semantics.base_run_id
        }
        (
            Subject::SolutionDifference { left, right },
            Payload::SolutionDifference { comparison },
        ) => comparison_matches(comparison, left, right, false),
        (Subject::Repair { current, base }, Payload::Repair { repair }) => {
            comparison_matches(&repair.comparison, base, current, true)
        }
        (
            Subject::OptimalityStatus {
                solve_run_id,
                run_manifest_checksum,
                result: expected,
            },
            Payload::OptimalityStatus { status },
        ) => {
            let certainty = if matches!(
                status.run_manifest.outcome,
                eutheto_domain_ir::RunTerminalOutcomeV1::Accepted { .. }
            ) {
                eutheto_domain_ir::ExplanationCertainty::IndependentlyVerified
            } else {
                eutheto_domain_ir::ExplanationCertainty::Deterministic
            };
            single_revision
                && status.schema_version == Some(2)
                && result.explanation.evidence.certainty == certainty
                && status.run_input.scenario_id == scenario_id
                && status.run_input.scenario_revision == selected_revision
                && status.run_input.run_id == *solve_run_id
                && status.run_manifest.run_id == *solve_run_id
                && status.run_manifest.checksum == *run_manifest_checksum
                && status.result.as_ref() == expected.as_ref()
        }
        _ => false,
    };
    let source_matches = result.schema_version
        == eutheto_domain_ir::EXPLANATION_RESULT_V2_SCHEMA_VERSION
        && result.explanation.schema_version
            == eutheto_domain_ir::EXPLANATION_RESULT_V2_SCHEMA_VERSION
        && result.explanation.evidence.schema_version
            == eutheto_domain_ir::EXPLANATION_EVIDENCE_V2_SCHEMA_VERSION
        && result.explanation.rendered.schema_version
            == eutheto_domain_ir::EVIDENCE_RENDER_RESULT_SCHEMA_VERSION
        && result.scenario_id == scenario_id
        && result.current_revision >= source_revision
        && result.explanation.evidence.evidence.kind() == subject.kind()
        && result.explanation.rendered.kind == subject.kind()
        && result.explanation.rendered.evidence_checksum == result.explanation.evidence.checksum
        && (matches!(subject, Subject::Infeasibility { .. })
            || result.infeasibility_context.is_none());
    if source_matches && subject_matches {
        Ok(())
    } else {
        Err(boundary_error(
            "protocol.result_mismatch",
            "The native explanation result does not match its admitted subject and source.",
            None,
        )
        .into())
    }
}

#[tauri::command]
async fn solution_explain<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: SolutionExplainOperationRequestV2 = decode_solution_value(
        request
            .0
            .ok_or_else(|| Box::new(invalid_solution_request()))?,
    )?;
    require_solve_version(request.schema_version)?;
    let time_limit = DurationMillis::new(EXPLANATION_TIME_LIMIT_MILLISECONDS)
        .map_err(|_| Box::new(operations::deadline_exceeded()))?;
    let operation = setup_boundary::claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::SolutionExplain,
        request.scenario_id,
        Some(request.source_revision),
    );
    let app = state.app.clone();
    state
        .operations
        .run_with_bounded_budget(
            window.label(),
            operation,
            Some(setup_boundary::progress(on_progress)),
            OperationPhaseV1::Validating,
            (request, None),
            time_limit,
            move |request, mut execution| async move {
                let (budget, _) = execution.take_solve_admission().ok_or_else(|| {
                    Box::new(boundary_error(
                        "operation.budget_missing",
                        "The explanation deadline was not admitted safely.",
                        None,
                    ))
                })?;
                let control = OperationControl::Solve(budget.phase_view());
                let request_id = request.request_id;
                let scenario_id = request.scenario_id;
                let source_revision = request.source_revision;
                let subject = request.request.subject.clone();
                let result = app
                    .query(AppQuery::SolutionExplain {
                        request: SolutionExplainRequestV1 {
                            schema_version: eutheto_core::SOLUTION_API_SCHEMA_VERSION,
                            scenario_id,
                            request: request.request,
                        },
                        source_revision,
                        control: control.clone(),
                    })
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let AppQueryResult::SolutionExplanation(result) = result else {
                    return Err(Box::new(boundary_error(
                        "protocol.result_mismatch",
                        "The application returned an unexpected solution-explanation result.",
                        None,
                    )));
                };
                let result = *result;
                validate_solution_explanation_source(
                    scenario_id,
                    source_revision,
                    &subject,
                    &result,
                )?;
                execution.preparing_response();
                tauri::async_runtime::spawn_blocking(move || {
                    solution_response_controlled(
                        request_id,
                        result.current_revision,
                        result,
                        Some(EXPLANATION_RESPONSE_MAX_BYTES),
                        Some(&control),
                    )
                })
                .await
                .map_err(|_| {
                    Box::new(boundary_error(
                        "protocol.response_serialization_failed",
                        "The explanation response could not be prepared safely.",
                        None,
                    ))
                })?
            },
        )
        .await
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
            solution_response_bounded(
                request_id,
                current_revision,
                result,
                Some(COUNTERFACTUAL_RESPONSE_MAX_BYTES),
            )
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
            solution_response_bounded(
                request_id,
                current_revision,
                result,
                Some(COUNTERFACTUAL_RESPONSE_MAX_BYTES),
            )
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected counterfactual-cancellation result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_get_counterfactual(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionGetCounterfactualRequestV1> =
        decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionGetCounterfactual(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionGetCounterfactual(result) => {
            let result = *result;
            solution_response_bounded(
                request_id,
                result.current_revision,
                result,
                Some(COUNTERFACTUAL_RESPONSE_MAX_BYTES),
            )
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected counterfactual result.",
            None,
        )
        .into()),
    }
}

#[tauri::command]
async fn solution_list_counterfactuals(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: CorrelatedRequest<SolutionListCounterfactualsRequestV1> =
        decode_solution_request(request)?;
    let request_id = request.request_id;
    match state
        .app
        .query(AppQuery::SolutionListCounterfactuals(request.operation))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::SolutionListCounterfactuals(result) => {
            let result = *result;
            solution_response_bounded(
                request_id,
                result.current_revision,
                result,
                Some(SOLVE_RESPONSE_MAX_BYTES),
            )
        }
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected counterfactual list.",
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
    scenario_migrate_preview => ("capability.scenario_migration_unavailable", "Scenario migration previews"),
);
async fn solve_current_revision(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
) -> Result<Revision, ApiError> {
    match app
        .query(AppQuery::ProjectMetadata(scenario_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Project(project) => Ok(project.revision),
        _ => Err(boundary_error(
            "protocol.result_mismatch",
            "The application returned an unexpected scenario metadata result.",
            None,
        )
        .into()),
    }
}

fn require_solve_revision(current: Revision, expected: Revision) -> Result<(), ApiError> {
    if current == expected {
        Ok(())
    } else {
        Err(Box::new(map_app_error(AppError::Conflict {
            expected_revision: expected,
            actual_revision: current,
        })))
    }
}

fn require_solve_runtime(app: &EuthetoApp) -> Result<(), ApiError> {
    if app.solve_runtime_available() {
        Ok(())
    } else {
        Err(boundary_error(
            "solve.backend_unavailable",
            "No compatible packaged solver backend is installed.",
            None,
        )
        .into())
    }
}

#[tauri::command]
async fn solve_get_backend_options(
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolveBackendOptionsRequest = decode_solve_request(request)?;
    require_solve_version(request.schema_version)?;
    require_solve_runtime(&state.app)?;
    let current = solve_current_revision(&state.app, request.scenario_id).await?;
    require_solve_revision(current, request.expected_revision)?;
    let options = state
        .app
        .solve_backend_descriptors()
        .into_iter()
        .map(|descriptor| SolveBackendOptionDto {
            backend_id: descriptor.id,
            backend_version: descriptor.version,
            adapter_version: descriptor.adapter_version,
            display_name: descriptor.display_name,
        })
        .collect();
    setup_boundary::encode_response(
        request.request_id,
        Some(current),
        SolveBackendOptionsDto {
            schema_version: SOLVE_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            revision: current,
            options,
        },
        SOLVE_RESPONSE_MAX_BYTES,
        None,
    )
    .await
}
#[tauri::command]
async fn solve_estimate_model<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request = decode_solve_estimate_request(request)?;
    require_solve_version(request.schema_version)?;
    let time_limit = DurationMillis::new(SOLVE_ESTIMATE_TIME_LIMIT_MILLISECONDS).map_err(|_| {
        boundary_error(
            "solve.budget_invalid",
            "The model estimate deadline could not be created safely.",
            None,
        )
    })?;
    let context = OperationContextV1::Scenario {
        scenario_id: request.scenario_id,
        expected_revision: Some(request.expected_revision),
    };
    let app = state.app.clone();
    state
        .operations
        .run_with_bounded_budget(
            window.label(),
            OperationClaim {
                operation_id: request.operation_id,
                request_id: request.request_id,
                purpose: Some(OperationPurposeV1::SolveEstimate),
                context,
            },
            Some(setup_boundary::progress(on_progress)),
            OperationPhaseV1::Validating,
            (request, None),
            time_limit,
            move |request, mut execution| async move {
                let (budget, _) = execution.take_solve_admission().ok_or_else(|| {
                    boundary_error(
                        "solve.budget_missing",
                        "The bounded model estimate was not admitted safely.",
                        None,
                    )
                })?;
                let result = app
                    .estimate_stored_model(
                        SolveEstimateModelRequestV1 {
                            scenario_id: request.scenario_id,
                            expected_revision: request.expected_revision,
                        },
                        budget,
                    )
                    .await
                    .map_err(map_app_error)?;
                let current_revision = result.current_revision;
                let cancellation = Some(execution.cancellation());
                setup_boundary::finish(
                    &mut execution,
                    request.request_id,
                    Some(current_revision),
                    result,
                    SOLVE_ESTIMATE_RESPONSE_MAX_BYTES,
                    cancellation,
                )
                .await
            },
        )
        .await
}

#[allow(clippy::too_many_lines, clippy::large_futures)]
#[tauri::command]
async fn solve_start<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolveStartRequest = decode_solve_request(request)?;
    if request.schema_version != SOLVE_START_REQUEST_SCHEMA_VERSION {
        return Err(boundary_error(
            "solve.unsupported_schema",
            "This solve-start request version is not supported.",
            Some("/schemaVersion"),
        )
        .into());
    }
    if matches!(request.intent, SolveIntent::Optimize)
        && request.baseline_source != eutheto_core::StoredBaselineSourceV1::Accepted
    {
        return Err(boundary_error(
            "solve.objective_invalid",
            "Use Repair plan or Alternatives to select an unverified starting plan.",
            Some("/baselineSource"),
        )
        .into());
    }
    let options = solve_options(request.mode, request.maximum_time_milliseconds)?;
    let context = OperationContextV1::Scenario {
        scenario_id: request.scenario_id,
        expected_revision: Some(request.expected_revision),
    };
    let run_id = Arc::new(std::sync::Mutex::new(None));
    let observer_run_id = Arc::clone(&run_id);
    let observer_app = state.app.clone();
    let observer = Arc::new(move |bound: eutheto_types::SolveRunId| {
        if let Ok(mut slot) = observer_run_id.lock() {
            *slot = Some(bound);
        }
        observer_app.publish_solve_progress(
            request.request_id,
            request.scenario_id,
            request.expected_revision,
            bound,
            SolvePhase::Queued,
            None,
        );
    });
    let app = state.app.clone();
    let scenario_id = request.scenario_id;
    let expected_revision = request.expected_revision;
    let request_id = request.request_id;
    let mut stored_request = StoredSolveRequest {
        request_id,
        scenario_id,
        expected_revision: Some(expected_revision),
        options,
        objective_selection: match request.intent {
            SolveIntent::Optimize => None,
            SolveIntent::Repair => Some(eutheto_core::StoredObjectiveSelectionV1 {
                schema_version: 1,
                profile_id: eutheto_domain_ir::ObjectiveProfileId::new(
                    "official.workforce.objective.minimal-changes",
                )
                .map_err(|_| {
                    boundary_error(
                        "solve.objective_invalid",
                        "The native repair objective is invalid.",
                        None,
                    )
                })?,
                semantic_version: 1,
                operation: eutheto_domain_ir::RunObjectiveOperation::Repair,
                baseline_source: request.baseline_source,
                alternatives_capture_id: None,
            }),
        },
    };
    state
        .operations
        .run_with_solve_budget(
            window.label(),
            OperationClaim {
                operation_id: request.operation_id,
                request_id,
                purpose: Some(OperationPurposeV1::SolveStart),
                context,
            },
            None,
            OperationPhaseV1::Validating,
            ((), None),
            stored_request.options.time_limit_milliseconds,
            move |(), mut execution| async move {
                let (budget, started_at) = execution.take_solve_admission().ok_or_else(|| {
                    boundary_error(
                        "solve.budget_missing",
                        "The bounded solve owner was not admitted safely.",
                        None,
                    )
                })?;
                let existing = app
                    .solve_lookup_native_request(&mut stored_request)
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                if let Some(existing) = existing {
                    let current_revision = solve_current_revision(&app, scenario_id).await?;
                    observer(existing.input.run_id);
                    let job = durable_solve_job(&app, &existing, current_revision).await?;
                    return setup_boundary::finish(
                        &mut execution,
                        request_id,
                        Some(current_revision),
                        SolveStartDto {
                            schema_version: SOLVE_API_SCHEMA_VERSION,
                            current_revision,
                            reused: true,
                            job,
                        },
                        SOLVE_RESPONSE_MAX_BYTES,
                        None,
                    )
                    .await;
                }
                require_solve_runtime(&app)?;
                let current_revision = solve_current_revision(&app, scenario_id).await?;
                require_solve_revision(current_revision, expected_revision)?;
                let mut progress = SolveProgressForwarder {
                    app: app.clone(),
                    request_id,
                    scenario_id,
                    revision: expected_revision,
                    run_id: Arc::clone(&run_id),
                    last: None,
                };
                let solved = app
                    .solve_stored_with_budget_observer(
                        stored_request,
                        &mut progress,
                        budget,
                        started_at,
                        observer,
                    )
                    .await
                    .map_err(|failure| Box::new(map_stored_solve_failure(failure)))?;
                let current_revision = solve_current_revision(&app, scenario_id).await?;
                let job = durable_solve_job(&app, solved.state(), current_revision).await?;
                if !solved.reused
                    && let Some(manifest) = solved.state().manifest.as_ref()
                {
                    match &manifest.outcome {
                        eutheto_domain_ir::RunTerminalOutcomeV1::Accepted { status, .. }
                        | eutheto_domain_ir::RunTerminalOutcomeV1::NoResult { status } => app
                            .publish_solve_completed(
                                request_id,
                                scenario_id,
                                job.scenario_revision,
                                job.run_id,
                                *status,
                                match &manifest.outcome {
                                    eutheto_domain_ir::RunTerminalOutcomeV1::Accepted {
                                        solution_id,
                                        ..
                                    } => Some(*solution_id),
                                    _ => None,
                                },
                            ),
                        // SolveCompleted carries SolveStatus only. Verification alarms and
                        // interrupted runs remain authoritative through the durable job read.
                        _ => {}
                    }
                }
                setup_boundary::finish(
                    &mut execution,
                    request_id,
                    Some(current_revision),
                    SolveStartDto {
                        schema_version: SOLVE_API_SCHEMA_VERSION,
                        current_revision,
                        reused: solved.reused,
                        job,
                    },
                    SOLVE_RESPONSE_MAX_BYTES,
                    None,
                )
                .await
            },
        )
        .await
}

#[tauri::command]
async fn solve_cancel<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolveCancelRequest = decode_solve_request(request)?;
    require_solve_version(request.schema_version)?;
    let acknowledgement = state.operations.cancel_solve(
        window.label(),
        &operations::OperationControlRequestV1 {
            request_id: request.cancel_request_id,
            schema_version: 1,
            operation_id: request.operation_id,
        },
        OperationContextV1::Scenario {
            scenario_id: request.scenario_id,
            expected_revision: Some(request.expected_revision),
        },
    )?;
    setup_boundary::encode_response(
        request.cancel_request_id,
        Some(request.expected_revision),
        SolveCancelDto {
            schema_version: SOLVE_API_SCHEMA_VERSION,
            cancel_request_id: request.cancel_request_id,
            acknowledgement: match acknowledgement.acknowledgement {
                operations::CancellationAcknowledgementV1::CancellationRequested => {
                    "cancellationRequested"
                }
                operations::CancellationAcknowledgementV1::NotActive => "notActive",
            },
        },
        SOLVE_RESPONSE_MAX_BYTES,
        None,
    )
    .await
}

#[tauri::command]
async fn solve_get_job<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolveGetJobRequest = decode_solve_request(request)?;
    require_solve_version(request.schema_version)?;
    let current = solve_current_revision(&state.app, request.scenario_id).await?;
    let retained = state
        .app
        .solve_get_job(request.scenario_id, request.run_id)
        .await
        .map_err(map_app_error)?
        .ok_or_else(|| {
            Box::new(map_app_error(AppError::NotFound(ResourceRef::SolveRun(
                request.run_id,
            ))))
        })?;
    let mut job = durable_solve_job(&state.app, &retained, current).await?;
    attach_live_solve_owner(&state.operations, window.label(), &mut job)?;
    setup_boundary::encode_response(
        request.request_id,
        Some(current),
        job,
        SOLVE_RESPONSE_MAX_BYTES,
        None,
    )
    .await
}

#[tauri::command]
async fn solve_list_runs<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolveListRunsRequest = decode_solve_request(request)?;
    require_solve_version(request.schema_version)?;
    if request.limit == 0 || request.limit > MAX_SOLVE_RUN_LIST_LIMIT {
        return Err(boundary_error(
            "solve.limit_invalid",
            "The solve history limit is outside the supported bound.",
            Some("/limit"),
        )
        .into());
    }
    let current = solve_current_revision(&state.app, request.scenario_id).await?;
    let retained = state
        .app
        .solve_list_runs(SolveRunListRequestV1 {
            scenario_id: request.scenario_id,
            scenario_revision: request.revision,
            limit: request.limit,
        })
        .await
        .map_err(map_app_error)?;
    let mut runs = Vec::with_capacity(retained.len());
    for run in &retained {
        let mut job = durable_solve_job(&state.app, run, current).await?;
        attach_live_solve_owner(&state.operations, window.label(), &mut job)?;
        runs.push(job);
    }
    setup_boundary::encode_response(
        request.request_id,
        Some(current),
        SolveListRunsDto {
            schema_version: SOLVE_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            current_revision: current,
            runs,
        },
        SOLVE_RESPONSE_MAX_BYTES,
        None,
    )
    .await
}

#[tauri::command]
async fn solve_get_diagnostics_summary<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: SolveGetJobRequest = decode_solve_request(request)?;
    require_solve_version(request.schema_version)?;
    let current = solve_current_revision(&state.app, request.scenario_id).await?;
    let retained = state
        .app
        .solve_get_job(request.scenario_id, request.run_id)
        .await
        .map_err(map_app_error)?
        .ok_or_else(|| {
            Box::new(map_app_error(AppError::NotFound(ResourceRef::SolveRun(
                request.run_id,
            ))))
        })?;
    let manifest = retained.manifest.as_ref();
    let diagnostic_code = manifest.and_then(|manifest| {
        if let eutheto_domain_ir::RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code } =
            &manifest.outcome
        {
            Some(diagnostic_code.clone())
        } else {
            None
        }
    });
    let mut job = durable_solve_job(&state.app, &retained, current).await?;
    attach_live_solve_owner(&state.operations, window.label(), &mut job)?;
    let first_verified_feasible_milliseconds = job
        .checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.verified_after_milliseconds)
        .or_else(|| manifest.and_then(|manifest| manifest.first_verified_feasible_milliseconds));
    setup_boundary::encode_response(
        request.request_id,
        Some(current),
        SolveDiagnosticsSummaryDto {
            schema_version: SOLVE_API_SCHEMA_VERSION,
            job,
            elapsed_milliseconds: manifest.and_then(|manifest| manifest.elapsed_milliseconds),
            first_incumbent_milliseconds: manifest
                .and_then(|manifest| manifest.first_incumbent_milliseconds),
            first_verified_feasible_milliseconds,
            phase_timings: manifest.map(|manifest| manifest.phase_timings),
            diagnostic_code,
            manifest_checksum: manifest.map(|manifest| manifest.checksum.clone()),
        },
        SOLVE_RESPONSE_MAX_BYTES,
        None,
    )
    .await
}

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
    (EventTopic::SolveVerified, "solve://verified"),
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
            let state = handle.state::<DesktopState>();
            state.operations.cancel_window(&label);
            state.csv.close_window(&label);
            state.settings.close_window(&label);
            state.portable.close_window(&label);
            state.recipient_reports.close_window(&label);
        }
        tauri::RunEvent::Exit => {
            let state = handle.state::<DesktopState>();
            state.operations.shutdown();
            state.csv.shutdown();
            state.settings.shutdown();
            state.portable.shutdown();
            state.recipient_reports.shutdown();
        }
        _ => {}
    });
    Ok(())
}

#[cfg(test)]
mod history_boundary_tests;
#[cfg(test)]
mod tests {
    use eutheto_core::{
        AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
        BackendSupportColumn, CapabilityMatrix, EuthetoApp, ScenarioSummaryV2,
        SolverSupportMatrixMetadata, StoredSolveFailure, SupportCell, SupportFeature,
        SupportFeatureCategory, SupportFeatureGate, SupportFeatureId,
    };
    use eutheto_domain_ir::RunManifest;
    use eutheto_types::{
        ApiErrorCategoryDto, ApiErrorDto, ApiResponseDto, AppError, BackendId, CancellationToken,
        CommandId, CommandResult, DomainPackRef, EntityId, KnownVerificationAlarmV1,
        ProjectListItemV1, ProjectMetadataDto, REVISION_MAX_V1, RequestId, Revision,
        SafeDiagnosticValue, ScenarioSettings, ScenarioViewDto, SolveMode, StorageFailure,
        SystemClock, SystemIdGenerator, VerificationAlarmTerminalization, VerificationFailure,
    };
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::error::Error;
    use std::sync::Arc;
    use tauri::Manager;

    use super::{
        DesktopState, NativeFileError, OPERATION_REQUEST_ENVELOPE_OVERHEAD_BYTES,
        SOLVE_REQUEST_MAX_BYTES, SolutionAdoptBaseRequestV1, SolutionCompareRequestV3,
        SolutionStartCounterfactualRequestV1, decode_counterfactual_start_request,
        decode_solution_request, map_app_error, map_native_file_error, map_stored_solve_failure,
        normalize_counterfactual_int64, operation_prepare, operation_release, pack_describe,
        pack_list, project_archive, project_create, project_delete, project_list, project_open,
        project_unarchive, read_bounded_portable, revision_diagnostic_value,
        scenario_apply_command, scenario_get_summary, scenario_redo, scenario_undo,
        selected_basename, solution_explain, solution_list, solution_response,
        solution_response_bounded, solve_options, solver_describe, solver_get_deferred_gates,
        solver_get_support_matrix, solver_list, solver_support_matrix_dto,
        suggested_portable_filename,
    };

    type IpcResult = Result<tauri::ipc::InvokeResponseBody, Value>;

    #[test]
    fn desktop_solve_time_override_preserves_modes_and_bounds_the_parent_budget()
    -> Result<(), Box<dyn Error>> {
        for (mode, default) in [
            (SolveMode::Quick, 2_000),
            (SolveMode::Balanced, 5_000),
            (SolveMode::Deep, 30_000),
        ] {
            let options = solve_options(mode, None).map_err(|error| format!("{error:?}"))?;
            assert_eq!(options.time_limit_milliseconds.value(), default);
            for override_ms in [1, 11_000, 30_000] {
                let options =
                    solve_options(mode, Some(override_ms)).map_err(|error| format!("{error:?}"))?;
                assert_eq!(
                    options.time_limit_milliseconds.value(),
                    u64::from(override_ms)
                );
            }
            for invalid in [0, 30_001, u32::MAX] {
                assert!(solve_options(mode, Some(invalid)).is_err());
            }
        }
        assert!(solve_options(SolveMode::Custom, Some(1)).is_err());
        Ok(())
    }

    #[test]
    fn native_known_verification_mapping_flattens_only_closed_scalars() -> Result<(), Box<dyn Error>>
    {
        let run_id = "018f47f2-e880-7000-8000-000000000010".parse()?;
        let alarm = KnownVerificationAlarmV1::new(
            run_id,
            &"ab".repeat(32),
            "verification.projection_failed",
            VerificationAlarmTerminalization::Unconfirmed,
            Some("storage.operation_failed"),
        )?;
        let mapped = map_app_error(AppError::Verification(
            VerificationFailure::from_known_alarm(alarm.clone(), None),
        ));
        assert_eq!(mapped.category, ApiErrorCategoryDto::Verification);
        assert_eq!(mapped.code, "verification.projection_failed");
        assert_eq!(mapped.message, VerificationFailure::KNOWN_ALARM_MESSAGE);
        let details = mapped.details.ok_or("known alarm details missing")?;
        assert_eq!(details.len(), 5);
        assert_eq!(
            details.get("runId"),
            Some(&SafeDiagnosticValue::Text(run_id.to_string()))
        );
        assert_eq!(
            details.get("inputChecksum"),
            Some(&SafeDiagnosticValue::Text("ab".repeat(32)))
        );
        assert_eq!(
            details.get("diagnosticCode"),
            Some(&SafeDiagnosticValue::Text(
                "verification.projection_failed".to_owned()
            ))
        );
        assert_eq!(
            details.get("terminalization"),
            Some(&SafeDiagnosticValue::Text("unconfirmed".to_owned()))
        );
        assert_eq!(
            details.get("secondaryCauseCode"),
            Some(&SafeDiagnosticValue::Text(
                "storage.operation_failed".to_owned()
            ))
        );
        assert!(details.keys().all(|key| {
            matches!(
                key.as_str(),
                "runId"
                    | "inputChecksum"
                    | "diagnosticCode"
                    | "terminalization"
                    | "secondaryCauseCode"
            )
        }));

        let mapped_stored = map_stored_solve_failure(StoredSolveFailure::AfterStart {
            run_id,
            known_alarm: Some(alarm),
            error: AppError::Storage(StorageFailure {
                code: "storage.operation_failed".to_owned(),
                message: "original private storage details are not public.".to_owned(),
                retryable: false,
                diagnostic_id: None,
            }),
            cleanup: Err(AppError::Storage(StorageFailure {
                code: "storage.operation_failed".to_owned(),
                message: "raw details are not public.".to_owned(),
                retryable: false,
                diagnostic_id: None,
            })),
        });
        assert_eq!(mapped_stored.category, ApiErrorCategoryDto::Verification);
        assert_eq!(mapped_stored.code, "verification.projection_failed");
        assert_eq!(
            mapped_stored
                .details
                .as_ref()
                .and_then(|details| details.get("terminalization")),
            Some(&SafeDiagnosticValue::Text("unconfirmed".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn native_exact_marked_alarm_readback_confirms_only_matching_manifest()
    -> Result<(), Box<dyn Error>> {
        let run_id = "018f47f2-e880-7000-8000-000000000010".parse()?;
        let checksum = "ab".repeat(32);
        let diagnostic_code = "verification.projection_failed";
        let marked_manifest = RunManifest::new_clock_failed_alarm(
            run_id,
            checksum.clone(),
            diagnostic_code.to_owned(),
            "2026-09-01T00:00:00Z".parse()?,
            "2026-09-01T00:00:01Z".parse()?,
        )?;
        let confirmed = map_stored_solve_failure(StoredSolveFailure::AfterStart {
            run_id,
            known_alarm: Some(KnownVerificationAlarmV1::new(
                run_id,
                &checksum,
                diagnostic_code,
                VerificationAlarmTerminalization::Unconfirmed,
                None,
            )?),
            error: AppError::Storage(StorageFailure {
                code: "storage.operation_failed".to_owned(),
                message: "the original execution failure remains private.".to_owned(),
                retryable: false,
                diagnostic_id: None,
            }),
            cleanup: Ok(Box::new(marked_manifest)),
        });
        assert_eq!(
            confirmed
                .details
                .as_ref()
                .and_then(|details| details.get("terminalization")),
            Some(&SafeDiagnosticValue::Text("confirmed".to_owned()))
        );

        let other_run = "018f47f2-e880-7000-8000-000000000011".parse()?;
        for (manifest_run, manifest_checksum, manifest_code) in [
            (other_run, checksum.clone(), diagnostic_code),
            (run_id, "cd".repeat(32), diagnostic_code),
            (run_id, checksum, "verification.score_failed"),
        ] {
            let manifest = RunManifest::new_clock_failed_alarm(
                manifest_run,
                manifest_checksum,
                manifest_code.to_owned(),
                "2026-09-01T00:00:00Z".parse()?,
                "2026-09-01T00:00:01Z".parse()?,
            )?;
            let alarm = KnownVerificationAlarmV1::new(
                run_id,
                &"ab".repeat(32),
                diagnostic_code,
                VerificationAlarmTerminalization::Confirmed,
                None,
            )?;
            let mapped = map_stored_solve_failure(StoredSolveFailure::AfterStart {
                run_id,
                known_alarm: Some(alarm),
                error: AppError::Storage(StorageFailure {
                    code: "storage.operation_failed".to_owned(),
                    message: "the original execution failure remains private.".to_owned(),
                    retryable: false,
                    diagnostic_id: None,
                }),
                cleanup: Ok(Box::new(manifest)),
            });
            assert_eq!(
                mapped
                    .details
                    .as_ref()
                    .and_then(|details| details.get("terminalization")),
                Some(&SafeDiagnosticValue::Text("unconfirmed".to_owned()))
            );
        }
        Ok(())
    }

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
    pub(super) async fn native_summary_and_core_snapshot(
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
        let cancellation = CancellationToken::new();
        let regular = directory.path().join("regular.eutheto");
        std::fs::write(&regular, b"portable")?;
        assert_eq!(
            read_bounded_portable(&regular, &cancellation),
            Ok(b"portable".to_vec())
        );
        let oversized = directory.path().join("oversized.eutheto");
        let oversized_file = std::fs::File::create(&oversized)?;
        oversized_file.set_len(eutheto_export::PORTABLE_LIMITS.max_archive_bytes + 1)?;
        assert_eq!(
            read_bounded_portable(&oversized, &cancellation),
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
                read_bounded_portable(&link, &cancellation),
                Err(NativeFileError::InvalidFileType)
            );
        }
        // Windows can refuse directory opening before same-handle type inspection.
        assert!(matches!(
            read_bounded_portable(directory.path(), &cancellation),
            Err(NativeFileError::InvalidFileType | NativeFileError::Unreadable)
        ));
        cancellation.cancel();
        assert_eq!(
            read_bounded_portable(&regular, &cancellation),
            Err(NativeFileError::Cancelled)
        );
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
    fn solve_progress_caps_same_phase_even_when_compiler_percent_changes() {
        let at = std::time::Instant::now();
        let last = Some((eutheto_types::SolvePhase::Compiling, at));
        assert!(!super::should_publish_progress(
            last,
            eutheto_types::SolvePhase::Compiling,
            at + std::time::Duration::from_millis(299)
        ));
        assert!(super::should_publish_progress(
            last,
            eutheto_types::SolvePhase::Compiling,
            at + std::time::Duration::from_millis(300)
        ));
        assert!(super::should_publish_progress(
            last,
            eutheto_types::SolvePhase::Verifying,
            at
        ));
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
            .invoke_handler(tauri::generate_handler![
                operation_prepare,
                operation_release,
                super::diagnostic_actions::solution_create_diagnostic_copy,
                super::diagnostic_actions::solution_export_diagnostic,
                super::recipient_report::solution_share_preview,
                super::recipient_report::solution_share_create,
                super::recipient_report::solution_share_release,
                super::schedule_export::solution_export_preview,
                super::schedule_export::solution_export,
                super::schedule_export::solution_export_cancel,
                super::schedule_export::solution_export_release,
                solution_explain,
                solution_list
            ])
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
                "freshlyVerified": false,
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
        let prepared: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "operation_prepare",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "purpose": {"kind": "solutionExplain"},
                "context": {
                    "kind": "scenario",
                    "scenarioId": scenario_id,
                    "expectedRevision": Revision::INITIAL,
                },
            }),
        )?;
        // Keep the admitted retained revision; it is not rewritten to a later current revision.
        let explanation_envelope = json!({
            "requestId": RequestId::new(&SystemIdGenerator)?,
            "schemaVersion": 2,
            "scenarioId": scenario_id,
            "operationId": prepared.result["operationId"],
            "sourceRevision": Revision::INITIAL,
            "request": explanation_request,
        });
        let oversized_request = json!({"oversized": "x".repeat(SOLVE_REQUEST_MAX_BYTES)});
        for (command, expected_code) in [
            ("solution_explain", "solution.request_too_large"),
            (
                "solution_create_diagnostic_copy",
                "diagnostic_action.request_too_large",
            ),
            (
                "solution_export_diagnostic",
                "diagnostic_action.request_too_large",
            ),
            ("solution_share_preview", "recipient.request_too_large"),
            ("solution_share_create", "recipient.request_too_large"),
            ("solution_share_release", "recipient.request_too_large"),
            (
                "solution_export_preview",
                "schedule_export.request_too_large",
            ),
            ("solution_export", "schedule_export.request_too_large"),
            (
                "solution_export_cancel",
                "schedule_export.request_too_large",
            ),
            (
                "solution_export_release",
                "schedule_export.request_too_large",
            ),
        ] {
            let error = invoke_ipc_args(
                &webview,
                command,
                json!({"request": oversized_request.clone(), "onProgress": 7}),
            )?
            .err()
            .ok_or("oversized operation request unexpectedly passed native admission")?;
            assert_eq!(error["code"], expected_code);
        }
        for (command, expected_code) in [
            ("solution_explain", "solution.request_too_large"),
            (
                "solution_create_diagnostic_copy",
                "diagnostic_action.request_too_large",
            ),
            (
                "solution_export_diagnostic",
                "diagnostic_action.request_too_large",
            ),
            ("solution_share_preview", "recipient.request_too_large"),
            ("solution_share_create", "recipient.request_too_large"),
            ("solution_share_release", "recipient.request_too_large"),
            (
                "solution_export_preview",
                "schedule_export.request_too_large",
            ),
            ("solution_export", "schedule_export.request_too_large"),
            (
                "solution_export_cancel",
                "schedule_export.request_too_large",
            ),
            (
                "solution_export_release",
                "schedule_export.request_too_large",
            ),
        ] {
            let error = invoke_ipc_args(
                &webview,
                command,
                json!({
                    "request": explanation_envelope.clone(),
                    "onProgress": "x".repeat(
                        SOLVE_REQUEST_MAX_BYTES + OPERATION_REQUEST_ENVELOPE_OVERHEAD_BYTES,
                    ),
                }),
            )?
            .err()
            .ok_or("oversized invoke envelope unexpectedly passed native admission")?;
            assert_eq!(error["code"], expected_code);
        }
        let error = invoke_ipc_args(
            &webview,
            "solution_explain",
            json!({
                "request": explanation_envelope.clone(),
                "onProgress": "__CHANNEL__:42",
                "padding": "x".repeat(
                    SOLVE_REQUEST_MAX_BYTES + OPERATION_REQUEST_ENVELOPE_OVERHEAD_BYTES,
                ),
            }),
        )?
        .err()
        .ok_or("oversized invoke envelope unexpectedly passed native admission")?;
        assert_eq!(error["code"], "solution.request_too_large");
        let explanation = invoke_ipc_args(
            &webview,
            "solution_explain",
            json!({"request": explanation_envelope, "onProgress": "__CHANNEL__:42"}),
        )?;
        let Err(explanation) = explanation else {
            return Err("unavailable validation explanation unexpectedly succeeded".into());
        };
        assert_eq!(
            explanation["code"],
            "solution.explanation_validation_unavailable"
        );
        // Ownership is the actual WebviewWindow label, not a caller-supplied context field.
        let other_webview =
            tauri::WebviewWindowBuilder::new(&desktop, "other", tauri::WebviewUrl::default())
                .build()?;
        let owner_prepared: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "operation_prepare",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "purpose": {"kind": "solutionExplain"},
                "context": {
                    "kind": "scenario",
                    "scenarioId": scenario_id,
                    "expectedRevision": Revision::INITIAL,
                },
            }),
        )?;
        let mut wrong_owner = explanation_envelope.clone();
        wrong_owner["requestId"] = json!(RequestId::new(&SystemIdGenerator)?);
        wrong_owner["operationId"] = owner_prepared.result["operationId"].clone();
        let wrong_owner = invoke_ipc_args(
            &other_webview,
            "solution_explain",
            json!({"request": wrong_owner, "onProgress": "__CHANNEL__:42"}),
        )?;
        let Err(wrong_owner) = wrong_owner else {
            return Err("explanation crossed its WebviewWindow owner".into());
        };
        assert_eq!(wrong_owner["code"], "operation.not_active");
        let context_prepared: ApiResponseDto<Value> = invoke_ok(
            &webview,
            "operation_prepare",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "purpose": {"kind": "solutionExplain"},
                "context": {
                    "kind": "scenario",
                    "scenarioId": scenario_id,
                    "expectedRevision": Revision::INITIAL,
                },
            }),
        )?;
        let mut wrong_context = explanation_envelope.clone();
        wrong_context["requestId"] = json!(RequestId::new(&SystemIdGenerator)?);
        wrong_context["operationId"] = context_prepared.result["operationId"].clone();
        wrong_context["scenarioId"] = json!(eutheto_types::ScenarioId::new(&SystemIdGenerator)?);
        let wrong_context = invoke_ipc_args(
            &webview,
            "solution_explain",
            json!({"request": wrong_context, "onProgress": "__CHANNEL__:42"}),
        )?;
        let Err(wrong_context) = wrong_context else {
            return Err("explanation crossed its admitted scenario context".into());
        };
        assert_eq!(wrong_context["code"], "operation.claim_mismatch");
        for operation_id in [
            owner_prepared.result["operationId"].clone(),
            context_prepared.result["operationId"].clone(),
        ] {
            let _: ApiResponseDto<Value> = invoke_ok(
                &webview,
                "operation_release",
                &json!({
                    "requestId": RequestId::new(&SystemIdGenerator)?,
                    "schemaVersion": 1,
                    "operationId": operation_id,
                }),
            )?;
        }
        assert_recipient_reservation_cleanup(&webview, &other_webview, scenario_id)?;

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

    #[allow(clippy::too_many_lines)]
    fn assert_recipient_reservation_cleanup(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        other: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        scenario_id: eutheto_types::ScenarioId,
    ) -> Result<(), Box<dyn Error>> {
        for (purpose, preview_command, release_command, format) in [
            (
                "sharePreview",
                "solution_share_preview",
                "solution_share_release",
                "html",
            ),
            (
                "sharePreview",
                "solution_share_preview",
                "solution_share_release",
                "pdf",
            ),
            (
                "exportPreview",
                "solution_export_preview",
                "solution_export_release",
                "ics",
            ),
        ] {
            let prepare = json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "purpose": {"kind": purpose},
                "context": {"kind": "scenario", "scenarioId": scenario_id, "expectedRevision": 0},
            });
            let mut operations = Vec::new();
            for _ in 0..2 {
                let prepared: ApiResponseDto<Value> =
                    invoke_ok(webview, "operation_prepare", &prepare)?;
                operations.push(prepared.result["operationId"].clone());
            }
            let mut preview = json!({
                "requestId": RequestId::new(&SystemIdGenerator)?,
                "schemaVersion": 1,
                "operationId": operations[0],
                "scenarioId": scenario_id,
                "expectedRevision": 0,
                "solutionId": RequestId::new(&SystemIdGenerator)?,
                "format": format,
            });
            if purpose == "sharePreview" {
                preview["options"] = json!({});
            } else {
                preview["personId"] = Value::Null;
            }
            for (patch, expected) in [
                (
                    json!({"format": if purpose == "sharePreview" { "ics" } else { "pdf" }}),
                    "setup.invalid_request",
                ),
                (
                    json!({"options": {"includeHiddenNotes": true}}),
                    "setup.invalid_request",
                ),
                (
                    json!({"destination": "caller-selected.html"}),
                    "setup.invalid_request",
                ),
                (json!({"schemaVersion": 2}), "operation.version_unsupported"),
                (
                    json!({"expectedRevision": 1}),
                    "recipient.preview_binding_mismatch",
                ),
                (
                    json!({"scenarioId": RequestId::new(&SystemIdGenerator)?}),
                    "recipient.preview_binding_mismatch",
                ),
            ] {
                let mut request = preview.clone();
                let Value::Object(fields) = patch else {
                    return Err("invalid request patch".into());
                };
                request
                    .as_object_mut()
                    .ok_or("invalid preview object")?
                    .extend(fields);
                let error = invoke_ipc_args(
                    webview,
                    preview_command,
                    json!({"request": request, "onProgress": "__CHANNEL__:42"}),
                )?
                .err()
                .ok_or("hostile recipient request passed admission")?;
                assert_eq!(error["code"], expected);
            }
            let error = invoke_ipc_args(
                other,
                preview_command,
                json!({"request": preview, "onProgress": "__CHANNEL__:42"}),
            )?
            .err()
            .ok_or("recipient reservation crossed native windows")?;
            assert_eq!(error["code"], "recipient.preview_not_active");
            let mut ordinary = prepare.clone();
            ordinary["purpose"] = json!({"kind": "scenarioSummary"});
            let ordinary: ApiResponseDto<Value> =
                invoke_ok(webview, "operation_prepare", &ordinary)?;
            let mut wrong_purpose = preview.clone();
            wrong_purpose["operationId"] = ordinary.result["operationId"].clone();
            let error = invoke_ipc_args(
                webview,
                preview_command,
                json!({"request": wrong_purpose, "onProgress": "__CHANNEL__:42"}),
            )?
            .err()
            .ok_or("ordinary read authority created a report")?;
            assert_eq!(error["code"], "recipient.preview_not_active");
            let _: ApiResponseDto<Value> = invoke_ok(
                webview,
                "operation_release",
                &json!({
                    "requestId": RequestId::new(&SystemIdGenerator)?,
                    "schemaVersion": 1,
                    "operationId": ordinary.result["operationId"],
                }),
            )?;
            // Repeated report refusals must not leak any of the sixteen generic operation slots.
            for _ in 0..32 {
                let error = invoke_ipc(webview, "operation_prepare", &prepare)?
                    .err()
                    .ok_or("recipient count limit was bypassed")?;
                assert_eq!(error["code"], "recipient.preview_capacity");
            }
            for (index, operation_id) in operations.into_iter().enumerate() {
                let command = if index == 0 {
                    release_command
                } else {
                    "operation_release"
                };
                let release = if index == 0 {
                    json!({
                        "requestId": RequestId::new(&SystemIdGenerator)?,
                        "schemaVersion": 1,
                        "scenarioId": scenario_id,
                        "target": {"kind": "operation", "operationId": operation_id},
                    })
                } else {
                    json!({
                        "requestId": RequestId::new(&SystemIdGenerator)?,
                        "schemaVersion": 1,
                        "operationId": operation_id,
                    })
                };
                let released: ApiResponseDto<Value> = invoke_ok(webview, command, &release)?;
                assert_eq!(released.result["acknowledgement"], "released");
                let mut released_preview = preview.clone();
                released_preview["operationId"] = operation_id.clone();
                let refused = invoke_ipc_args(
                    webview,
                    preview_command,
                    json!({
                        "request": released_preview,
                        "onProgress": "__CHANNEL__:42",
                    }),
                )?
                .err()
                .ok_or("released report reservation was recreated")?;
                assert_eq!(refused["code"], "recipient.preview_not_active");
                let _: ApiResponseDto<Value> = invoke_ok(
                    webview,
                    "operation_release",
                    &json!({
                        "requestId": RequestId::new(&SystemIdGenerator)?,
                        "schemaVersion": 1,
                        "operationId": operation_id,
                    }),
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn base_adoption_request_accepts_only_bounded_result_identity_not_assignments()
    -> Result<(), Box<dyn Error>> {
        let source = json!({
            "schemaVersion": 1,
            "requestId": RequestId::new(&SystemIdGenerator)?,
            "commandId": CommandId::new(&SystemIdGenerator)?,
            "scenarioId": eutheto_types::ScenarioId::new(&SystemIdGenerator)?,
            "expectedRevision": 3,
            "result": {
                "solutionId": eutheto_types::SolutionId::new(&SystemIdGenerator)?,
                "resultChecksum": "a".repeat(64),
            },
            "actor": {"actorId": null, "displayName": "Local desktop user"},
            "truncateRedo": false,
        });
        let parsed: SolutionAdoptBaseRequestV1 =
            decode_solution_request(Some(source.clone())).map_err(|error| error.message)?;
        assert_eq!(parsed.expected_revision, Revision::new(3));
        let mut forged = source.clone();
        forged["assignments"] = json!([{"personId": "untrusted", "shiftId": "untrusted"}]);
        let Err(error) = decode_solution_request::<SolutionAdoptBaseRequestV1>(Some(forged)) else {
            return Err("caller-owned assignments bypassed the stored-result projection".into());
        };
        assert_eq!(error.code, "solution.request_invalid");
        let mut oversized = source;
        oversized["padding"] = json!("x".repeat(65_536));
        let Err(error) = decode_solution_request::<SolutionAdoptBaseRequestV1>(Some(oversized))
        else {
            return Err("oversized input was admitted as a typed request".into());
        };
        assert_eq!(error.code, "solution.request_too_large");
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
    fn explanation_response_admits_the_complete_two_megabyte_encoded_envelope()
    -> Result<(), Box<dyn Error>> {
        #[derive(Clone, serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CompleteFixture {
            escaped: String,
            minimum: i64,
            wide_unsigned: u64,
            safe_unsigned: u64,
            padding: String,
        }

        let request_id = RequestId::new(&SystemIdGenerator)?;
        let escaped = "\"quoted\" \\\\ slash\nline\t\u{1f600}".to_owned();
        let fixture = CompleteFixture {
            escaped: escaped.clone(),
            minimum: i64::MIN,
            wide_unsigned: u64::MAX,
            safe_unsigned: 9_007_199_254_740_991,
            padding: String::new(),
        };
        let base = solution_response(request_id, Revision::INITIAL, fixture.clone())
            .map_err(|error| format!("base response serialization failed: {}", error.message))?;
        let base = tauri::ipc::IpcResponse::body(base)?;
        let tauri::ipc::InvokeResponseBody::Json(base) = base else {
            return Err("base explanation response was not JSON".into());
        };
        let base_bytes = base.len();
        assert!(base_bytes < super::EXPLANATION_RESPONSE_MAX_BYTES);
        let fixture = CompleteFixture {
            escaped,
            minimum: fixture.minimum,
            wide_unsigned: fixture.wide_unsigned,
            safe_unsigned: fixture.safe_unsigned,
            padding: "x".repeat(super::EXPLANATION_RESPONSE_MAX_BYTES - base_bytes),
        };
        let response = solution_response_bounded(
            request_id,
            Revision::INITIAL,
            fixture.clone(),
            Some(super::EXPLANATION_RESPONSE_MAX_BYTES),
        )
        .map_err(|error| {
            format!(
                "complete explanation response was rejected: {}",
                error.message
            )
        })?;
        let body = tauri::ipc::IpcResponse::body(response)?;
        let tauri::ipc::InvokeResponseBody::Json(body) = body else {
            return Err("complete explanation response was not JSON".into());
        };
        assert_eq!(body.len(), super::EXPLANATION_RESPONSE_MAX_BYTES);
        let value: Value = serde_json::from_str(&body)?;
        assert_eq!(value["result"]["escaped"], json!(fixture.escaped));
        assert_eq!(value["result"]["minimum"], json!(i64::MIN.to_string()));
        assert_eq!(value["result"]["wideUnsigned"], json!(u64::MAX.to_string()));
        assert_eq!(
            value["result"]["safeUnsigned"],
            json!(9_007_199_254_740_991_u64)
        );
        assert_eq!(value["requestId"], json!(request_id));
        Ok(())
    }

    #[test]
    fn explanation_response_observes_cancellation_after_serialization_completion()
    -> Result<(), Box<dyn Error>> {
        struct CancelAtCompletion {
            token: CancellationToken,
        }
        impl serde::Serialize for CancelAtCompletion {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let result = serializer.serialize_str("serialized");
                if result.is_ok() {
                    self.token.cancel();
                }
                result
            }
        }

        let token = CancellationToken::new();
        let control = super::OperationControl::Cancellation(token.clone());
        let result = super::solution_response_controlled(
            RequestId::new(&SystemIdGenerator)?,
            Revision::INITIAL,
            CancelAtCompletion { token },
            Some(super::EXPLANATION_RESPONSE_MAX_BYTES),
            Some(&control),
        );
        assert_eq!(
            result
                .err()
                .ok_or("serialization completed after cancellation")?
                .code,
            "operation.cancelled"
        );
        Ok(())
    }

    #[test]
    fn current_comparison_response_fails_closed_above_native_size_limit()
    -> Result<(), Box<dyn Error>> {
        let request_id = RequestId::new(&SystemIdGenerator)?;
        let scenario_id = eutheto_types::ScenarioId::new(&SystemIdGenerator)?;
        let base_id = eutheto_types::SolutionId::new(&SystemIdGenerator)?;
        let candidate_id = eutheto_types::SolutionId::new(&SystemIdGenerator)?;
        let request = json!({
            "schemaVersion": 3,
            "scenarioId": scenario_id,
            "baseSolutionId": base_id,
            "candidateSolutionId": candidate_id,
        });
        let decoded: SolutionCompareRequestV3 = decode_solution_request(Some(request.clone()))
            .map_err(|error| format!("valid V3 comparison request rejected: {error:?}"))?;
        assert_eq!(decoded.base_solution_id, base_id);
        let mut extra = request.clone();
        extra["unknown"] = json!(true);
        assert!(decode_solution_request::<SolutionCompareRequestV3>(Some(extra)).is_err());
        let too_large = solution_response_bounded(
            request_id,
            Revision::INITIAL,
            json!({ "difference": "x".repeat(super::SOLVE_RESPONSE_MAX_BYTES) }),
            Some(super::SOLVE_RESPONSE_MAX_BYTES),
        );
        assert!(matches!(
            too_large,
            Err(ref error) if error.code == "solution.response_too_large"
        ));
        Ok(())
    }

    #[test]
    fn comparison_explanation_binds_exact_refs_without_reanchoring_to_visible_head()
    -> Result<(), Box<dyn Error>> {
        use eutheto_domain_ir::{
            EvidenceRenderRequestV2, EvidenceRenderResultV1, ExplanationEvidencePayloadV2,
            ExplanationEvidenceV2, ExplanationRequestSubjectV1, ExplanationResultV2,
            PortableAcceptedResult, prepare_comparison_v2,
        };

        // Portable contract data is not fresh or local acceptance authority.
        let source = PortableAcceptedResult::from_json(include_bytes!(
            "../../../../crates/eutheto-domain-ir/tests/fixtures/portable_accepted_v2.json"
        ))?;
        let comparison = prepare_comparison_v2(&source, &source, None)?
            .finish(eutheto_domain_ir::ComparisonEnrichmentV2::default())?;
        let source_revision = Revision::try_new(source.scenario_revision)?;
        let visible_revision = Revision::try_new(source.scenario_revision + 1)?;
        let subject = ExplanationRequestSubjectV1::SolutionDifference {
            left: comparison.base.accepted_result.clone(),
            right: comparison.candidate.accepted_result.clone(),
        };
        let evidence =
            ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::SolutionDifference {
                comparison: Box::new(comparison),
            })?;
        let request = EvidenceRenderRequestV2::new(evidence.clone())?;
        let rendered = EvidenceRenderResultV1::new(&request, Vec::new())?;
        let mut result = eutheto_core::SolutionExplanationDtoV2 {
            schema_version: 2,
            scenario_id: source.scenario_id,
            current_revision: visible_revision,
            scenario_revisions: vec![source_revision, source_revision],
            explanation: ExplanationResultV2::new(evidence, rendered)?,
            infeasibility_context: None,
        };
        super::validate_solution_explanation_source(
            source.scenario_id,
            source_revision,
            &subject,
            &result,
        )
        .map_err(|error| format!("immutable-source explanation rejected: {error:?}"))?;
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                visible_revision,
                &subject,
                &result,
            )
            .is_err()
        );

        let mut wrong_subject = subject.clone();
        if let ExplanationRequestSubjectV1::SolutionDifference { right, .. } = &mut wrong_subject {
            right.result_checksum = "b".repeat(64);
        }
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                source_revision,
                &wrong_subject,
                &result,
            )
            .is_err()
        );
        result.scenario_revisions.push(visible_revision);
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                source_revision,
                &subject,
                &result,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn current_status_reply_refuses_recorded_backend_proof_and_legacy_payload()
    -> Result<(), Box<dyn Error>> {
        use eutheto_domain_ir::{
            AcceptedResultRefV1, EvidenceRenderRequestV2, EvidenceRenderResultV1,
            ExplanationCertainty, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
            ExplanationRequestSubjectV1, ExplanationResultV2, OptimalityStatusEvidence,
            PortableAcceptedResult,
        };
        let source = PortableAcceptedResult::from_json(include_bytes!(
            "../../../../crates/eutheto-domain-ir/tests/fixtures/portable_accepted_v2.json"
        ))?;
        let revision = Revision::try_new(source.scenario_revision)?;
        let reference = AcceptedResultRefV1::from_result(&source.accepted_result)?;
        let subject = ExplanationRequestSubjectV1::OptimalityStatus {
            solve_run_id: source.run_input.run_id,
            run_manifest_checksum: source.run_manifest.checksum.clone(),
            result: Some(reference.clone()),
        };
        let response =
            |schema_version| -> Result<eutheto_core::SolutionExplanationDtoV2, Box<dyn Error>> {
                let evidence =
                    ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::OptimalityStatus {
                        status: Box::new(OptimalityStatusEvidence {
                            schema_version,
                            run_input: source.run_input.clone(),
                            run_manifest: source.run_manifest.clone(),
                            result: Some(reference.clone()),
                        }),
                    })?;
                let request = EvidenceRenderRequestV2::new(evidence.clone())?;
                let rendered = EvidenceRenderResultV1::new(&request, Vec::new())?;
                Ok(eutheto_core::SolutionExplanationDtoV2 {
                    schema_version: 2,
                    scenario_id: source.scenario_id,
                    current_revision: revision,
                    scenario_revisions: vec![revision],
                    explanation: ExplanationResultV2::new(evidence, rendered)?,
                    infeasibility_context: None,
                })
            };
        let mut current = response(Some(2))?;
        super::validate_solution_explanation_source(
            source.scenario_id,
            revision,
            &subject,
            &current,
        )
        .map_err(|error| format!("descriptive current status rejected: {error:?}"))?;
        current.explanation.evidence.certainty = ExplanationCertainty::BackendProof;
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                revision,
                &subject,
                &current,
            )
            .is_err()
        );
        let legacy = response(None)?;
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                revision,
                &subject,
                &legacy,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn retained_unavailable_diagnostic_binds_its_own_custody_checksum() -> Result<(), Box<dyn Error>>
    {
        use eutheto_domain_ir::{
            ConflictUnavailableReason, DomainEvidenceId, EvidenceRenderRequestV2,
            EvidenceRenderResultV1, ExplanationEvidencePayloadV2, ExplanationEvidenceV2,
            ExplanationRequestSubjectV1, ExplanationResultV2, InfeasibilityEvidenceV1,
            PortableAcceptedResult,
        };

        // Contract identity data only; this does not establish native diagnostic authority.
        let source = PortableAcceptedResult::from_json(include_bytes!(
            "../../../../crates/eutheto-domain-ir/tests/fixtures/portable_accepted_v2.json"
        ))?;
        let revision = Revision::try_new(source.scenario_revision)?;
        let retained_checksum = "a".repeat(64);
        let conflict_id = DomainEvidenceId::new(format!("infeasibility.{retained_checksum}"))?;
        let subject = ExplanationRequestSubjectV1::Infeasibility {
            solve_run_id: source.run_input.run_id,
            run_manifest_checksum: source.run_manifest.checksum.clone(),
            conflict_id: Some(conflict_id.clone()),
        };
        let evidence = ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Infeasibility {
            infeasibility: InfeasibilityEvidenceV1::Unavailable {
                reason: ConflictUnavailableReason::FoundationalInfeasibility,
            },
            condition: None,
        })?;
        let request = EvidenceRenderRequestV2::new(evidence.clone())?;
        let rendered = EvidenceRenderResultV1::new(&request, Vec::new())?;
        let mut result = eutheto_core::SolutionExplanationDtoV2 {
            schema_version: 2,
            scenario_id: source.scenario_id,
            current_revision: revision,
            scenario_revisions: vec![revision],
            explanation: ExplanationResultV2::new(evidence, rendered)?,
            infeasibility_context: Some(eutheto_core::InfeasibilityExplanationContextV1 {
                run_id: source.run_input.run_id,
                source_revision: revision,
                run_manifest_checksum: source.run_manifest.checksum.clone(),
                evidence_checksum: retained_checksum,
                conflict_id,
            }),
        };
        super::validate_solution_explanation_source(
            source.scenario_id,
            revision,
            &subject,
            &result,
        )
        .map_err(|error| format!("retained unavailable evidence rejected: {error:?}"))?;
        result
            .infeasibility_context
            .as_mut()
            .ok_or("missing retained context")?
            .evidence_checksum = result.explanation.evidence.checksum.clone();
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                revision,
                &subject,
                &result,
            )
            .is_err()
        );
        result.infeasibility_context = None;
        assert!(
            super::validate_solution_explanation_source(
                source.scenario_id,
                revision,
                &subject,
                &result,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn counterfactual_start_bounds_input_before_integer_normalization() {
        let request = json!({
            "condition": {"value": {"type": "integer", "value": "not-an-integer"}},
            "unknown": "x".repeat(SOLVE_REQUEST_MAX_BYTES),
        });
        let failure = decode_counterfactual_start_request(Some(request));
        assert!(matches!(
            failure,
            Err(ref error) if error.code == "solution.request_too_large"
        ));
    }

    #[test]
    fn bounded_response_writer_stops_before_retaining_excess_bytes() -> Result<(), Box<dyn Error>> {
        use std::io::Write;
        let mut writer = super::LimitedResponseWriter {
            bytes: Vec::new(),
            maximum: Some(4),
            exceeded: false,
            control: None,
            interrupted: None,
        };
        writer.write_all(b"1234")?;
        assert!(writer.write_all(b"56").is_err());
        assert_eq!(writer.bytes, b"1234");
        assert!(writer.exceeded);
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

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn native_calendar_creation_and_open_preserve_authoritative_dates()
    -> Result<(), Box<dyn Error>> {
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
        let desktop = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                project_create,
                project_list,
                project_open
            ])
            .manage(state.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
        let window =
            tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default())
                .build()?;
        let request = json!({
            "schemaVersion": 1, "requestId": RequestId::new(&SystemIdGenerator)?,
            "title": "Spring clinic", "description": "",
            "domainPack": {"id": "official.workforce", "schemaVersion": 1},
            "settings": {
                "timeZone": "America/New_York", "locale": "en-US", "units": "metric",
                "firstDate": "2026-03-07", "lastDate": "2026-03-08",
                "gapPolicy": "reject", "overlapPolicy": "earlier"
            }
        });
        let created: ApiResponseDto<ProjectMetadataDto> =
            invoke_ok(&window, "project_create", &request)?;
        let AppQueryResult::Scenario(view) = state
            .app
            .query(AppQuery::ScenarioView(created.result.scenario_id))
            .await
            .map_err(|error| format!("{error:?}"))?
        else {
            return Err("created scenario unavailable".into());
        };
        assert_eq!(
            view.document.settings.horizon.start.to_string(),
            "2026-03-07T05:00:00Z"
        );
        assert_eq!(
            view.document.settings.horizon.end.to_string(),
            "2026-03-09T04:00:00Z"
        );
        let list_request = json!({
            "schemaVersion": 1, "requestId": RequestId::new(&SystemIdGenerator)?, "scope": "all"
        });
        let listed: ApiResponseDto<Vec<ProjectListItemV1>> =
            invoke_ok(&window, "project_list", &list_request)?;
        assert_eq!(listed.result.len(), 1);
        assert_eq!(listed.result[0].scenario_id, created.result.scenario_id);
        assert!(listed.result[0].last_opened_at.is_none());
        let before_open = state
            .app
            .application_settings_snapshot()
            .await
            .map_err(|error| format!("{error:?}"))?
            .library_revision;
        let opened: ApiResponseDto<ProjectListItemV1> = invoke_ok(
            &window,
            "project_open",
            &json!({
                "schemaVersion": 1, "requestId": RequestId::new(&SystemIdGenerator)?,
                "scenarioId": created.result.scenario_id
            }),
        )?;
        assert!(opened.result.last_opened_at.is_some());
        assert_eq!(opened.result.revision, created.result.revision);
        let after_open = state
            .app
            .application_settings_snapshot()
            .await
            .map_err(|error| format!("{error:?}"))?
            .library_revision;
        assert_eq!(after_open, before_open.checked_next()?);
        for (last_date, code) in [
            ("2026-03-06", "project.date_range_invalid"),
            ("9999-12-31", "project.date_overflow"),
        ] {
            let mut invalid = request.clone();
            invalid["requestId"] = json!(RequestId::new(&SystemIdGenerator)?);
            invalid["settings"]["lastDate"] = json!(last_date);
            let rejected = invoke_ipc(&window, "project_create", &invalid)?
                .err()
                .ok_or("invalid calendar range created a project")?;
            assert_eq!(rejected["code"], code);
        }
        let mut skipped = request;
        skipped["requestId"] = json!(RequestId::new(&SystemIdGenerator)?);
        skipped["settings"]["timeZone"] = json!("Pacific/Apia");
        skipped["settings"]["firstDate"] = json!("2011-12-30");
        skipped["settings"]["lastDate"] = json!("2011-12-31");
        skipped["settings"]["gapPolicy"] = json!("moveForward");
        let rejected = invoke_ipc(&window, "project_create", &skipped)?
            .err()
            .ok_or("skipped date created a shifted project")?;
        assert_eq!(rejected["code"], "project.midnight_invalid");
        let listed: ApiResponseDto<Vec<ProjectListItemV1>> =
            invoke_ok(&window, "project_list", &list_request)?;
        assert_eq!(listed.result.len(), 1);
        assert_eq!(
            listed.result[0].last_opened_at,
            opened.result.last_opened_at
        );
        let after_rejection = state
            .app
            .application_settings_snapshot()
            .await
            .map_err(|error| format!("{error:?}"))?
            .library_revision;
        assert_eq!(after_rejection, after_open);
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
                "schemaVersion": 1,
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
                    "firstDate": "2026-09-01",
                    "lastDate": "2026-09-30",
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
        let listed: ApiResponseDto<Vec<ProjectListItemV1>> = invoke_ok(
            &reopened_webview,
            "project_list",
            &json!({
                "requestId": list_request_id,
                "schemaVersion": 1,
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
