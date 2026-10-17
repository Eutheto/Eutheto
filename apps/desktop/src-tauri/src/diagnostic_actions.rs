use crate::operations::{OperationClaim, OperationPhaseV1, OperationPurposeV1, require_version};
use crate::setup_boundary::{claim, decode, finish, progress};
use crate::{
    ApiError, DesktopState, NativeFileError, NativeOperationRequest, SolutionApiResult,
    boundary_error, map_app_error, native_file_task,
};
use eutheto_core::{
    AppCommand, AppCommandResult, DiagnosticCopyRequestV1, DiagnosticExportRequestV1,
};
use eutheto_types::{OperationControl, Revision};
use serde::de::DeserializeOwned;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

pub(super) const REQUEST_MAX_BYTES: usize = 64 * 1024;
const RESPONSE_COMPACT_MAX_BYTES: usize = 136 * 1024;

fn decode_diagnostic_request<T: DeserializeOwned>(
    request: NativeOperationRequest,
) -> Result<T, ApiError> {
    decode(request.0)
}

fn validate_checksum(checksum: &str, field: &'static str) -> Result<(), ApiError> {
    if checksum.len() == 64 && checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(boundary_error(
            "diagnostic.reference_invalid",
            "The diagnostic authority checksum is invalid.",
            Some(field),
        )
        .into())
    }
}

fn operation_claim(
    operation_id: eutheto_types::OperationId,
    request_id: eutheto_types::RequestId,
    purpose: OperationPurposeV1,
    scenario_id: eutheto_types::ScenarioId,
    source_revision: Revision,
) -> OperationClaim {
    claim(
        operation_id,
        request_id,
        purpose,
        scenario_id,
        Some(source_revision),
    )
}

#[tauri::command]
pub(super) async fn solution_create_diagnostic_copy<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: DiagnosticCopyRequestV1 = decode_diagnostic_request(request)?;
    require_version(request.schema_version)?;
    validate_checksum(&request.run_manifest_checksum, "/runManifestChecksum")?;
    validate_checksum(&request.evidence_checksum, "/evidenceChecksum")?;
    let operation = operation_claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::SolutionDiagnosticCopy,
        request.scenario_id,
        request.source_revision,
    );
    let app = state.app.clone();
    state
        .operations
        .run(
            window.label(),
            operation,
            Some(progress(on_progress)),
            OperationPhaseV1::Validating,
            (request, None),
            move |request, mut execution| async move {
                let request_id = request.request_id;
                let result = app
                    .execute(AppCommand::CreateDiagnosticCopy {
                        request,
                        control: OperationControl::Cancellation(execution.cancellation()),
                    })
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let AppCommandResult::DiagnosticCopy(receipt) = result else {
                    return Err(Box::new(boundary_error(
                        "protocol.result_mismatch",
                        "The application returned an unexpected diagnostic-copy result.",
                        None,
                    )));
                };
                finish(
                    &mut execution,
                    request_id,
                    None,
                    receipt,
                    RESPONSE_COMPACT_MAX_BYTES,
                    None,
                )
                .await
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn solution_export_diagnostic<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: DiagnosticExportRequestV1 = decode_diagnostic_request(request)?;
    require_version(request.schema_version)?;
    validate_checksum(&request.run_manifest_checksum, "/runManifestChecksum")?;
    validate_checksum(&request.evidence_checksum, "/evidenceChecksum")?;
    let operation = operation_claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::SolutionDiagnosticExport,
        request.scenario_id,
        request.source_revision,
    );
    let owner = window.label().to_owned();
    let handle = window.app_handle().clone();
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner,
            operation,
            Some(progress(on_progress)),
            OperationPhaseV1::CapturingSnapshot,
            (request, None),
            move |request, mut execution| async move {
                let request_id = request.request_id;
                let cancellation = execution.cancellation();
                let captured = app
                    .prepare_diagnostic_export(
                        request,
                        OperationControl::Cancellation(cancellation.clone()),
                    )
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                captured
                    .admit()
                    .map_err(|error| Box::new(map_app_error(error)))?;

                execution.report(OperationPhaseV1::SelectingFile);
                let picker_cancellation = cancellation.clone();
                let selected = native_file_task(move || {
                    if picker_cancellation.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let selected = handle
                        .dialog()
                        .file()
                        .set_title("Export diagnostic report")
                        .set_file_name("eutheto-infeasibility-report.json")
                        .add_filter("JSON report", &["json"])
                        .blocking_save_file()
                        .ok_or(NativeFileError::Cancelled)?;
                    if picker_cancellation.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    selected
                        .into_path()
                        .map_err(|_| NativeFileError::Conversion)
                })
                .await;
                let destination = match selected {
                    Ok(destination) => destination,
                    Err(error) => {
                        captured
                            .admit()
                            .map_err(|error| Box::new(map_app_error(error)))?;
                        return Err(error);
                    }
                };

                execution.report(OperationPhaseV1::PublishingFile);
                let metadata = tauri::async_runtime::spawn_blocking(move || {
                    captured
                        .prepare_destination(&destination)?
                        .publish_controlled()
                })
                .await
                .map_err(|_| {
                    Box::new(boundary_error(
                        "operation.execution_failed",
                        "The diagnostic report could not be published safely.",
                        None,
                    ))
                })?
                .map_err(|error| Box::new(map_app_error(error)))?;
                finish(
                    &mut execution,
                    request_id,
                    Some(metadata.observed_current_revision),
                    metadata,
                    RESPONSE_COMPACT_MAX_BYTES,
                    None,
                )
                .await
            },
        )
        .await
}

#[cfg(test)]
#[path = "diagnostic_actions_tests.rs"]
mod tests;
