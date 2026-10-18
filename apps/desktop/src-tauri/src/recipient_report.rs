//! Window-owned recipient approval. Only Core's captured bytes can be published.
use crate::operations::{
    OperationExecution, OperationPhaseV1, OperationPurposeV1, require_version,
};
use crate::setup_boundary::{claim, decode, progress, response_wire_limit};
use crate::{
    ApiError, ApiResult, DesktopState, NativeOperationRequest, boundary_error, map_app_error,
    response, selected_basename,
};
use eutheto_core::{RecipientHtmlPublishedV1, RecipientPdfPublishedV1, RecipientReportRequestV1};
use eutheto_types::{
    ApiErrorDto, DurationMillis, MAX_RECIPIENT_PDF_BYTES, MAX_RECIPIENT_REPORT_BYTES,
    OperationControl, OperationId, OperationInterruption, RecipientPdfRendererV1,
    RecipientReportOptionsV1, RecipientReportV1, RequestId, Revision, ScenarioId, SolutionId,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

pub(crate) mod custody;
use custody::{
    CaptureFormat, CaptureKind, CapturedReport, ConsumingReport, PreparingReport,
    ReleaseAcknowledgement, Scope, Target,
};

pub(super) const FRAME_BYTES: usize = 64 * 1024;
const PREVIEW_COMPACT_BYTES: usize = MAX_RECIPIENT_REPORT_BYTES + FRAME_BYTES;
const OPERATION_MILLISECONDS: u64 = 30_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RecipientFileFormatV1 {
    Html,
    Pdf,
}

impl RecipientFileFormatV1 {
    fn capture_format(self) -> CaptureFormat {
        match self {
            Self::Html => CaptureFormat::Html,
            Self::Pdf => CaptureFormat::Pdf,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewRequest {
    request_id: RequestId,
    schema_version: u32,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    solution_id: SolutionId,
    format: RecipientFileFormatV1,
    options: RecipientReportOptionsV1,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipientPreviewDtoV1 {
    schema_version: u32,
    preview_id: RequestId,
    format: RecipientFileFormatV1,
    library_revision: Revision,
    model_sha256: String,
    html_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pdf_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pdf_renderer: Option<RecipientPdfRendererV1>,
    expires_after_milliseconds: u64,
    report: RecipientReportV1,
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
    format: RecipientFileFormatV1,
    allow_stale: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipientCreatedDtoV1 {
    schema_version: u32,
    preview_id: RequestId,
    format: RecipientFileFormatV1,
    filename: String,
    publication: RecipientPublicationV1,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RecipientPublicationV1 {
    Html(RecipientHtmlPublishedV1),
    Pdf(RecipientPdfPublishedV1),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseRequest {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    target: Target,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RecipientReleasedDtoV1 {
    pub(super) schema_version: u32,
    pub(super) acknowledgement: ReleaseAcknowledgement,
}

/// Field order is intentional: abandoned replies free bytes before retiring their charge.
/// Do not derive Serialize: Tauri must move this exact response without re-encoding it.
pub(super) struct RecipientPreviewResponse {
    response: tauri::ipc::Response,
    reservation: PreparingReport,
}

impl tauri::ipc::IpcResponse for RecipientPreviewResponse {
    fn body(self) -> tauri::Result<tauri::ipc::InvokeResponseBody> {
        let body = tauri::ipc::IpcResponse::body(self.response)?;
        // Pinned Tauri2.11.5 Response::body moves its allocation without copying. From
        // this point the transport owns it; this is not a delivery acknowledgement.
        self.reservation.complete_response();
        Ok(body)
    }
}

/// The small confirmed receipt also stays charged until transport takes ownership.
pub(super) struct RecipientCreatedResponse {
    response: tauri::ipc::Response,
    consuming: Arc<ConsumingReport>,
}

impl tauri::ipc::IpcResponse for RecipientCreatedResponse {
    fn body(self) -> tauri::Result<tauri::ipc::InvokeResponseBody> {
        let body = tauri::ipc::IpcResponse::body(self.response)?;
        drop(self.consuming);
        Ok(body)
    }
}

#[tauri::command]
pub(super) async fn solution_share_preview<R: tauri::Runtime>(
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
        kind: CaptureKind::Recipient,
    };
    let reservation = state
        .recipient_reports
        .begin(window.label(), request.operation_id, scope)?;
    let operation = claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::SharePreview,
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
                let (capture, report) = app
                    .prepare_recipient_report(
                        RecipientReportRequestV1 {
                            schema_version: 1,
                            scenario_id: request.scenario_id,
                            solution_id: request.solution_id,
                            expected_revision: request.expected_revision,
                            options: request.options,
                        },
                        control.clone(),
                    )
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let library_revision = capture.metadata().library_revision;
                let model_sha256 = capture.model_sha256().to_owned();
                let html_sha256 = capture.html_sha256().to_owned();
                let capture = CapturedReport::Recipient(capture);
                let (capture, pdf_sha256, pdf_renderer) =
                    if request.format == RecipientFileFormatV1::Pdf {
                        reserve_pdf_preparation(&mut reservation, &capture, &report, &control)?;
                        let pdf = capture
                            .into_recipient()?
                            .prepare_pdf(control.clone())
                            .await
                            .map_err(|error| Box::new(map_app_error(error)))?;
                        let digest = pdf.pdf_sha256().to_owned();
                        let renderer = pdf.renderer().clone();
                        (CapturedReport::Pdf(pdf), Some(digest), Some(renderer))
                    } else {
                        (capture, None, None)
                    };
                let charged_bytes = preview_charge(&capture, &report, &control)?;
                let preview_id = reservation.stage(capture, charged_bytes)?;
                let result = RecipientPreviewDtoV1 {
                    schema_version: 1,
                    preview_id,
                    format: request.format,
                    library_revision,
                    model_sha256,
                    html_sha256,
                    pdf_sha256,
                    pdf_renderer,
                    expires_after_milliseconds: 600_000,
                    report,
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

fn reserve_pdf_preparation(
    reservation: &mut PreparingReport,
    capture: &CapturedReport,
    report: &RecipientReportV1,
    control: &OperationControl,
) -> Result<(), ApiError> {
    let growth = preview_charge(capture, report, control)?
        .checked_add(MAX_RECIPIENT_PDF_BYTES + MAX_RECIPIENT_REPORT_BYTES + 4 * FRAME_BYTES)
        .ok_or_else(|| {
            Box::new(boundary_error(
                "recipient.preview_capacity",
                "The PDF review exceeds its retained resource limit.",
                None,
            ))
        })?;
    // Reserve PDF output and bounded model/I/O buffers before conversion;
    // the existing charge already includes source, HTML and response growth.
    let reserved = reservation.reserve_preparation(growth);
    capture
        .admit()
        .map_err(|error| Box::new(map_app_error(error)))?;
    reserved
}

fn preview_charge(
    capture: &CapturedReport,
    report: &RecipientReportV1,
    control: &OperationControl,
) -> Result<usize, ApiError> {
    let captured = capture
        .retained_bytes(control)
        .map_err(|error| Box::new(map_app_error(error)))?;
    // The bounded formatter's Vec can grow to <2×wire. A callback handoff can also
    // escape into at most2×wire; protocol-channel/custom-protocol moves cost less.
    // Keep the whole allowance until release, not just until encoding completes.
    let result = report
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
                "The recipient preview exceeds its retained resource limit.",
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

pub(super) async fn encode_preview<T: Serialize + Send + 'static>(
    request_id: RequestId,
    revision: Revision,
    result: T,
    mut reservation: PreparingReport,
    control: OperationControl,
    compact_bytes: usize,
) -> Result<RecipientPreviewResponse, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let envelope = response(request_id, Some(revision), Vec::new(), result);
        let encoded = check_control(&control).and_then(|()| {
            crate::setup_boundary::encode(&envelope, response_wire_limit(compact_bytes), None)
        });
        drop(envelope);
        let encoded = match encoded {
            Ok(encoded) => encoded,
            Err(error) => {
                reservation.admit()?;
                return Err(error);
            }
        };
        if let Err(error) = check_control(&control) {
            drop(encoded);
            reservation.admit()?;
            return Err(error);
        }
        if let Err(error) = reservation.ready() {
            drop(encoded);
            return Err(error);
        }
        Ok(RecipientPreviewResponse {
            response: encoded,
            reservation,
        })
    })
    .await
    .map_err(|_| {
        Box::new(boundary_error(
            "operation.execution_failed",
            "The recipient response could not finish safely.",
            None,
        ))
    })?
}

#[tauri::command]
pub(super) async fn solution_share_create<R: tauri::Runtime>(
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
            kind: CaptureKind::Recipient,
        },
        request.format.capture_format(),
    )?);
    let readmission = Arc::clone(&consuming);
    let operation = claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::ShareCreate,
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
                let selected = select_destination(
                    handle,
                    control.clone(),
                    request.format.capture_format(),
                    match request.format {
                        RecipientFileFormatV1::Html => "shared-plan.html",
                        RecipientFileFormatV1::Pdf => "shared-plan.pdf",
                    }
                    .to_owned(),
                )
                .await;
                // Picker/clock/cancellation failure cannot hide an established exact-source alarm.
                consuming.admit()?;
                let (destination, filename) = selected?;
                execution.report(OperationPhaseV1::PublishingFile);
                let capture = consuming.take_capture()?;
                let (publication, revision) = match request.format {
                    RecipientFileFormatV1::Html => {
                        let receipt = capture
                            .into_recipient()?
                            .publish_html(destination, request.allow_stale, control)
                            .await
                            .map_err(|error| Box::new(map_app_error(error)))?;
                        let revision = receipt.current_revision;
                        (RecipientPublicationV1::Html(receipt), revision)
                    }
                    RecipientFileFormatV1::Pdf => {
                        let receipt = capture
                            .into_pdf()?
                            .publish_pdf(destination, request.allow_stale, control)
                            .await
                            .map_err(|error| Box::new(map_app_error(error)))?;
                        let revision = receipt.current_revision;
                        (RecipientPublicationV1::Pdf(receipt), revision)
                    }
                };
                let result = RecipientCreatedDtoV1 {
                    schema_version: 1,
                    preview_id: request.preview_id,
                    format: request.format,
                    filename,
                    publication,
                };
                // The publication is confirmed. No late cancellation/cleanup check changes it.
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

pub(super) async fn encode_created<T: Serialize + Send + 'static>(
    request_id: RequestId,
    revision: Revision,
    result: T,
    consuming: Arc<ConsumingReport>,
) -> Result<RecipientCreatedResponse, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let envelope = response(request_id, Some(revision), Vec::new(), result);
        let encoded =
            crate::setup_boundary::encode(&envelope, response_wire_limit(FRAME_BYTES), None);
        drop(envelope);
        let response = encoded.map_err(|_| publication_reply_error())?;
        Ok(RecipientCreatedResponse {
            response,
            consuming,
        })
    })
    .await
    .map_err(|_| publication_reply_error())?
}

fn publication_reply_error() -> ApiErrorDto {
    boundary_error(
        "recipient.acknowledgement_unavailable",
        "The report was saved, but its acknowledgement could not be prepared. Check the selected destination before exporting again.",
        None,
    )
}

pub(super) async fn select_destination<R: tauri::Runtime>(
    handle: tauri::AppHandle<R>,
    control: OperationControl,
    format: CaptureFormat,
    suggested_name: String,
) -> Result<(PathBuf, String), ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        check_control(&control)?;
        let (title, filter, extensions, extension_message): (_, _, &[&str], _) = match format {
            CaptureFormat::Html => (
                "Save recipient HTML report",
                "HTML report",
                &["html", "htm"],
                "Choose an HTML destination ending in .html or .htm.",
            ),
            CaptureFormat::Pdf => (
                "Save recipient PDF report",
                "PDF report",
                &["pdf"],
                "Choose a PDF destination ending in .pdf.",
            ),
            CaptureFormat::Schedule(eutheto_core::ScheduleExportFormatV1::Ics) => (
                "Save calendar",
                "iCalendar",
                &["ics"],
                "Choose an iCalendar destination ending in .ics.",
            ),
            CaptureFormat::Schedule(_) => (
                "Save schedule CSV",
                "CSV",
                &["csv"],
                "Choose a CSV destination ending in .csv.",
            ),
        };
        let selected = handle
            .dialog()
            .file()
            .set_title(title)
            .set_file_name(suggested_name)
            .add_filter(filter, extensions)
            .blocking_save_file()
            .ok_or_else(|| Box::new(crate::operations::cancelled()))?;
        check_control(&control)?;
        let destination = selected.into_path().map_err(|_| selection_error())?;
        if !destination
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|allowed| extension.eq_ignore_ascii_case(allowed))
            })
        {
            return Err(Box::new(boundary_error(
                "recipient.extension_invalid",
                extension_message,
                None,
            )));
        }
        let filename = selected_basename(&destination).map_err(|_| selection_error())?;
        if filename.len() > 4096 {
            return Err(selection_error().into());
        }
        Ok((destination, filename))
    })
    .await
    .map_err(|_| selection_error())?
}

