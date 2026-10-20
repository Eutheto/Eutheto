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
    AssignmentCsvDetection, AssignmentCsvMapping, AssignmentCsvPreviewRequestV1,
    AssignmentCsvReadFormat, AssignmentImportPreview, AssignmentImportTarget,
    AvailabilityCsvMapping, AvailabilityCsvPreviewRequestV1, AvailabilityCsvRowDecision,
    AvailabilityImportPreview, CsvApplyRequestV1, CsvDialect, CsvOperation, CsvReportDtoV1,
    CsvReportRows, CsvSampleCell, EligibilityCsvMapping, EligibilityCsvPreviewRequestV1,
    EligibilityImportPreview, EligibilityMatrixCellAddress, EligibilityMatrixDetection,
    EligibilityMatrixMapping, EligibilityMatrixPreview, EligibilityMatrixPreviewRequestV1,
    MAX_ASSIGNMENT_CSV_REPORT_BYTES, MAX_AVAILABILITY_CSV_REPORT_BYTES, MAX_CSV_DATA_RECORDS,
    MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS, MAX_CSV_LOGICAL_RECORDS, MAX_CSV_MAPPING_BYTES,
    MAX_CSV_SOURCE_BYTES, MAX_ELIGIBILITY_CSV_REPORT_BYTES, MAX_ELIGIBILITY_MATRIX_REPORT_BYTES,
    MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES, MAX_SHIFT_CSV_REPORT_BYTES, PeopleCsvDetection,
    PeopleCsvMapping, PeopleCsvPreviewRequestV1, PeopleImportPreview, RowDecision, ShiftCsvMapping,
    ShiftCsvPreviewRequestV1, ShiftCsvRowDecision, ShiftImportPreview, bounded_json_size,
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
const SAMPLE_BYTES: usize = 128 * 1024;
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

