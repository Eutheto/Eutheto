//! Native portable operations own admission, file grants and finite review custody.
mod custody;

use crate::operations::{
    OperationClaim, OperationContextV1, OperationControlRequestV1, OperationExecution,
    OperationPhaseV1, OperationPurposeV1, require_version,
};
use crate::setup_boundary::{channel, decode, encode_response};
use crate::{
    ApiError, DesktopState, NativeFileError, PortableAppliedDto, PortableArtifactDto,
    PortableCapabilityDto, PortableFilePreviewDto, PreparedPortableKind, PreparedPortableOutput,
    RestoreAuthorizationDto, SolutionApiResult, UnopenedBundleMetadataDto,
    UnopenedBundlePreviewDto, UnopenedBundleScenarioDto, backup_summary, boundary_error,
    map_app_error, native_file_task, new_prepared_preview_id, portable_preview,
    prepared_output_error, read_bounded_portable, require_portable_path, selected_basename,
    suggested_portable_filename,
};
pub(crate) use custody::PortableCustody;
use custody::{Creator, PreviewTarget, ReviewBinding, ReviewKind};
use eutheto_core::{
    AppCommand, AppCommandResult, AppQuery, AppQueryResult, BackupSelection,
    PreparedPortableBinding, bounded_json_size,
};
use eutheto_import::{CollisionPlan, ImportOptions};
use eutheto_types::{
    CancellationToken, OperationId, RequestId, Revision, SafeDiagnosticValue, ScenarioId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const COMPACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    options: ImportOptions,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum RestoreOrigin {
    UserSelected,
    SafetyBackups,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestorePreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    options: ImportOptions,
    origin: RestoreOrigin,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportApplyRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    preview_id: RequestId,
    expected_library_revision: Revision,
    collision_plan: CollisionPlan,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreApplyRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    preview_id: RequestId,
    expected_library_revision: Revision,
    collision_plan: CollisionPlan,
    authorization: RestoreAuthorizationDto,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportCreateRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    expected_library_revision: Revision,
    preview_id: RequestId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupCreateRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    expected_library_revision: Revision,
    preview_id: RequestId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnopenedReexportRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    preview_id: RequestId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiscardRequest {
    schema_version: u32,
    request_id: RequestId,
    target: PreviewTarget,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Discarded {
    schema_version: u32,
}

fn bounded_request(request: Option<Value>) -> Result<Option<Value>, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, COMPACT_BYTES).is_err())
    {
        return Err(boundary_error(
            "portable.request_too_large",
            "The portable request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    Ok(request)
}

fn library_claim(
    operation_id: OperationId,
    request_id: RequestId,
    purpose: OperationPurposeV1,
    expected_library_revision: Option<Revision>,
) -> OperationClaim {
    OperationClaim {
        operation_id,
        request_id,
        purpose: Some(purpose),
        context: OperationContextV1::Library {
            expected_library_revision,
        },
    }
}

async fn finish<T: Serialize + Send + 'static>(
    execution: &mut OperationExecution,
    request_id: RequestId,
    revision: Option<Revision>,
    result: T,
    cancellation: Option<CancellationToken>,
) -> SolutionApiResult {
    execution.preparing_response();
    let check_token = cancellation.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        if check_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ApiError::from(crate::operations::cancelled()));
        }
        bounded_json_size(&result, COMPACT_BYTES).map_err(|_| {
            ApiError::from(boundary_error(
                "portable.response_too_large",
                "The portable result exceeds its compact response limit.",
                None,
            ))
        })?;
        if check_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ApiError::from(crate::operations::cancelled()));
        }
        Ok(result)
    })
    .await
    .map_err(|_| {
        boundary_error(
            "operation.execution_failed",
            "The portable response could not be prepared safely.",
            None,
        )
    })??;
    encode_response(request_id, revision, result, COMPACT_BYTES, cancellation).await
}

