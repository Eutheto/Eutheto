//! Window-owned CSV/calendar review; publication consumes only Core's retained artifact.
use crate::operations::{
    OperationCancelledV1, OperationControlRequestV1, OperationPhaseV1, OperationPurposeV1,
    require_version,
};
use crate::recipient_report::{
    RecipientCreatedResponse, RecipientPreviewResponse, RecipientReleasedDtoV1, check_control,
    custody::{CaptureFormat, CaptureKind, CapturedReport, ReleaseAcknowledgement, Scope, Target},
    encode_created, encode_preview, select_destination, take_control,
};
use crate::setup_boundary::{claim, decode, progress, response_wire_limit};
use crate::{
    ApiError, ApiResult, DesktopState, NativeOperationRequest, boundary_error, map_app_error,
    response,
};
use eutheto_core::{
    CapturedScheduleExport, ScheduleExportFormatV1, ScheduleExportPreviewV1,
    ScheduleExportPublishedV1, ScheduleExportRequestV1,
};
use eutheto_types::{
    DurationMillis, OperationControl, OperationId, PersonId, RequestId, Revision, ScenarioId,
    SolutionId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{Manager, State};

pub(super) const FRAME_BYTES: usize = 64 * 1024;
const PREVIEW_COMPACT_BYTES: usize = 64 * 1024;
const OPERATION_MILLISECONDS: u64 = 30_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewRequest {
    request_id: RequestId,
    schema_version: u32,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    solution_id: SolutionId,
    format: ScheduleExportFormatV1,
    person_id: Option<PersonId>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleExportPreviewDtoV1 {
    schema_version: u32,
    preview_id: RequestId,
    expires_after_milliseconds: u64,
    preview: ScheduleExportPreviewV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    request_id: RequestId,
    schema_version: u32,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    preview_id: RequestId,
    format: ScheduleExportFormatV1,
    allow_stale: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleExportCreatedDtoV1 {
    schema_version: u32,
    preview_id: RequestId,
    filename: String,
    publication: ScheduleExportPublishedV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseRequest {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    target: Target,
}

#[tauri::command]
pub(super) async fn solution_export_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> Result<RecipientPreviewResponse, ApiError> {
    let request: PreviewRequest = decode(request.0)?;
    require_version(request.schema_version)?;
    let scope = Scope {
        scenario_id: request.scenario_id,
        expected_revision: request.expected_revision,
        kind: CaptureKind::Schedule,
    };
    let reservation = state
        .recipient_reports
        .begin(window.label(), request.operation_id, scope)?;
    let operation = claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::ExportPreview,
        request.scenario_id,
        Some(request.expected_revision),
    );
    let app = state.app.clone();
    state
        .operations
        .run_with_mutation_budget(
            window.label(),
            operation,
            Some(progress(on_progress)),
            OperationPhaseV1::CapturingSnapshot,
            ((request, reservation), None),
            DurationMillis::new(OPERATION_MILLISECONDS)
                .map_err(|_| Box::new(crate::operations::deadline_exceeded()))?,
            move |(request, mut reservation), mut execution| async move {
                let control = take_control(&mut execution)?;
                let (capture, preview) = app
                    .prepare_schedule_export(
                        ScheduleExportRequestV1 {
                            schema_version: 1,
                            scenario_id: request.scenario_id,
                            solution_id: request.solution_id,
                            expected_revision: request.expected_revision,
                            format: request.format,
                            person_id: request.person_id,
                        },
                        control.clone(),
                    )
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let charged_bytes = preview_charge(&capture, &preview, &control)?;
                let preview_id =
                    reservation.stage(CapturedReport::Schedule(capture), charged_bytes)?;
                let result = ScheduleExportPreviewDtoV1 {
                    schema_version: 1,
                    preview_id,
                    expires_after_milliseconds: 600_000,
                    preview,
                };
                execution.preparing_response();
                encode_preview(
                    request.request_id,
                    request.expected_revision,
                    result,
                    reservation,
                    control,
                    PREVIEW_COMPACT_BYTES,
                )
                .await
            },
        )
        .await
}

fn preview_charge(
    capture: &CapturedScheduleExport,
    preview: &ScheduleExportPreviewV1,
    control: &OperationControl,
) -> Result<usize, ApiError> {
    let captured = capture
        .retained_bytes(control)
        .map_err(|error| Box::new(map_app_error(error)))?;
    let result = preview
        .retained_bytes()
        .and_then(|bytes| bytes.checked_add(captured))
        .and_then(|bytes| {
            response_wire_limit(PREVIEW_COMPACT_BYTES)
                .checked_mul(4)
                .and_then(|wire| bytes.checked_add(wire))
        })
        .and_then(|bytes| bytes.checked_add(FRAME_BYTES))
        .ok_or_else(|| {
            Box::new(boundary_error(
                "recipient.preview_capacity",
                "The export preview exceeds its retained resource limit.",
                None,
            ))
        })
        .and_then(|bytes| {
            check_control(control)?;
            Ok(bytes)
        });
    capture
        .admit()
        .map_err(|error| Box::new(map_app_error(error)))?;
    result
}

#[tauri::command]
pub(super) async fn solution_export<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> Result<RecipientCreatedResponse, ApiError> {
    let request: CreateRequest = decode(request.0)?;
    require_version(request.schema_version)?;
    let time_limit = DurationMillis::new(OPERATION_MILLISECONDS)
        .map_err(|_| Box::new(crate::operations::deadline_exceeded()))?;
    let consuming = Arc::new(state.recipient_reports.consume(
        window.label(),
        request.preview_id,
        Scope {
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            kind: CaptureKind::Schedule,
        },
        CaptureFormat::Schedule(request.format),
    )?);
    let readmission = Arc::clone(&consuming);
    let operation = claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::ExportCreate,
        request.scenario_id,
        Some(request.expected_revision),
    );
    let handle = window.app_handle().clone();
    let outcome = state
        .operations
        .run_with_mutation_budget(
            window.label(),
            operation,
            Some(progress(on_progress)),
            OperationPhaseV1::SelectingFile,
            ((request, consuming), None),
            time_limit,
            move |(request, consuming), mut execution| async move {
                let control = take_control(&mut execution)?;
                consuming.admit()?;
                let source_revision = consuming.source_revision()?;
                let (prefix, extension) = match request.format {
                    ScheduleExportFormatV1::AssignmentsCsv => ("assignments", "csv"),
                    ScheduleExportFormatV1::SummaryCsv => ("people-shift-summary", "csv"),
                    ScheduleExportFormatV1::Ics => ("schedule", "ics"),
                };
                let historical = if source_revision == request.expected_revision {
                    ""
                } else {
                    "-historical"
                };
                let suggested = format!(
                    "{prefix}-revision-{}{historical}.{extension}",
                    source_revision.value()
                );
                let selected = select_destination(
                    handle,
                    control.clone(),
                    CaptureFormat::Schedule(request.format),
                    suggested,
                )
                .await;
                consuming.admit()?;
                let (destination, filename) = selected?;
                execution.report(OperationPhaseV1::PublishingFile);
                let capture = consuming.take_capture()?.into_schedule()?;
                let publication = capture
                    .publish(destination, request.allow_stale, control)
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let revision = publication.current_revision;
                let result = ScheduleExportCreatedDtoV1 {
                    schema_version: 1,
                    preview_id: request.preview_id,
                    filename,
                    publication,
                };
                execution.preparing_response();
                encode_created(request.request_id, revision, result, consuming).await
            },
        )
        .await;
    if outcome.is_err() {
        readmission.admit_if_retained()?;
    }
    outcome
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn solution_export_cancel<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
) -> ApiResult<OperationCancelledV1> {
    let request: OperationControlRequestV1 = decode(request.0)?;
    let result = state.operations.cancel_export(window.label(), &request)?;
    Ok(response(request.request_id, None, Vec::new(), result))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn solution_export_release<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
) -> ApiResult<RecipientReleasedDtoV1> {
    let request: ReleaseRequest = decode(request.0)?;
    require_version(request.schema_version)?;
    let acknowledgement: ReleaseAcknowledgement = state.recipient_reports.release(
        window.label(),
        request.scenario_id,
        CaptureKind::Schedule,
        request.target,
    );
    Ok(response(
        request.request_id,
        None,
        Vec::new(),
        RecipientReleasedDtoV1 {
            schema_version: 1,
            acknowledgement,
        },
    ))
}