fn selection_error() -> ApiErrorDto {
    boundary_error(
        "recipient.selection_invalid",
        "The selected report destination could not be used.",
        None,
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn solution_share_release<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: NativeOperationRequest,
) -> ApiResult<RecipientReleasedDtoV1> {
    let request: ReleaseRequest = decode(request.0)?;
    require_version(request.schema_version)?;
    let acknowledgement = state.recipient_reports.release(
        window.label(),
        request.scenario_id,
        CaptureKind::Recipient,
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

pub(super) fn take_control(
    execution: &mut OperationExecution,
) -> Result<OperationControl, ApiError> {
    let (budget, _) = execution.take_solve_admission().ok_or_else(|| {
        Box::new(boundary_error(
            "operation.budget_missing",
            "The recipient operation deadline was not admitted safely.",
            None,
        ))
    })?;
    Ok(OperationControl::Solve(budget.phase_view()))
}

pub(super) fn check_control(control: &OperationControl) -> Result<(), ApiError> {
    control.check().map_err(|reason| {
        Box::new(match reason {
            OperationInterruption::Cancelled => crate::operations::cancelled(),
            OperationInterruption::DeadlineExceeded => crate::operations::deadline_exceeded(),
        })
    })
}

#[cfg(all(test, feature = "bundled-ortools"))]
#[path = "recipient_report/worker_tests.rs"]
mod worker_tests;