async fn read_selection<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    title: &'static str,
    directory: Option<PathBuf>,
    cancellation: CancellationToken,
    execution: &mut OperationExecution,
) -> Result<Vec<u8>, ApiError> {
    let picker_token = cancellation.clone();
    let path = native_file_task(move || {
        if picker_token.is_cancelled() {
            return Err(NativeFileError::Cancelled);
        }
        let mut picker = handle
            .dialog()
            .file()
            .set_title(title)
            .add_filter("Eutheto portable file", &["eutheto"]);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }
        let selected = picker
            .blocking_pick_file()
            .ok_or(NativeFileError::Cancelled)?;
        if picker_token.is_cancelled() {
            return Err(NativeFileError::Cancelled);
        }
        selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)
    })
    .await?;
    execution.report(OperationPhaseV1::DetectingFormat);
    native_file_task(move || read_bounded_portable(&path, &cancellation)).await
}

async fn save_selection<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    title: &'static str,
    suggested_name: String,
    cancellation: CancellationToken,
) -> Result<(PathBuf, String), ApiError> {
    native_file_task(move || {
        if cancellation.is_cancelled() {
            return Err(NativeFileError::Cancelled);
        }
        let selected = handle
            .dialog()
            .file()
            .set_title(title)
            .set_file_name(suggested_name)
            .add_filter("Eutheto portable file", &["eutheto"])
            .blocking_save_file()
            .ok_or(NativeFileError::Cancelled)?;
        if cancellation.is_cancelled() {
            return Err(NativeFileError::Cancelled);
        }
        let destination = selected
            .into_path()
            .map_err(|_| NativeFileError::Conversion)
            .and_then(require_portable_path)?;
        let artifact_name = selected_basename(&destination)?;
        Ok((destination, artifact_name))
    })
    .await
}

enum InputPreview {
    Import(ImportOptions),
    Restore {
        options: ImportOptions,
        origin: RestoreOrigin,
    },
    Unopened,
}

