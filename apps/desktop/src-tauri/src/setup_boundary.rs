//! Versioned, window-owned adapters; scenario and domain authority remain in the core service.
use super::operations::{
    OperationCancelledV1, OperationClaim, OperationContextV1, OperationControlRequestV1,
    OperationExecution, OperationPhaseV1, OperationPrepareRequestV1, OperationPreparedV1,
    OperationPurposeV1, OperationReleasedV1, ProgressSink, require_version,
};
use super::{
    ApiError, ApiResult, DesktopState, SolutionApiResult, boundary_error, map_app_error, response,
};
use eutheto_core::{
    DomainSetupQueryV1, MAX_COMMAND_RESULT_BYTES, ScenarioSetupStatusV2, SetupContinuationV1,
    SetupSourceV2, WorkforceGenerationApplyRequestV1,
};
use eutheto_types::{CancellationToken, EntityId, OperationId, RequestId, Revision, ScenarioId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{io::Write, sync::Arc};
use tauri::State;
const FRAME_BYTES: usize = 64 * 1024;
const SUMMARY_BYTES: usize = 2 * 1024 * 1024;
const VALIDATION_BYTES: usize = 16 * 1024 * 1024;
const VIEW_BYTES: usize = 32 * 1024 * 1024;
// Pinned Tauri2.11.5 sends JSON below8192 bytes directly to the bound webview.
// Larger channel messages enter a shared fetch queue; progress must never use it.
const PROGRESS_WIRE_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenarioReadRequestV2 {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    operation_id: OperationId,
    expected_revision: Option<Revision>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenarioSetupViewRequestV2 {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    operation_id: OperationId,
    source: SetupSourceV2,
    query: DomainSetupQueryV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FullValidationRequestV2 {
    request_id: RequestId,
    schema_version: u32,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    operation_id: OperationId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenarioEntityRequestV2 {
    request_id: RequestId,
    schema_version: u32,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    kind: String,
    entity_id: EntityId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenarioEntitySearchRequestV2 {
    request_id: RequestId,
    schema_version: u32,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    kind: String,
    search: String,
    #[serde(default = "default_page_limit")]
    limit: u16,
    cursor: Option<SetupContinuationV1>,
}
const fn default_page_limit() -> u16 {
    50
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScenarioRuleCatalogRequestV2 {
    request_id: RequestId,
    schema_version: u32,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
}

fn decode<T: DeserializeOwned>(request: Option<Value>) -> Result<T, ApiError> {
    request
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            Box::new(boundary_error(
                "setup.invalid_request",
                "The setup request does not match its supported contract.",
                None,
            ))
        })
}
fn version_two(version: u32) -> Result<(), ApiError> {
    if version == 2 {
        Ok(())
    } else {
        Err(Box::new(boundary_error(
            "setup.version_unsupported",
            "This setup request version is not supported.",
            Some("/schemaVersion"),
        )))
    }
}
fn claim(
    operation_id: OperationId,
    request_id: RequestId,
    purpose: OperationPurposeV1,
    scenario_id: ScenarioId,
    expected_revision: Option<Revision>,
) -> OperationClaim {
    OperationClaim {
        operation_id,
        request_id,
        purpose: Some(purpose),
        context: OperationContextV1::Scenario {
            scenario_id,
            expected_revision,
        },
    }
}
fn progress(channel: tauri::ipc::Channel<tauri::ipc::Response>) -> ProgressSink {
    Arc::new(move |event| {
        if let Ok(message) = encode(event, PROGRESS_WIRE_BYTES, None) {
            let _ = channel.send(message);
        }
    })
}

fn preflight_view(
    app: &eutheto_core::EuthetoApp,
    request: &ScenarioSetupViewRequestV2,
    cancellation: &CancellationToken,
) -> Result<(), ApiError> {
    app.preflight_setup_view(&request.source, &request.query, cancellation)
        .map_err(|error| Box::new(map_app_error(error)))
}
fn preflight_generation(
    app: &eutheto_core::EuthetoApp,
    request: &WorkforceGenerationApplyRequestV1,
    cancellation: &CancellationToken,
) -> Result<(), ApiError> {
    app.preflight_reviewed_generation(request, cancellation)
        .map_err(|error| Box::new(map_app_error(error)))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn operation_prepare<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<OperationPreparedV1> {
    let request: OperationPrepareRequestV1 = decode(request)?;
    let result = state.operations.prepare(window.label(), &request)?;
    Ok(response(request.request_id, None, Vec::new(), result))
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn operation_cancel<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<OperationCancelledV1> {
    let request: OperationControlRequestV1 = decode(request)?;
    let result = state.operations.cancel(window.label(), &request)?;
    Ok(response(request.request_id, None, Vec::new(), result))
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri extracts owned command arguments"
)]
#[tauri::command]
pub(super) fn operation_release<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> ApiResult<OperationReleasedV1> {
    let request: OperationControlRequestV1 = decode(request)?;
    let result = state.operations.release(window.label(), &request)?;
    Ok(response(request.request_id, None, Vec::new(), result))
}

#[tauri::command]
pub(super) async fn scenario_get_summary<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    summary(
        &window,
        &state,
        decode(request)?,
        OperationPurposeV1::ScenarioSummary,
        progress(on_progress),
    )
    .await
}
#[tauri::command]
pub(super) async fn scenario_get_setup_status<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    summary(
        &window,
        &state,
        decode(request)?,
        OperationPurposeV1::SetupStatus,
        progress(on_progress),
    )
    .await
}
async fn summary<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    state: &DesktopState,
    request: ScenarioReadRequestV2,
    purpose: OperationPurposeV1,
    sink: ProgressSink,
) -> SolutionApiResult {
    version_two(request.schema_version)?;
    let status_only = purpose == OperationPurposeV1::SetupStatus;
    let operation = claim(
        request.operation_id,
        request.request_id,
        purpose,
        request.scenario_id,
        request.expected_revision,
    );
    let app = state.app.clone();
    state
        .operations
        .run(
            window.label(),
            operation,
            Some(sink),
            OperationPhaseV1::CapturingSnapshot,
            ((), None),
            move |(), mut execution| async move {
                let result = app
                    .setup_summary(
                        request.scenario_id,
                        request.expected_revision,
                        execution.cancellation(),
                    )
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let revision = result.revision;
                let cancellation = Some(execution.cancellation());
                if status_only {
                    let result = ScenarioSetupStatusV2 {
                        schema_version: result.schema_version,
                        scenario_id: result.scenario_id,
                        revision,
                        structure: result.structure,
                        fast: result.fast,
                        full: result.full,
                    };
                    finish(
                        &mut execution,
                        request.request_id,
                        revision,
                        result,
                        SUMMARY_BYTES + FRAME_BYTES,
                        cancellation,
                    )
                    .await
                } else {
                    finish(
                        &mut execution,
                        request.request_id,
                        revision,
                        result,
                        SUMMARY_BYTES + FRAME_BYTES,
                        cancellation,
                    )
                    .await
                }
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn scenario_get_view<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    view(&window, &state, decode(request)?, progress(on_progress)).await
}
async fn view<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    state: &DesktopState,
    request: ScenarioSetupViewRequestV2,
    sink: ProgressSink,
) -> SolutionApiResult {
    version_two(request.schema_version)?;
    let (purpose, phase) = match &request.source {
        SetupSourceV2::Stored => (
            OperationPurposeV1::SetupView {
                view_id: request.query.view_id.clone(),
            },
            OperationPhaseV1::BuildingView,
        ),
        SetupSourceV2::CommandPreview { .. } => (
            OperationPurposeV1::CommandPreview {
                view_id: request.query.view_id.clone(),
            },
            OperationPhaseV1::ApplyingPreview,
        ),
    };
    let operation = claim(
        request.operation_id,
        request.request_id,
        purpose,
        request.scenario_id,
        Some(request.expected_revision),
    );
    let app = state.app.clone();
    state
        .operations
        .run(
            window.label(),
            operation,
            Some(sink),
            phase,
            (request, Some(preflight_view)),
            move |request, mut execution| async move {
                let result = app
                    .setup_view(
                        request.scenario_id,
                        request.expected_revision,
                        request.source,
                        request.query,
                        execution.cancellation(),
                    )
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let revision = result.revision;
                let cancellation = Some(execution.cancellation());
                finish(
                    &mut execution,
                    request.request_id,
                    revision,
                    result,
                    VIEW_BYTES + FRAME_BYTES,
                    cancellation,
                )
                .await
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn scenario_get_entity<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: ScenarioEntityRequestV2 = decode(request)?;
    view(&window, &state, ScenarioSetupViewRequestV2 {
    request_id: request.request_id, schema_version: request.schema_version, scenario_id: request.scenario_id,
    expected_revision: request.expected_revision, operation_id: request.operation_id, source: SetupSourceV2::Stored,
    query: DomainSetupQueryV1 { schema_version: 1, view_id: "eutheto.setup.entity_detail".to_owned(), continuation: None,
        parameters: serde_json::json!({"entityKind": request.kind, "entityId": request.entity_id}) },
}, progress(on_progress)).await
}
#[tauri::command]
pub(super) async fn scenario_search_entities<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: ScenarioEntitySearchRequestV2 = decode(request)?;
    view(&window, &state, ScenarioSetupViewRequestV2 {
    request_id: request.request_id, schema_version: request.schema_version, scenario_id: request.scenario_id,
    expected_revision: request.expected_revision, operation_id: request.operation_id, source: SetupSourceV2::Stored,
    query: DomainSetupQueryV1 { schema_version: 1, view_id: "eutheto.setup.entity_page".to_owned(), continuation: request.cursor,
        parameters: serde_json::json!({"entityKind": request.kind, "search": request.search, "limit": request.limit}) },
}, progress(on_progress)).await
}
#[tauri::command]
pub(super) async fn scenario_get_rule_catalog<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: ScenarioRuleCatalogRequestV2 = decode(request)?;
    view(
        &window,
        &state,
        ScenarioSetupViewRequestV2 {
            request_id: request.request_id,
            schema_version: request.schema_version,
            scenario_id: request.scenario_id,
            expected_revision: request.expected_revision,
            operation_id: request.operation_id,
            source: SetupSourceV2::Stored,
            query: DomainSetupQueryV1 {
                schema_version: 1,
                view_id: "eutheto.setup.rule_catalog".to_owned(),
                continuation: None,
                parameters: serde_json::json!({}),
            },
        },
        progress(on_progress),
    )
    .await
}

#[tauri::command]
pub(super) async fn scenario_validate<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: FullValidationRequestV2 = decode(request)?;
    version_two(request.schema_version)?;
    let operation = claim(
        request.operation_id,
        request.request_id,
        OperationPurposeV1::FullValidation,
        request.scenario_id,
        Some(request.expected_revision),
    );
    let app = state.app.clone();
    state
        .operations
        .run(
            window.label(),
            operation,
            Some(progress(on_progress)),
            OperationPhaseV1::Validating,
            ((), None),
            move |(), mut execution| async move {
                let result = app
                    .full_validate_setup(
                        request.scenario_id,
                        request.expected_revision,
                        request.operation_id,
                        execution.cancellation(),
                    )
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let revision = result.revision;
                let cancellation = Some(execution.cancellation());
                finish(
                    &mut execution,
                    request.request_id,
                    revision,
                    result,
                    VALIDATION_BYTES + FRAME_BYTES,
                    cancellation,
                )
                .await
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn workforce_apply_reviewed_generation<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    on_progress: tauri::ipc::Channel<tauri::ipc::Response>,
) -> SolutionApiResult {
    let request: WorkforceGenerationApplyRequestV1 = decode(request)?;
    require_version(request.schema_version)?;
    let request_id = request.request_id;
    let operation = claim(
        request.operation_id,
        request_id,
        OperationPurposeV1::ApplyReviewedGeneration,
        request.scenario_id,
        Some(request.expected_revision),
    );
    let app = state.app.clone();
    state
        .operations
        .run(
            window.label(),
            operation,
            Some(progress(on_progress)),
            OperationPhaseV1::ApplyingPreview,
            (request, Some(preflight_generation)),
            move |request, mut execution| async move {
                let result = app
                    .apply_reviewed_generation(request, execution.cancellation())
                    .await
                    .map_err(|error| Box::new(map_app_error(error)))?;
                let revision = result.new_revision;
                // A committed result remains success even when a late cancellation races with transport.
                finish(
                    &mut execution,
                    request_id,
                    revision,
                    result,
                    MAX_COMMAND_RESULT_BYTES,
                    None,
                )
                .await
            },
        )
        .await
}

async fn finish<T: Serialize + Send + 'static>(
    execution: &mut OperationExecution,
    request_id: RequestId,
    revision: Revision,
    result: T,
    compact_limit: usize,
    cancellation: Option<CancellationToken>,
) -> SolutionApiResult {
    execution.preparing_response();
    tauri::async_runtime::spawn_blocking(move || {
        let envelope = response(request_id, Some(revision), Vec::new(), result);
        // Every quoted unsafe integer occupies at least16 bytes before its two added quotes.
        let wire_limit = compact_limit + compact_limit / 8 + FRAME_BYTES;
        encode(&envelope, wire_limit, cancellation)
    })
    .await
    .map_err(|_| {
        Box::new(boundary_error(
            "operation.execution_failed",
            "The native operation could not finish safely.",
            None,
        ))
    })?
}

struct SetupIntegerFormatter;
impl serde_json::ser::Formatter for SetupIntegerFormatter {
    fn write_i64<W: ?Sized + Write>(&mut self, writer: &mut W, value: i64) -> std::io::Result<()> {
        if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value) {
            write!(writer, "{value}")
        } else {
            write!(writer, "\"{value}\"")
        }
    }
    fn write_u64<W: ?Sized + Write>(&mut self, writer: &mut W, value: u64) -> std::io::Result<()> {
        if value <= 9_007_199_254_740_991 {
            write!(writer, "{value}")
        } else {
            write!(writer, "\"{value}\"")
        }
    }
}
struct ResponseWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
    cancellation: Option<CancellationToken>,
}
impl Write for ResponseWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(std::io::Error::other("cancelled"));
        }
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("response limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode<T: Serialize>(
    value: &T,
    limit: usize,
    cancellation: Option<CancellationToken>,
) -> SolutionApiResult {
    let mut writer = ResponseWriter {
        bytes: Vec::new(),
        limit,
        exceeded: false,
        cancellation,
    };
    if value
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut writer,
            SetupIntegerFormatter,
        ))
        .is_err()
    {
        let (code, message) = if writer
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            ("operation.cancelled", "The operation was cancelled.")
        } else if writer.exceeded {
            (
                "operation.resource_limit",
                "The setup response exceeds its resource limit.",
            )
        } else {
            (
                "protocol.response_serialization_failed",
                "The setup response could not be translated safely.",
            )
        };
        return Err(Box::new(boundary_error(code, message, None)));
    }
    let json = String::from_utf8(writer.bytes).map_err(|_| {
        Box::new(boundary_error(
            "protocol.response_serialization_failed",
            "The setup response could not be translated safely.",
            None,
        ))
    })?;
    Ok(tauri::ipc::Response::new(json))
}

#[cfg(test)]
#[path = "setup_boundary_tests.rs"]
mod tests;
