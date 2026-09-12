//! Native file grants and reviewed settings custody; settings authority remains in core.
mod custody;

use crate::operations::{
    OperationClaim, OperationContextV1, OperationPhaseV1, OperationPurposeV1, ProgressSink,
    require_version,
};
use crate::setup_boundary::{decode, encode_response, finish, progress};
use crate::{
    ApiError, DesktopState, NativeFileError, SolutionApiResult, boundary_error, map_app_error,
    native_file_task,
};
pub(crate) use custody::SettingsCustody;
use custody::{Creator, PreviewTarget};
use eutheto_core::bounded_json_size;
use eutheto_types::{
    MAX_APPLICATION_SETTINGS_BYTES, OperationControl, OperationId, RequestId, Revision,
    SettingsImportApplyDtoV1, SettingsImportApplyRequestV1, SettingsImportPreviewDtoV1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{str::FromStr, sync::Arc};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const FRAME_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ImportRequest {
    Preview {
        schema_version: u32,
        request_id: RequestId,
        operation_id: OperationId,
    },
    Apply {
        schema_version: u32,
        request_id: RequestId,
        operation_id: OperationId,
        preview_id: RequestId,
        approval_sha256: String,
        expected_library_revision: Revision,
    },
    Discard {
        schema_version: u32,
        request_id: RequestId,
        target: PreviewTarget,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum ImportResult {
    Preview(SettingsImportPreviewDtoV1),
    Applied(SettingsImportApplyDtoV1),
    Discarded { schema_version: u32 },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    schema_version: u32,
    library_revision: Revision,
    written_bytes: usize,
}

fn bounded_request(request: Option<Value>) -> Result<Option<Value>, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, FRAME_BYTES).is_err())
    {
        return Err(boundary_error(
            "settings.request_too_large",
            "The settings request exceeds its native admission limit.",
            None,
        )
        .into());
    }
    Ok(request)
}

fn channel<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    value: Option<Value>,
) -> Result<ProgressSink, ApiError> {
    let invalid = || {
        boundary_error(
            "operation.progress_channel_invalid",
            "This operation requires a valid progress channel.",
            Some("/onProgress"),
        )
    };
    let Some(Value::String(value)) = value else {
        return Err(invalid().into());
    };
    if value.len() > 64 {
        return Err(invalid().into());
    }
    let id = tauri::ipc::JavaScriptChannelId::from_str(&value).map_err(|_| invalid())?;
    Ok(progress(id.channel_on(webview)))
}

fn claim(
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

#[tauri::command]
pub(super) async fn settings_import_nonsecret<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: ImportRequest = decode(bounded_request(request)?)?;
    let owner = webview.window().label().to_owned();
    match request {
        ImportRequest::Discard {
            schema_version,
            request_id,
            target,
        } => {
            require_version(schema_version)?;
            // Lifecycle cleanup neither claims heavy capacity nor requires a channel.
            state.settings.discard(&owner, &target);
            encode_response(
                request_id,
                None,
                ImportResult::Discarded { schema_version: 1 },
                FRAME_BYTES,
                None,
            )
            .await
        }
        ImportRequest::Preview {
            schema_version,
            request_id,
            operation_id,
        } => {
            require_version(schema_version)?;
            preview_import(webview, state, owner, request_id, operation_id, on_progress).await
        }
        ImportRequest::Apply {
            schema_version,
            request_id,
            operation_id,
            preview_id,
            approval_sha256,
            expected_library_revision,
        } => {
            require_version(schema_version)?;
            let sink = channel(webview, on_progress)?;
            let custody = Arc::clone(&state.settings);
            let app = state.app.clone();
            state
                .operations
                .run(
                    &owner.clone(),
                    claim(
                        operation_id,
                        request_id,
                        OperationPurposeV1::SettingsImportApply,
                        Some(expected_library_revision),
                    ),
                    Some(sink),
                    OperationPhaseV1::ApplyingPreview,
                    (
                        SettingsImportApplyRequestV1 {
                            schema_version,
                            request_id,
                            preview_id,
                            approval_sha256,
                            expected_library_revision,
                        },
                        None,
                    ),
                    move |request, mut execution| async move {
                        let lease = custody.acquire_apply(
                            &owner,
                            request.preview_id,
                            &request.approval_sha256,
                            request.expected_library_revision,
                        )?;
                        let applied = app
                            .apply_nonsecret_settings(request, execution.cancellation())
                            .await
                            .map_err(map_app_error)?;
                        // Core owns the commit decision. A late cancellation cannot relabel success.
                        let result = finish(
                            &mut execution,
                            request_id,
                            Some(applied.library_revision),
                            ImportResult::Applied(applied),
                            FRAME_BYTES,
                            None,
                        )
                        .await;
                        drop(lease);
                        result
                    },
                )
                .await
        }
    }
}