#[tauri::command]
pub(super) async fn project_import_preview<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: ImportPreviewRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    preview_input(
        webview,
        state,
        request.request_id,
        request.operation_id,
        InputPreview::Import(request.options),
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn project_restore_preview<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: RestorePreviewRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    preview_input(
        webview,
        state,
        request.request_id,
        request.operation_id,
        InputPreview::Restore {
            options: request.options,
            origin: request.origin,
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn project_unopened_bundle_inspect<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: OperationRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    preview_input(
        webview,
        state,
        request.request_id,
        request.operation_id,
        InputPreview::Unopened,
        on_progress,
    )
    .await
}

async fn preview_input<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request_id: RequestId,
    operation_id: OperationId,
    input: InputPreview,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let owner = webview.window().label().to_owned();
    let handle = webview.app_handle().clone();
    let sink = channel(webview, on_progress)?;
    let (kind, purpose, title, directory) = match &input {
        InputPreview::Import(_) => (
            ReviewKind::Import,
            OperationPurposeV1::ProjectImportPreview,
            "Choose an Eutheto file to import",
            None,
        ),
        InputPreview::Restore { origin, .. } => (
            ReviewKind::Restore,
            OperationPurposeV1::ProjectRestorePreview,
            "Choose an Eutheto backup to restore",
            match origin {
                RestoreOrigin::UserSelected => None,
                RestoreOrigin::SafetyBackups => Some(state.backup_dir.clone()),
            },
        ),
        InputPreview::Unopened => (
            ReviewKind::Unopened,
            OperationPurposeV1::ProjectUnopenedBundleInspect,
            "Choose an unopened Eutheto bundle to inspect",
            None,
        ),
    };
    let custody = Arc::clone(&state.portable);
    let app = state.app.clone();
    state.operations.run(&owner.clone(), library_claim(operation_id, request_id, purpose, None),
        Some(sink), OperationPhaseV1::SelectingFile, (input, None),
        move |input, mut execution| async move {
            let cancellation = execution.cancellation();
            let reservation = custody.reserve(&owner, Creator { operation_id, request_id }, kind,
                cancellation.clone())?;
            let bytes = read_selection(handle, title, directory, cancellation.clone(), &mut execution).await?;
            execution.report(OperationPhaseV1::Validating);
            let query = match input {
                InputPreview::Import(options) => AppQuery::PreviewImport { bytes, options, cancellation: cancellation.clone() },
                InputPreview::Restore { options, .. } => AppQuery::PreviewRestore { bytes, options, cancellation: cancellation.clone() },
                InputPreview::Unopened => AppQuery::InspectUnopenedBundle { bytes, cancellation: cancellation.clone() },
            };
            let result = match app.query(query).await.map_err(map_app_error)? {
                AppQueryResult::PortablePreview { preview_id, preview } => {
                    let revision = preview.binding.local_library_revision;
                    let binding = match kind {
                        ReviewKind::Import => ReviewBinding::Import { library_revision: revision },
                        ReviewKind::Restore => ReviewBinding::Restore { library_revision: revision },
                        _ => return Err(prepared_output_error("protocol.result_mismatch").into()),
                    };
                    reservation.publish_core(preview_id, binding)?;
                    finish(&mut execution, request_id, Some(revision), portable_preview(preview_id, *preview),
                        Some(cancellation)).await
                }
                AppQueryResult::UnopenedBundlePreview { preview_id, metadata } if kind == ReviewKind::Unopened => {
                    reservation.publish_core(preview_id, ReviewBinding::Unopened)?;
                    let result = UnopenedBundlePreviewDto {
                        schema_version: 1, preview_id,
                        metadata: UnopenedBundleMetadataDto {
                            file_sha256: metadata.file_sha256, format: metadata.format,
                            format_version: metadata.format_version,
                            portable_schema_version: metadata.portable_schema_version,
                            bundle_kind: metadata.bundle_kind, title: metadata.title,
                            required_capabilities: metadata.required_capabilities.into_iter().map(|capability| {
                                PortableCapabilityDto { id: capability.id, version: capability.version }
                            }).collect(),
                            scenarios: metadata.scenarios.into_iter().map(|scenario| UnopenedBundleScenarioDto {
                                path: scenario.path, scenario_id: scenario.scenario_id, pack_id: scenario.pack_id,
                                internal_pack_schema_version: scenario.internal_pack_schema_version,
                                portable_pack_schema_version: scenario.portable_pack_schema_version,
                            }).collect(),
                        },
                    };
                    finish(&mut execution, request_id, None, result, Some(cancellation)).await
                }
                _ => return Err(prepared_output_error("protocol.result_mismatch").into()),
            };
            if result.is_err() { custody.discard(&owner, &PreviewTarget::Creator { operation_id, request_id }); }
            result
        }).await
}

struct ApplyInput {
    request_id: RequestId,
    operation_id: OperationId,
    preview_id: RequestId,
    expected_library_revision: Revision,
    collision_plan: CollisionPlan,
    authorization: Option<RestoreAuthorizationDto>,
}

#[tauri::command]
pub(super) async fn project_import_apply<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: ImportApplyRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    apply_input(
        webview,
        state,
        ApplyInput {
            request_id: request.request_id,
            operation_id: request.operation_id,
            preview_id: request.preview_id,
            expected_library_revision: request.expected_library_revision,
            collision_plan: request.collision_plan,
            authorization: None,
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn project_restore_apply<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: RestoreApplyRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    apply_input(
        webview,
        state,
        ApplyInput {
            request_id: request.request_id,
            operation_id: request.operation_id,
            preview_id: request.preview_id,
            expected_library_revision: request.expected_library_revision,
            collision_plan: request.collision_plan,
            authorization: Some(request.authorization),
        },
        on_progress,
    )
    .await
}

async fn apply_input<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    input: ApplyInput,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let owner = webview.window().label().to_owned();
    let sink = channel(webview, on_progress)?;
    let custody = Arc::clone(&state.portable);
    let app = state.app.clone();
    let (purpose, binding) = if input.authorization.is_some() {
        (
            OperationPurposeV1::ProjectRestoreApply,
            ReviewBinding::Restore {
                library_revision: input.expected_library_revision,
            },
        )
    } else {
        (
            OperationPurposeV1::ProjectImportApply,
            ReviewBinding::Import {
                library_revision: input.expected_library_revision,
            },
        )
    };
    state
        .operations
        .run(
            &owner.clone(),
            library_claim(
                input.operation_id,
                input.request_id,
                purpose,
                Some(input.expected_library_revision),
            ),
            Some(sink),
            OperationPhaseV1::ApplyingPreview,
            (input, None),
            move |input, mut execution| async move {
                let lease = custody.acquire(&owner, input.preview_id, binding)?;
                let cancellation = execution.cancellation();
                let command = if let Some(authorization) = input.authorization {
                    AppCommand::ApplyRestore {
                        request_id: input.request_id,
                        preview_id: input.preview_id,
                        collision_plan: input.collision_plan,
                        authorization: authorization.into(),
                        cancellation: cancellation.clone(),
                    }
                } else {
                    AppCommand::ApplyImport {
                        request_id: input.request_id,
                        preview_id: input.preview_id,
                        collision_plan: input.collision_plan,
                        cancellation: cancellation.clone(),
                    }
                };
                let applied = match app.execute(command).await {
                    Ok(applied) => applied,
                    Err(error) => {
                        let mut error = map_app_error(error);
                        let retained = lease.retain_restore_retry(&cancellation).await;
                        if error.code == "restore.safety_backup_failed" {
                            error.details.get_or_insert_with(Default::default).insert(
                                "portablePreviewRetained".to_owned(),
                                SafeDiagnosticValue::Boolean(retained),
                            );
                        }
                        return Err(error.into());
                    }
                };
                let AppCommandResult::PortableApplied {
                    scenarios,
                    library_revision,
                    safety_backup,
                } = applied
                else {
                    return Err(prepared_output_error("protocol.result_mismatch").into());
                };
                let result = finish(
                    &mut execution,
                    input.request_id,
                    Some(library_revision),
                    PortableAppliedDto {
                        schema_version: 1,
                        library_revision,
                        safety_backup: safety_backup.into(),
                        scenario_ids: scenarios
                            .into_iter()
                            .map(|scenario| scenario.scenario_id)
                            .collect(),
                    },
                    None,
                )
                .await;
                drop(lease);
                result
            },
        )
        .await
}

async fn export_title(
    app: &eutheto_core::EuthetoApp,
    scenario_id: ScenarioId,
) -> Result<String, ApiError> {
    match app
        .query(AppQuery::ProjectMetadata(scenario_id))
        .await
        .map_err(map_app_error)?
    {
        AppQueryResult::Project(project) => Ok(project.title),
        _ => Err(prepared_output_error("protocol.result_mismatch").into()),
    }
}

#[tauri::command]
pub(super) async fn project_export_preview<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: ExportPreviewRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    let owner = webview.window().label().to_owned();
    let sink = channel(webview, on_progress)?;
    let custody = Arc::clone(&state.portable);
    let app = state.app.clone();
    let claim = crate::setup_boundary::claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::ProjectExportPreview,
        request.scenario_id,
        Some(request.expected_revision),
    );
    state
        .operations
        .run(
            &owner.clone(),
            claim,
            Some(sink),
            OperationPhaseV1::CapturingSnapshot,
            (request, None),
            move |request, mut execution| async move {
                let cancellation = execution.cancellation();
                let reservation = custody.reserve(
                    &owner,
                    Creator {
                        operation_id: request.operation_id,
                        request_id: request.request_id,
                    },
                    ReviewKind::ScenarioExport,
                    cancellation.clone(),
                )?;
                let title = export_title(&app, request.scenario_id).await?;
                let AppQueryResult::Bundle {
                    bytes,
                    scenario_revision,
                    library_revision,
                } = app
                    .query(AppQuery::ExportScenario {
                        scenario_id: request.scenario_id,
                        cancellation: cancellation.clone(),
                    })
                    .await
                    .map_err(map_app_error)?
                else {
                    return Err(prepared_output_error("protocol.result_mismatch").into());
                };
                if scenario_revision != request.expected_revision {
                    return Err(map_app_error(eutheto_types::AppError::Conflict {
                        expected_revision: request.expected_revision,
                        actual_revision: scenario_revision,
                    })
                    .into());
                }
                let preview_id = new_prepared_preview_id()?;
                let digest = eutheto_export::sha256_hex(&bytes);
                let byte_length = bytes.len();
                reservation.publish_prepared(
                    preview_id,
                    PreparedPortableOutput {
                        bytes,
                        sha256: digest.clone(),
                        kind: PreparedPortableKind::Scenario {
                            scenario_id: request.scenario_id,
                            revision: scenario_revision,
                            library_revision,
                            title: title.clone(),
                        },
                    },
                )?;
                let result = finish(
                    &mut execution,
                    request.request_id,
                    Some(scenario_revision),
                    PortableFilePreviewDto {
                        schema_version: 1,
                        title,
                        byte_length,
                        backup_summary: None,
                        preview_id,
                        digest,
                        current_revision: Some(scenario_revision),
                        library_revision,
                    },
                    Some(cancellation),
                )
                .await;
                if result.is_err() {
                    custody.discard(&owner, &PreviewTarget::Preview { preview_id });
                }
                result
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn project_backup_preview<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: BackupPreviewRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    let owner = webview.window().label().to_owned();
    let sink = channel(webview, on_progress)?;
    let custody = Arc::clone(&state.portable);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            library_claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::ProjectBackupPreview,
                None,
            ),
            Some(sink),
            OperationPhaseV1::CapturingSnapshot,
            (request, None),
            move |request, mut execution| async move {
                let cancellation = execution.cancellation();
                let reservation = custody.reserve(
                    &owner,
                    Creator {
                        operation_id: request.operation_id,
                        request_id: request.request_id,
                    },
                    ReviewKind::Backup,
                    cancellation.clone(),
                )?;
                let title = request.title.trim().to_owned();
                let AppQueryResult::BackupBundle {
                    bytes,
                    summary,
                    library_revision,
                } = app
                    .query(AppQuery::ExportBackup {
                        title: title.clone(),
                        selection: BackupSelection::default(),
                        cancellation: cancellation.clone(),
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
                reservation.publish_prepared(
                    preview_id,
                    PreparedPortableOutput {
                        bytes,
                        sha256: digest.clone(),
                        kind: PreparedPortableKind::Backup {
                            library_revision,
                            title: title.clone(),
                        },
                    },
                )?;
                let result = finish(
                    &mut execution,
                    request.request_id,
                    Some(library_revision),
                    PortableFilePreviewDto {
                        schema_version: 1,
                        title,
                        byte_length,
                        backup_summary: Some(summary),
                        preview_id,
                        digest,
                        current_revision: None,
                        library_revision,
                    },
                    Some(cancellation),
                )
                .await;
                if result.is_err() {
                    custody.discard(&owner, &PreviewTarget::Preview { preview_id });
                }
                result
            },
        )
        .await
}

struct PreparedPublication {
    request_id: RequestId,
    operation_id: OperationId,
    preview_id: RequestId,
    binding: PreparedPortableBinding,
}

#[tauri::command]
pub(super) async fn project_export_create<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: ExportCreateRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    publish_prepared(
        webview,
        state,
        PreparedPublication {
            request_id: request.request_id,
            operation_id: request.operation_id,
            preview_id: request.preview_id,
            binding: PreparedPortableBinding::Scenario {
                scenario_id: request.scenario_id,
                expected_revision: request.expected_revision,
                expected_library_revision: request.expected_library_revision,
            },
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn project_backup_create<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: BackupCreateRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    publish_prepared(
        webview,
        state,
        PreparedPublication {
            request_id: request.request_id,
            operation_id: request.operation_id,
            preview_id: request.preview_id,
            binding: PreparedPortableBinding::Backup {
                expected_library_revision: request.expected_library_revision,
            },
        },
        on_progress,
    )
    .await
}

fn publication_admission(
    request: &PreparedPublication,
) -> (OperationClaim, ReviewBinding, Revision, Revision) {
    match &request.binding {
        PreparedPortableBinding::Scenario {
            scenario_id,
            expected_revision,
            expected_library_revision,
        } => (
            crate::setup_boundary::claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::ProjectExportCreate,
                *scenario_id,
                Some(*expected_revision),
            ),
            ReviewBinding::ScenarioExport {
                scenario_id: *scenario_id,
                scenario_revision: *expected_revision,
                library_revision: *expected_library_revision,
            },
            *expected_revision,
            *expected_library_revision,
        ),
        PreparedPortableBinding::Backup {
            expected_library_revision,
        } => (
            library_claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::ProjectBackupCreate,
                Some(*expected_library_revision),
            ),
            ReviewBinding::Backup {
                library_revision: *expected_library_revision,
            },
            *expected_library_revision,
            *expected_library_revision,
        ),
    }
}