fn bounded_preview_request(
    request: Option<Value>,
    decisions_field: &str,
) -> Result<Option<Value>, ApiError> {
    if let Some(value) = &request {
        let mapping_exceeded = value
            .get("mapping")
            .is_some_and(|mapping| bounded_json_size(mapping, MAX_CSV_MAPPING_BYTES).is_err());
        let decisions_exceeded = value.get(decisions_field).is_some_and(|decisions| {
            decisions
                .as_array()
                .is_some_and(|rows| rows.len() > MAX_CSV_DECISIONS)
                || bounded_json_size(decisions, MAX_CSV_DECISION_BYTES).is_err()
        });
        let aliases_exceeded = [
            "personMappings",
            "shiftMappings",
            "assignmentTypeMappings",
            "locationMappings",
            "referenceMappings",
        ]
        .iter()
        .any(|field| {
            value
                .get("mapping")
                .and_then(|mapping| mapping.get(field))
                .and_then(Value::as_object)
                .is_some_and(|aliases| aliases.len() > MAX_CSV_DATA_RECORDS as usize)
        });
        if mapping_exceeded || decisions_exceeded || aliases_exceeded {
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

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum CsvFamily {
    People,
    Assignments,
    Eligibility,
    EligibilityMatrix,
    Shifts,
    Availability,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(tag = "family", rename_all = "camelCase", deny_unknown_fields)]
enum CsvReadFormat {
    People { dialect: CsvDialect },
    Assignments { format: AssignmentCsvReadFormat },
    Eligibility { dialect: CsvDialect },
    EligibilityMatrix {},
    Shifts { dialect: CsvDialect },
    Availability { dialect: CsvDialect },
}

#[derive(Serialize)]
#[serde(tag = "family", content = "data", rename_all = "camelCase")]
enum DetectionResult {
    People(PeopleCsvDetection),
    Assignments(AssignmentCsvDetection),
    Eligibility(PeopleCsvDetection),
    EligibilityMatrix(EligibilityMatrixDetection),
    Shifts(PeopleCsvDetection),
    Availability(PeopleCsvDetection),
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
struct MatrixSourceOpenRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    text: String,
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
    family: CsvFamily,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SampleRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    read_format: CsvReadFormat,
    record: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PeoplePreviewRequest {
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
struct AssignmentPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    mapping: AssignmentCsvMapping,
    target: AssignmentImportTarget,
    skipped_records: Vec<u32>,
    allow_empty: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EligibilityPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    mapping: EligibilityCsvMapping,
    skipped_records: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MatrixPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    mapping: EligibilityMatrixMapping,
    skipped_cells: Vec<EligibilityMatrixCellAddress>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShiftPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    mapping: ShiftCsvMapping,
    decisions: Vec<ShiftCsvRowDecision>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AvailabilityPreviewRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    mapping: AvailabilityCsvMapping,
    decisions: Vec<AvailabilityCsvRowDecision>,
}

enum PreviewPolicy {
    People {
        mapping: PeopleCsvMapping,
        decisions: Vec<RowDecision>,
    },
    Assignments {
        mapping: AssignmentCsvMapping,
        target: AssignmentImportTarget,
        skipped_records: Vec<u32>,
        allow_empty: bool,
    },
    Eligibility {
        mapping: EligibilityCsvMapping,
        skipped_records: Vec<u32>,
    },
    EligibilityMatrix {
        mapping: EligibilityMatrixMapping,
        skipped_cells: Vec<EligibilityMatrixCellAddress>,
    },
    Shifts {
        mapping: ShiftCsvMapping,
        decisions: Vec<ShiftCsvRowDecision>,
    },
    Availability {
        mapping: AvailabilityCsvMapping,
        decisions: Vec<AvailabilityCsvRowDecision>,
    },
}

impl PreviewPolicy {
    #[allow(clippy::too_many_lines)]
    async fn preview(
        self,
        app: &eutheto_core::EuthetoApp,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        input: impl std::io::Read + Send + 'static,
        operation: CsvOperation,
    ) -> Result<(RequestId, PreviewBody), ApiError> {
        match self {
            Self::People { mapping, decisions } => app
                .preview_people_csv(
                    PeopleCsvPreviewRequestV1 {
                        schema_version: 1,
                        scenario_id,
                        expected_revision,
                        mapping,
                        decisions,
                    },
                    input,
                    operation,
                )
                .await
                .map(|preview| (preview.preview_id, PreviewBody::People(preview.preview))),
            Self::Assignments {
                mapping,
                target,
                skipped_records,
                allow_empty,
            } => app
                .preview_assignment_csv(
                    AssignmentCsvPreviewRequestV1 {
                        schema_version: 1,
                        scenario_id,
                        expected_revision,
                        mapping,
                        target,
                        skipped_records,
                        allow_empty,
                    },
                    input,
                    operation,
                )
                .await
                .map(|preview| {
                    (
                        preview.preview_id,
                        PreviewBody::Assignments(preview.preview),
                    )
                }),
            Self::Eligibility {
                mapping,
                skipped_records,
            } => app
                .preview_eligibility_csv(
                    EligibilityCsvPreviewRequestV1 {
                        schema_version: 1,
                        scenario_id,
                        expected_revision,
                        mapping,
                        skipped_records,
                    },
                    input,
                    operation,
                )
                .await
                .map(|preview| {
                    (
                        preview.preview_id,
                        PreviewBody::Eligibility(preview.preview),
                    )
                }),
            Self::EligibilityMatrix {
                mapping,
                skipped_cells,
            } => app
                .preview_eligibility_matrix(
                    EligibilityMatrixPreviewRequestV1 {
                        schema_version: 1,
                        scenario_id,
                        expected_revision,
                        mapping,
                        skipped_cells,
                    },
                    input,
                    operation,
                )
                .await
                .map(|preview| {
                    (
                        preview.preview_id,
                        PreviewBody::EligibilityMatrix(preview.preview),
                    )
                }),
            Self::Shifts { mapping, decisions } => app
                .preview_shift_csv(
                    ShiftCsvPreviewRequestV1 {
                        schema_version: 1,
                        scenario_id,
                        expected_revision,
                        mapping,
                        decisions,
                    },
                    input,
                    operation,
                )
                .await
                .map(|preview| (preview.preview_id, PreviewBody::Shifts(preview.preview))),
            Self::Availability { mapping, decisions } => app
                .preview_availability_csv(
                    AvailabilityCsvPreviewRequestV1 {
                        schema_version: 1,
                        scenario_id,
                        expected_revision,
                        mapping,
                        decisions,
                    },
                    input,
                    operation,
                )
                .await
                .map(|preview| {
                    (
                        preview.preview_id,
                        PreviewBody::Availability(preview.preview),
                    )
                }),
        }
        .map_err(|error| map_app_error(error).into())
    }
}

struct PreviewWork {
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    source_id: SourceId,
    policy: PreviewPolicy,
}

#[derive(Serialize)]
#[serde(untagged)]
enum PreviewBody {
    People(PeopleImportPreview),
    Assignments(AssignmentImportPreview),
    Eligibility(EligibilityImportPreview),
    EligibilityMatrix(EligibilityMatrixPreview),
    Shifts(ShiftImportPreview),
    Availability(AvailabilityImportPreview),
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
struct SampleResult {
    schema_version: u32,
    source_id: SourceId,
    read_format: CsvReadFormat,
    record: u32,
    cells: Option<Vec<CsvSampleCell>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResult {
    schema_version: u32,
    source_id: SourceId,
    scenario_id: ScenarioId,
    revision: Revision,
    preview_id: RequestId,
    preview: PreviewBody,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyResult {
    schema_version: u32,
    source_id: SourceId,
    scenario_id: ScenarioId,
    outcome: eutheto_core::CsvApplyOutcomeV1,
    report: CsvReportDtoV1,
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
pub(super) async fn csv_source_open<R: tauri::Runtime>(
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
                        .set_title("Choose CSV snapshot")
                        .add_filter("CSV or tab-separated text", &["csv", "tsv", "txt"])
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

#[tauri::command]
pub(super) async fn eligibility_matrix_source_open<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    // The initial IPC Value already exists; reject before typed decoding or queued retention.
    if request
        .as_ref()
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .is_some_and(|text| text.len() > MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES)
    {
        return Err(request_too_large().into());
    }
    let request: MatrixSourceOpenRequest = decode(bounded_request(
        request,
        6 * MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + FRAME_BYTES,
    )?)?;
    require_version(request.schema_version)?;
    let owner = window.label().to_owned();
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
            OperationPhaseV1::CapturingSnapshot,
            (request, None),
            move |request, mut execution| async move {
                let reservation = custody.reserve_source(
                    &owner,
                    request.scenario_id,
                    Creator {
                        operation_id: request.operation_id,
                        request_id: request.request_id,
                    },
                    execution.cancellation(),
                )?;
                let byte_count = request.text.len();
                let source_id = reservation.publish(request.text.into_bytes())?;
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
pub(super) fn csv_source_close<R: tauri::Runtime>(
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
pub(super) async fn csv_detect<R: tauri::Runtime>(
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
                let operation = CsvOperation::child_of(&execution.cancellation());
                let result = match request.family {
                    CsvFamily::People => DetectionResult::People(
                        app.detect_people_csv(source.reader(), operation)
                            .await
                            .map_err(map_app_error)?,
                    ),
                    CsvFamily::Assignments => DetectionResult::Assignments(
                        app.detect_assignment_csv(source.reader(), operation)
                            .await
                            .map_err(map_app_error)?,
                    ),
                    CsvFamily::Eligibility => DetectionResult::Eligibility(
                        app.detect_people_csv(source.reader(), operation)
                            .await
                            .map_err(map_app_error)?,
                    ),
                    CsvFamily::EligibilityMatrix => DetectionResult::EligibilityMatrix(
                        app.detect_eligibility_matrix(source.reader(), operation)
                            .await
                            .map_err(map_app_error)?,
                    ),
                    CsvFamily::Shifts => DetectionResult::Shifts(
                        app.detect_people_csv(source.reader(), operation)
                            .await
                            .map_err(map_app_error)?,
                    ),
                    CsvFamily::Availability => DetectionResult::Availability(
                        app.detect_people_csv(source.reader(), operation)
                            .await
                            .map_err(map_app_error)?,
                    ),
                };
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
pub(super) async fn csv_record_sample<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: SampleRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let maximum_record = match request.read_format {
        CsvReadFormat::Assignments {
            format: AssignmentCsvReadFormat::CanonicalV1,
        } => MAX_CSV_DATA_RECORDS + 2,
        _ => MAX_CSV_LOGICAL_RECORDS,
    };
    if !(1..=maximum_record).contains(&request.record) {
        return Err(boundary_error(
            "people_csv.invalid_record",
            "Choose a logical record within the CSV record limit.",
            Some("/record"),
        )
        .into());
    }
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
                OperationPurposeV1::CsvSample,
                request.scenario_id,
                Some(request.expected_revision),
            ),
            Some(progress(on_progress)),
            OperationPhaseV1::BuildingView,
            (request, None),
            move |request, mut execution| async move {
                let source = custody.source(&owner, request.scenario_id, request.source_id)?;
                let operation = CsvOperation::child_of(&execution.cancellation());
                let sample = match request.read_format {
                    CsvReadFormat::People { dialect }
                    | CsvReadFormat::Eligibility { dialect }
                    | CsvReadFormat::Shifts { dialect }
                    | CsvReadFormat::Availability { dialect } => {
                        app.sample_people_csv_record(
                            source.reader(),
                            dialect,
                            request.record,
                            operation,
                        )
                        .await
                    }
                    CsvReadFormat::Assignments { format } => {
                        app.sample_assignment_csv_record(
                            source.reader(),
                            format,
                            request.record,
                            operation,
                        )
                        .await
                    }
                    CsvReadFormat::EligibilityMatrix {} => {
                        app.sample_eligibility_matrix_record(
                            source.reader(),
                            request.record,
                            operation,
                        )
                        .await
                    }
                }
                .map_err(map_app_error)?;
                let result = SampleResult {
                    schema_version: 1,
                    source_id: request.source_id,
                    read_format: request.read_format,
                    record: request.record,
                    cells: sample.map(|sample| sample.cells),
                };
                let cancellation = Some(execution.cancellation());
                let result = finish(
                    &mut execution,
                    request.request_id,
                    None,
                    result,
                    SAMPLE_BYTES,
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
    let request: PeoplePreviewRequest = decode(bounded_preview_request(request, "decisions")?)?;
    require_version(request.schema_version)?;
    preview_csv(
        window,
        state,
        PreviewWork {
            request_id: request.request_id,
            operation_id: request.operation_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            source_id: request.source_id,
            policy: PreviewPolicy::People {
                mapping: request.mapping,
                decisions: request.decisions,
            },
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn assignment_csv_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: AssignmentPreviewRequest =
        decode(bounded_preview_request(request, "skippedRecords")?)?;
    require_version(request.schema_version)?;
    preview_csv(
        window,
        state,
        PreviewWork {
            request_id: request.request_id,
            operation_id: request.operation_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            source_id: request.source_id,
            policy: PreviewPolicy::Assignments {
                mapping: request.mapping,
                target: request.target,
                skipped_records: request.skipped_records,
                allow_empty: request.allow_empty,
            },
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn eligibility_csv_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: EligibilityPreviewRequest =
        decode(bounded_preview_request(request, "skippedRecords")?)?;
    require_version(request.schema_version)?;
    preview_csv(
        window,
        state,
        PreviewWork {
            request_id: request.request_id,
            operation_id: request.operation_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            source_id: request.source_id,
            policy: PreviewPolicy::Eligibility {
                mapping: request.mapping,
                skipped_records: request.skipped_records,
            },
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn eligibility_matrix_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: MatrixPreviewRequest = decode(bounded_preview_request(request, "skippedCells")?)?;
    require_version(request.schema_version)?;
    preview_csv(
        window,
        state,
        PreviewWork {
            request_id: request.request_id,
            operation_id: request.operation_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            source_id: request.source_id,
            policy: PreviewPolicy::EligibilityMatrix {
                mapping: request.mapping,
                skipped_cells: request.skipped_cells,
            },
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn shift_csv_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: ShiftPreviewRequest = decode(bounded_preview_request(request, "decisions")?)?;
    require_version(request.schema_version)?;
    preview_csv(
        window,
        state,
        PreviewWork {
            request_id: request.request_id,
            operation_id: request.operation_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            source_id: request.source_id,
            policy: PreviewPolicy::Shifts {
                mapping: request.mapping,
                decisions: request.decisions,
            },
        },
        on_progress,
    )
    .await
}

#[tauri::command]
pub(super) async fn availability_csv_preview<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: AvailabilityPreviewRequest =
        decode(bounded_preview_request(request, "decisions")?)?;
    require_version(request.schema_version)?;
    preview_csv(
        window,
        state,
        PreviewWork {
            request_id: request.request_id,
            operation_id: request.operation_id,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            source_id: request.source_id,
            policy: PreviewPolicy::Availability {
                mapping: request.mapping,
                decisions: request.decisions,
            },
        },
        on_progress,
    )
    .await
}

async fn preview_csv<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: PreviewWork,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
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
                let operation = CsvOperation::child_of(&execution.cancellation());
                let (core_preview_id, body) = request
                    .policy
                    .preview(
                        &app,
                        request.scenario_id,
                        request.expected_revision,
                        source.reader(),
                        operation,
                    )
                    .await?;
                let preview_id = reservation.publish(core_preview_id)?;
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
                        preview: body,
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
pub(super) async fn csv_apply<R: tauri::Runtime>(
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
                    .apply_csv_import(
                        CsvApplyRequestV1 {
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
                        CsvOperation::child_of(&execution.cancellation()),
                    )
                    .await
                    .map_err(map_app_error)?;
                let revision = match applied.outcome {
                    eutheto_core::CsvApplyOutcomeV1::Applied { revision, .. }
                    | eutheto_core::CsvApplyOutcomeV1::NoChanges { revision } => revision,
                };
                let receipt_bytes = report_response_limit(&applied.report);
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
                    receipt_bytes,
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
pub(super) fn csv_preview_discard<R: tauri::Runtime>(
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
pub(super) async fn csv_report<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<CsvReportDtoV1> {
    let request: ReportRequest = decode(bounded_request(request, FRAME_BYTES)?)?;
    require_version(request.schema_version)?;
    let preview =
        state
            .csv
            .preview_for_report(window.label(), request.scenario_id, request.preview_id)?;
    let report = state
        .app
        .csv_report(preview.preview_id())
        .await
        .map_err(map_app_error)?;
    Ok(response(request.request_id, None, Vec::new(), report))
}

fn report_response_limit(report: &CsvReportDtoV1) -> usize {
    match &report.rows {
        CsvReportRows::People { .. } => DETECTION_BYTES + FRAME_BYTES,
        CsvReportRows::Assignments { .. } => MAX_ASSIGNMENT_CSV_REPORT_BYTES + FRAME_BYTES,
        CsvReportRows::Eligibility { .. } => MAX_ELIGIBILITY_CSV_REPORT_BYTES + FRAME_BYTES,
        CsvReportRows::EligibilityMatrix { .. } => {
            MAX_ELIGIBILITY_MATRIX_REPORT_BYTES + FRAME_BYTES
        }
        CsvReportRows::Shifts { .. } => MAX_SHIFT_CSV_REPORT_BYTES + FRAME_BYTES,
        CsvReportRows::Availability { .. } => MAX_AVAILABILITY_CSV_REPORT_BYTES + FRAME_BYTES,
    }
}

fn publication_error(error: &eutheto_export::ExportError) -> ApiErrorDto {
    let (code, message) = match error {
        eutheto_export::ExportError::Cancelled => {
            ("operation.cancelled", "Report publication was cancelled.")
        }
        eutheto_export::ExportError::DeadlineExceeded => (
            "operation.deadline_exceeded",
            "Report publication reached its deadline.",
        ),
        eutheto_export::ExportError::DestinationExists(_) => (
            "people_csv.destination_exists",
            "The destination already exists. Choose a new report filename.",
        ),
        _ => (
            "people_csv.publication_failed",
            "The CSV row report could not be saved. The import outcome is unchanged.",
        ),
    };
    boundary_error(code, message, None)
}

fn report_file_labels(rows: &CsvReportRows) -> (&'static str, &'static str) {
    match rows {
        CsvReportRows::People { .. } => (
            "Save rejected people rows",
            "people-import-rejected-rows.json",
        ),
        CsvReportRows::Assignments { .. } => (
            "Save excluded assignment rows",
            "assignment-import-excluded-rows.json",
        ),
        CsvReportRows::Eligibility { .. } => (
            "Save excluded eligibility rows",
            "eligibility-import-excluded-rows.json",
        ),
        CsvReportRows::EligibilityMatrix { .. } => (
            "Save excluded eligibility matrix cells",
            "eligibility-matrix-excluded-cells.json",
        ),
        CsvReportRows::Shifts { .. } => (
            "Save excluded shift rows",
            "shift-import-excluded-rows.json",
        ),
        CsvReportRows::Availability { .. } => (
            "Save excluded availability rows",
            "availability-import-excluded-rows.json",
        ),
    }
}

#[tauri::command]
pub(super) async fn csv_report_save<R: tauri::Runtime>(
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
                    .csv_report(preview.preview_id())
                    .await
                    .map_err(map_app_error)?;
                let (title, filename) = report_file_labels(&report.rows);
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
                        .set_title(title)
                        .set_file_name(filename)
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
