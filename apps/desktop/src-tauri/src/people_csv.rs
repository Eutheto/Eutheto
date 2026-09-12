//! Native CSV grants and window custody; parsing and mutation authority stay in core.
mod custody;

use crate::operations::{OperationPhaseV1, OperationPurposeV1, require_version};
use crate::setup_boundary::{claim, decode, finish, progress};
use crate::{
    ApiError, ApiResult, DesktopState, NativeFileError, SolutionApiResult, boundary_error,
    map_app_error, native_file_task, response,
};
pub(crate) use custody::CsvCustody;
use custody::{Creator, PreviewTarget, SourceId, SourceTarget};
use eutheto_core::{
    MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS, MAX_CSV_MAPPING_BYTES, MAX_CSV_SOURCE_BYTES,
    PeopleCsvApplyRequestV1, PeopleCsvMapping, PeopleCsvOperation, PeopleCsvPreviewRequestV1,
    PeopleCsvRejectedRowsDtoV1, PeopleImportPreview, RowDecision, bounded_json_size,
};
use eutheto_types::{
    ActorRef, ApiErrorDto, CancellationToken, CommandId, CommandSource, OperationControl,
    OperationId, RequestId, Revision, ScenarioId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const FRAME_BYTES: usize = 64 * 1024;
const DETECTION_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 16 * 1024 * 1024;

// Tauri has already materialized Value. Bound further typed allocation and queued retention;
// these checks do not claim to bound the transport's initial JSON allocation.
fn bounded_request(
    request: Option<Value>,
    maximum_bytes: usize,
) -> Result<Option<Value>, ApiError> {
    if request
        .as_ref()
        .is_some_and(|value| bounded_json_size(value, maximum_bytes).is_err())
    {
        return Err(request_too_large().into());
    }
    Ok(request)
}

fn bounded_preview_request(request: Option<Value>) -> Result<Option<Value>, ApiError> {
    if let Some(value) = &request {
        let mapping_exceeded = value
            .get("mapping")
            .is_some_and(|mapping| bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES).is_err());
        let decisions_exceeded = value.get("decisions").is_some_and(|decisions| {
            decisions
                .as_array()
                .is_some_and(|rows| rows.len() > MAX_CSV_DECISIONS)
                || bounded_json_size(decisions, MAX_CSV_DECISION_BYTES).is_err()
        });
        if mapping_exceeded || decisions_exceeded {
            return Err(request_too_large().into());
        }
    }
    bounded_request(
        request,
        MAX_CSV_MAPPING_BYTES + MAX_CSV_DECISION_BYTES + FRAME_BYTES,
    )
}