async fn publish_prepared<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: PreparedPublication,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let owner = webview.window().label().to_owned();
    let handle = webview.app_handle().clone();
    let sink = channel(webview, on_progress)?;
    let custody = Arc::clone(&state.portable);
    let app = state.app.clone();
    let (claim, reviewed, revision, library_revision) = publication_admission(&request);
    state
        .operations
        .run(
            &owner.clone(),
            claim,
            Some(sink),
            OperationPhaseV1::SelectingFile,
            (request, None),
            move |request, mut execution| async move {
                let mut lease = custody.acquire(&owner, request.preview_id, reviewed)?;
                let output = lease.take_output()?;
                let (title, fallback, dialog_title) = match &output.kind {
                    PreparedPortableKind::Scenario { title, .. } => {
                        (title, "Eutheto-Export", "Save Eutheto export")
                    }
                    PreparedPortableKind::Backup { title, .. } => {
                        (title, "Eutheto-Backup", "Save Eutheto backup")
                    }
                };
                let cancellation = execution.cancellation();
                let (destination, artifact_name) = save_selection(
                    handle,
                    dialog_title,
                    suggested_portable_filename(title, fallback),
                    cancellation.clone(),
                )
                .await?;
                execution.report(OperationPhaseV1::PublishingFile);
                let written = app
                    .execute(AppCommand::PublishPreparedPortable {
                        destination,
                        bytes: output.bytes,
                        expected_sha256: output.sha256,
                        binding: request.binding,
                        cancellation,
                    })
                    .await
                    .map_err(map_app_error)?;
                if !matches!(written, AppCommandResult::BundleWritten) {
                    return Err(prepared_output_error("protocol.result_mismatch").into());
                }
                let result = finish(
                    &mut execution,
                    request.request_id,
                    Some(revision),
                    PortableArtifactDto {
                        schema_version: 1,
                        current_revision: match reviewed {
                            ReviewBinding::ScenarioExport {
                                scenario_revision, ..
                            } => Some(scenario_revision),
                            _ => None,
                        },
                        library_revision: Some(library_revision),
                        artifact_name,
                    },
                    None,
                )
                .await;
                drop(lease);
                result
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn project_unopened_bundle_reexport<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: UnopenedReexportRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    let owner = webview.window().label().to_owned();
    let handle = webview.app_handle().clone();
    let sink = channel(webview, on_progress)?;
    let custody = Arc::clone(&state.portable);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            library_claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::ProjectUnopenedBundleReexport,
                None,
            ),
            Some(sink),
            OperationPhaseV1::SelectingFile,
            (request, None),
            move |request, mut execution| async move {
                let lease = custody.acquire(&owner, request.preview_id, ReviewBinding::Unopened)?;
                let cancellation = execution.cancellation();
                let (destination, artifact_name) = save_selection(
                    handle,
                    "Save exact unopened Eutheto bundle",
                    "Eutheto-Unopened.eutheto".to_owned(),
                    cancellation.clone(),
                )
                .await?;
                execution.report(OperationPhaseV1::PublishingFile);
                let written = app
                    .execute(AppCommand::ExactReexportUnopenedBundle {
                        preview_id: request.preview_id,
                        destination,
                        cancellation,
                    })
                    .await
                    .map_err(map_app_error)?;
                if !matches!(written, AppCommandResult::UnopenedBundleReexported) {
                    return Err(prepared_output_error("protocol.result_mismatch").into());
                }
                let result = finish(
                    &mut execution,
                    request.request_id,
                    None,
                    PortableArtifactDto {
                        schema_version: 1,
                        current_revision: None,
                        library_revision: None,
                        artifact_name,
                    },
                    None,
                )
                .await;
                drop(lease);
                result
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn project_operation_cancel<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: DiscardRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    let owner = webview.window().label().to_owned();
    if let PreviewTarget::Creator { operation_id, .. } = &request.target {
        // The owner-scoped operation may still be waiting to reserve custody.
        state.operations.cancel(
            &owner,
            &OperationControlRequestV1 {
                schema_version: 1,
                request_id: request.request_id,
                operation_id: *operation_id,
            },
        )?;
    }
    state.portable.discard(&owner, &request.target);
    encode_response(
        request.request_id,
        None,
        Discarded { schema_version: 1 },
        64 * 1024,
        None,
    )
    .await
}

#[cfg(test)]
#[path = "portable/boundary_tests.rs"]
mod boundary_tests;