async fn preview_import<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    owner: String,
    request_id: RequestId,
    operation_id: OperationId,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let handle = webview.app_handle().clone();
    let sink = channel(webview, on_progress)?;
    let custody = Arc::clone(&state.settings);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            claim(
                operation_id,
                request_id,
                OperationPurposeV1::SettingsImportPreview,
                None,
            ),
            Some(sink),
            OperationPhaseV1::SelectingFile,
            ((), None),
            move |(), mut execution| async move {
                let token = execution.cancellation();
                let reservation = custody.reserve(
                    &owner,
                    Creator {
                        operation_id,
                        request_id,
                    },
                    token.clone(),
                )?;
                let picker_token = token.clone();
                let bytes = native_file_task(move || {
                    if picker_token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let selected = handle
                        .dialog()
                        .file()
                        .set_title("Choose nonsecret application settings")
                        .add_filter("Application settings JSON", &["json"])
                        .blocking_pick_file()
                        .ok_or(NativeFileError::Cancelled)?;
                    if picker_token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let path = selected
                        .into_path()
                        .map_err(|_| NativeFileError::Conversion)?;
                    crate::native_file::read_bounded_file(
                        &path,
                        MAX_APPLICATION_SETTINGS_BYTES,
                        &picker_token,
                    )
                })
                .await?;
                execution.report(OperationPhaseV1::CapturingSnapshot);
                let preview = app
                    .preview_nonsecret_settings(bytes, token.clone())
                    .await
                    .map_err(map_app_error)?;
                reservation.publish(&preview)?;
                let preview_id = preview.preview_id;
                let revision = preview.library_revision;
                let result = finish(
                    &mut execution,
                    request_id,
                    Some(revision),
                    ImportResult::Preview(preview),
                    FRAME_BYTES,
                    Some(token),
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
pub(super) async fn settings_export_nonsecret<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: Option<Value>,
) -> SolutionApiResult {
    let request: ExportRequest = decode(bounded_request(request)?)?;
    require_version(request.schema_version)?;
    let owner = webview.window().label().to_owned();
    let handle = webview.app_handle().clone();
    let sink = channel(webview, on_progress)?;
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner,
            claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::SettingsExport,
                None,
            ),
            Some(sink),
            OperationPhaseV1::SelectingFile,
            (request, None),
            move |request, mut execution| async move {
                let token = execution.cancellation();
                let picker_token = token.clone();
                let destination = native_file_task(move || {
                    if picker_token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let selected = handle
                        .dialog()
                        .file()
                        .set_title("Save nonsecret application settings")
                        .set_file_name("eutheto-settings.json")
                        .add_filter("Application settings JSON", &["json"])
                        .blocking_save_file()
                        .ok_or(NativeFileError::Cancelled)?;
                    if picker_token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    selected
                        .into_path()
                        .map_err(|_| NativeFileError::Conversion)
                })
                .await?;
                execution.report(OperationPhaseV1::CapturingSnapshot);
                let snapshot = app
                    .export_nonsecret_settings(token.clone())
                    .await
                    .map_err(map_app_error)?;
                let library_revision = snapshot.library_revision;
                let written_bytes = snapshot.byte_count;
                execution.report(OperationPhaseV1::PublishingFile);
                tauri::async_runtime::spawn_blocking(move || {
                    let control = OperationControl::Cancellation(token);
                    eutheto_export::prepare_json_atomic_controlled(
                        &destination,
                        &snapshot.document,
                        &control,
                    )
                    .and_then(|prepared| prepared.publish_controlled(&control))
                    .map_err(|error| ApiError::from(publication_error(&error)))
                })
                .await
                .map_err(|_| {
                    boundary_error(
                        "settings.publication_failed",
                        "The settings publication task could not finish.",
                        None,
                    )
                })??;
                finish(
                    &mut execution,
                    request.request_id,
                    Some(library_revision),
                    ExportResult {
                        schema_version: 1,
                        library_revision,
                        written_bytes,
                    },
                    FRAME_BYTES,
                    None,
                )
                .await
            },
        )
        .await
}

fn publication_error(error: &eutheto_export::ExportError) -> eutheto_types::ApiErrorDto {
    let (code, message) = match error {
        eutheto_export::ExportError::Cancelled => {
            ("operation.cancelled", "Settings publication was cancelled.")
        }
        eutheto_export::ExportError::DestinationExists(_) => (
            "settings.destination_exists",
            "The destination already exists. Choose a new settings filename.",
        ),
        _ => (
            "settings.publication_failed",
            "The settings snapshot could not be saved.",
        ),
    };
    boundary_error(code, message, None)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