fn request_too_large() -> ApiErrorDto {
    boundary_error(
        "people_csv.request_too_large",
        "The CSV request exceeds its native admission limits.",
        None,
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceOpenRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceWorkRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    mapping: PeopleCsvMapping,
    decisions: Vec<RowDecision>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    preview_id: RequestId,
    approved_digest: String,
    command_id: CommandId,
    actor: ActorRef,
    truncate_redo: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceCloseRequest {
    schema_version: u32,
    request_id: RequestId,
    scenario_id: ScenarioId,
    target: SourceTarget,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewDiscardRequest {
    schema_version: u32,
    request_id: RequestId,
    scenario_id: ScenarioId,
    target: PreviewTarget,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReportRequest {
    schema_version: u32,
    request_id: RequestId,
    scenario_id: ScenarioId,
    preview_id: RequestId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReportSaveRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    preview_id: RequestId,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceOpened {
    schema_version: u32,
    source_id: SourceId,
    byte_count: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResult {
    schema_version: u32,
    source_id: SourceId,
    scenario_id: ScenarioId,
    revision: Revision,
    preview_id: RequestId,
    preview: PeopleImportPreview,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyResult {
    schema_version: u32,
    source_id: SourceId,
    scenario_id: ScenarioId,
    outcome: eutheto_core::PeopleCsvApplyOutcomeV1,
    report: PeopleCsvRejectedRowsDtoV1,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CustodyClosed {
    schema_version: u32,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportSaved {
    schema_version: u32,
    preview_id: RequestId,
}

fn check_cancelled(token: &CancellationToken) -> Result<(), ApiError> {
    if token.is_cancelled() {
        Err(boundary_error("operation.cancelled", "The operation was cancelled.", None).into())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub(super) async fn people_csv_source_open<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: SourceOpenRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let owner = window.label().to_owned();
    let handle = window.app_handle().clone();
    let custody = Arc::clone(&state.csv);
    state
        .operations
        .run(
            &owner.clone(),
            claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::CsvSourceOpen,
                request.scenario_id,
                Some(request.expected_revision),
            ),
            Some(progress(on_progress)),
            OperationPhaseV1::SelectingFile,
            (request, None),
            move |request, mut execution| async move {
                let token = execution.cancellation();
                let creator = Creator {
                    operation_id: request.operation_id,
                    request_id: request.request_id,
                };
                let reservation =
                    custody.reserve_source(&owner, request.scenario_id, creator, token.clone())?;
                let source_id = native_file_task(move || {
                    if token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let selected = handle
                        .dialog()
                        .file()
                        .set_title("Choose people CSV snapshot")
                        .add_filter("People CSV", &["csv", "tsv", "txt"])
                        .blocking_pick_file()
                        .ok_or(NativeFileError::Cancelled)?;
                    if token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let path = selected
                        .into_path()
                        .map_err(|_| NativeFileError::Conversion)?;
                    crate::native_file::read_bounded_file(&path, MAX_CSV_SOURCE_BYTES, &token)
                })
                .await;
                let bytes = source_id?;
                let byte_count = bytes.len();
                let source_id = reservation.publish(bytes)?;
                let cancellation = Some(execution.cancellation());
                let result = finish(
                    &mut execution,
                    request.request_id,
                    None,
                    SourceOpened {
                        schema_version: 1,
                        source_id,
                        byte_count,
                    },
                    FRAME_BYTES,
                    cancellation,
                )
                .await;
                if result.is_err() {
                    custody.close_source(
                        &owner,
                        request.scenario_id,
                        &SourceTarget::Source { source_id },
                    );
                }
                result
            },
        )
        .await
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn people_csv_source_close<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<CustodyClosed> {
    let request: SourceCloseRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    state
        .csv
        .close_source(window.label(), request.scenario_id, &request.target);
    Ok(response(
        request.request_id,
        None,
        Vec::new(),
        CustodyClosed { schema_version: 1 },
    ))
}

#[tauri::command]
pub(super) async fn people_csv_detect<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: SourceWorkRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let owner = window.label().to_owned();
    let custody = Arc::clone(&state.csv);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::CsvDetect,
                request.scenario_id,
                Some(request.expected_revision),
            ),
            Some(progress(on_progress)),
            OperationPhaseV1::DetectingFormat,
            (request, None),
            move |request, mut execution| async move {
                let source = custody.source(&owner, request.scenario_id, request.source_id)?;
                let result = app
                    .detect_people_csv(
                        source.reader(),
                        PeopleCsvOperation::child_of(&execution.cancellation()),
                    )
                    .await
                    .map_err(map_app_error)?;
                let cancellation = Some(execution.cancellation());
                let result = finish(
                    &mut execution,
                    request.request_id,
                    None,
                    result,
                    DETECTION_BYTES + FRAME_BYTES,
                    cancellation,
                )
                .await;
                drop(source);
                result
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn people_csv_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: PreviewRequest = decode(bounded_preview_request(request)?)?;
    require_version(request.schema_version)?;
    let owner = window.label().to_owned();
    let custody = Arc::clone(&state.csv);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::CsvPreview,
                request.scenario_id,
                Some(request.expected_revision),
            ),
            Some(progress(on_progress)),
            OperationPhaseV1::ApplyingPreview,
            (request, None),
            move |request, mut execution| async move {
                let source = custody.source(&owner, request.scenario_id, request.source_id)?;
                let reservation = custody.reserve_preview(
                    &source,
                    request.expected_revision,
                    Creator {
                        operation_id: request.operation_id,
                        request_id: request.request_id,
                    },
                    execution.cancellation(),
                )?;
                let preview = app
                    .preview_people_csv(
                        PeopleCsvPreviewRequestV1 {
                            schema_version: 1,
                            scenario_id: request.scenario_id,
                            expected_revision: request.expected_revision,
                            mapping: request.mapping,
                            decisions: request.decisions,
                        },
                        source.reader(),
                        PeopleCsvOperation::child_of(&execution.cancellation()),
                    )
                    .await
                    .map_err(map_app_error)?;
                let preview_id = reservation.publish(preview.preview_id)?;
                let cancellation = Some(execution.cancellation());
                let result = finish(
                    &mut execution,
                    request.request_id,
                    Some(request.expected_revision),
                    PreviewResult {
                        schema_version: 1,
                        source_id: request.source_id,
                        scenario_id: request.scenario_id,
                        revision: request.expected_revision,
                        preview_id,
                        preview: preview.preview,
                    },
                    PREVIEW_BYTES + FRAME_BYTES,
                    cancellation,
                )
                .await;
                if result.is_err() {
                    custody.discard_preview(
                        &owner,
                        request.scenario_id,
                        &PreviewTarget::Preview { preview_id },
                    );
                }
                drop(source);
                result
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn people_csv_apply<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: ApplyRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let owner = window.label().to_owned();
    let custody = Arc::clone(&state.csv);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::CsvApply,
                request.scenario_id,
                Some(request.expected_revision),
            ),
            Some(progress(on_progress)),
            OperationPhaseV1::ApplyingPreview,
            (request, None),
            move |request, mut execution| async move {
                let source = custody.source(&owner, request.scenario_id, request.source_id)?;
                let preview = custody.preview_for_apply(
                    &owner,
                    request.scenario_id,
                    request.source_id,
                    request.expected_revision,
                    request.preview_id,
                )?;
                let applied = app
                    .apply_people_csv_import(
                        PeopleCsvApplyRequestV1 {
                            schema_version: 1,
                            preview_id: request.preview_id,
                            scenario_id: request.scenario_id,
                            expected_revision: request.expected_revision,
                            approved_digest: request.approved_digest,
                            command_id: request.command_id,
                            request_id: request.request_id,
                            actor: request.actor,
                            source: CommandSource::Desktop,
                            truncate_redo: request.truncate_redo,
                        },
                        source.reader(),
                        PeopleCsvOperation::child_of(&execution.cancellation()),
                    )
                    .await
                    .map_err(map_app_error)?;
                let revision = match applied.outcome {
                    eutheto_core::PeopleCsvApplyOutcomeV1::Applied { revision, .. }
                    | eutheto_core::PeopleCsvApplyOutcomeV1::NoChanges { revision } => revision,
                };
                // Never replace a committed/no-change receipt with a late cancellation or cleanup error.
                let result = finish(
                    &mut execution,
                    request.request_id,
                    Some(revision),
                    ApplyResult {
                        schema_version: 1,
                        source_id: request.source_id,
                        scenario_id: request.scenario_id,
                        outcome: applied.outcome,
                        report: applied.report,
                    },
                    DETECTION_BYTES + FRAME_BYTES,
                    None,
                )
                .await;
                custody.close_source(
                    &owner,
                    request.scenario_id,
                    &SourceTarget::Source {
                        source_id: request.source_id,
                    },
                );
                drop(preview);
                drop(source);
                result
            },
        )
        .await
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn people_csv_preview_discard<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<CustodyClosed> {
    let request: PreviewDiscardRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    state
        .csv
        .discard_preview(window.label(), request.scenario_id, &request.target);
    Ok(response(
        request.request_id,
        None,
        Vec::new(),
        CustodyClosed { schema_version: 1 },
    ))
}

#[tauri::command]
pub(super) async fn people_csv_rejected_rows<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<PeopleCsvRejectedRowsDtoV1> {
    let request: ReportRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let preview =
        state
            .csv
            .preview_for_report(window.label(), request.scenario_id, request.preview_id)?;
    let report = state
        .app
        .people_csv_rejected_rows(preview.preview_id())
        .await
        .map_err(map_app_error)?;
    Ok(response(request.request_id, None, Vec::new(), report))
}

fn publication_error(error: &eutheto_export::ExportError) -> ApiErrorDto {
    let (code, message) = match error {
        eutheto_export::ExportError::Cancelled => {
            ("operation.cancelled", "Report publication was cancelled.")
        }
        eutheto_export::ExportError::DestinationExists(_) => (
            "people_csv.destination_exists",
            "The destination already exists. Choose a new report filename.",
        ),
        _ => (
            "people_csv.publication_failed",
            "The rejected-row report could not be saved. The import outcome is unchanged.",
        ),
    };
    boundary_error(code, message, None)
}

#[tauri::command]
pub(super) async fn people_csv_rejected_rows_save<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: ReportSaveRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let owner = window.label().to_owned();
    let handle = window.app_handle().clone();
    let custody = Arc::clone(&state.csv);
    let app = state.app.clone();
    state
        .operations
        .run(
            &owner.clone(),
            claim(
                request.operation_id,
                request.request_id,
                OperationPurposeV1::CsvReportSave,
                request.scenario_id,
                Some(request.expected_revision),
            ),
            Some(progress(on_progress)),
            OperationPhaseV1::SelectingFile,
            (request, None),
            move |request, mut execution| async move {
                let preview =
                    custody.preview_for_report(&owner, request.scenario_id, request.preview_id)?;
                let report = app
                    .people_csv_rejected_rows(preview.preview_id())
                    .await
                    .map_err(map_app_error)?;
                let token = execution.cancellation();
                check_cancelled(&token)?;
                let picker_token = token.clone();
                let destination = native_file_task(move || {
                    if picker_token.is_cancelled() {
                        return Err(NativeFileError::Cancelled);
                    }
                    let selected = handle
                        .dialog()
                        .file()
                        .set_title("Save rejected people rows")
                        .set_file_name("people-import-rejected-rows.json")
                        .add_filter("JSON report", &["json"])
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
                execution.report(OperationPhaseV1::PublishingReport);
                tauri::async_runtime::spawn_blocking(move || {
                    let control = OperationControl::Cancellation(token);
                    eutheto_export::prepare_json_atomic_controlled(&destination, &report, &control)
                        .and_then(|prepared| prepared.publish_controlled(&control))
                        .map_err(|error| Box::new(publication_error(&error)))
                })
                .await
                .map_err(|_| {
                    boundary_error(
                        "people_csv.publication_failed",
                        "The report publication task could not finish.",
                        None,
                    )
                })??;
                let result = finish(
                    &mut execution,
                    request.request_id,
                    None,
                    ReportSaved {
                        schema_version: 1,
                        preview_id: request.preview_id,
                    },
                    FRAME_BYTES,
                    None,
                )
                .await;
                drop(preview);
                result
            },
        )
        .await
}

#[cfg(test)]
#[path = "people_csv_tests.rs"]
mod tests;
