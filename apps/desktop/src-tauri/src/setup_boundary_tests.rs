use super::*;
use crate::operations::{OperationProgressV1, OperationRegistry};
use crate::tests::{invoke_ipc_args, invoke_ok};
use eutheto_core::{AppDependencies, AppPaths, AppQuery, AppQueryResult, EuthetoApp};
use eutheto_types::{
    ApiResponseDto, FixedClock, FixedMonotonicClock, ScenarioDocument, SystemIdGenerator,
};
use serde_json::json;
use std::{error::Error, sync::Mutex};
use tauri::ipc::IpcResponse;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
type ObservedProgress = (String, u32, usize, Value);
struct Fixture {
    _directory: tempfile::TempDir,
    _desktop: tauri::App<tauri::test::MockRuntime>,
    state: DesktopState,
    window: tauri::WebviewWindow<tauri::test::MockRuntime>,
    observer: tauri::WebviewWindow<tauri::test::MockRuntime>,
    scenario_id: ScenarioId,
    events: Arc<Mutex<Vec<ObservedProgress>>>,
}
fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}
async fn fixture() -> TestResult<Fixture> {
    let directory = tempfile::tempdir()?;
    let clock = Arc::new(FixedClock::new("2026-09-10T12:00:00Z".parse()?));
    let monotonic = Arc::new(FixedMonotonicClock::default());
    let ids = Arc::new(SystemIdGenerator);
    let app = EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("setup.sqlite3"),
            safety_backups: directory.path().join("backups"),
        },
        clock: clock.clone(),
        monotonic_clock: monotonic.clone(),
        ids: ids.clone(),
        cancellation: CancellationToken::new(),
    })
    .await
    .map_err(boxed)?;
    let mut state = DesktopState::new(
        app.clone(),
        directory.path().join("cache"),
        directory.path().join("backups"),
    );
    state.operations = Arc::new(OperationRegistry::new(
        app,
        clock,
        monotonic,
        ids,
        ["main".to_owned(), "observer".to_owned()],
    ));
    let events = Arc::new(Mutex::new(Vec::new()));
    let desktop = tauri::test::mock_builder()
        .channel_interceptor({
            let events = events.clone();
            move |window, callback, index, body| {
                if let tauri::ipc::InvokeResponseBody::Json(json) = body
                    && let Ok(message) = serde_json::from_str::<Value>(json)
                    && let Ok(mut events) = events.lock()
                {
                    events.push((window.label().to_owned(), callback.0, index, message));
                }
                true
            }
        })
        .invoke_handler(tauri::generate_handler![
            crate::project_create,
            operation_prepare,
            operation_cancel,
            operation_release,
            scenario_get_summary,
            scenario_get_setup_status,
            scenario_get_view,
            scenario_search_entities,
            scenario_get_entity,
            scenario_get_rule_catalog,
            scenario_validate,
            workforce_apply_reviewed_generation
        ])
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let window =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    let observer =
        tauri::WebviewWindowBuilder::new(&desktop, "observer", tauri::WebviewUrl::default())
            .build()?;
    let project: ApiResponseDto<Value> = invoke_ok(
        &window,
        "project_create",
        &json!({
            "schemaVersion": 1,
            "requestId": RequestId::new(&SystemIdGenerator)?, "title": "Native Workforce setup", "description": "",
            "domainPack": {"id":"official.workforce", "schemaVersion":1},
            "settings": {"timeZone":"UTC", "locale":"en-US", "units":"metric", "firstDate":"2026-09-01", "lastDate":"2026-09-01", "gapPolicy":"reject", "overlapPolicy":"earlier"}
        }),
    )?;
    let scenario_id = project.result["scenarioId"]
        .as_str()
        .ok_or("missing scenario id")?
        .parse()?;
    Ok(Fixture {
        _directory: directory,
        _desktop: desktop,
        state,
        window,
        observer,
        scenario_id,
        events,
    })
}
fn prepare(
    fixture: &Fixture,
    purpose: &Value,
    revision: Option<Revision>,
) -> TestResult<OperationId> {
    let prepared: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "operation_prepare",
        &json!({
            "requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":1, "purpose":purpose,
            "context":{"kind":"scenario", "scenarioId":fixture.scenario_id, "expectedRevision":revision}
        }),
    )?;
    Ok(prepared.result["operationId"]
        .as_str()
        .ok_or("missing operation id")?
        .parse()?)
}
fn heavy(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    request: &Value,
) -> TestResult<ApiResponseDto<Value>> {
    match invoke_ipc_args(
        window,
        command,
        json!({"request":request, "onProgress":"__CHANNEL__:42"}),
    )? {
        Ok(body) => Ok(body.deserialize()?),
        Err(error) => Err(format!("{command}: {error}").into()),
    }
}
fn summary(fixture: &Fixture) -> TestResult<ApiResponseDto<Value>> {
    let operation_id = prepare(fixture, &json!({"kind":"scenarioSummary"}), None)?;
    heavy(
        &fixture.window,
        "scenario_get_summary",
        &json!({"requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":2, "scenarioId":fixture.scenario_id, "expectedRevision":null, "operationId":operation_id}),
    )
}
async fn document(fixture: &Fixture) -> TestResult<ScenarioDocument> {
    match fixture
        .state
        .app
        .query(AppQuery::ScenarioView(fixture.scenario_id))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::Scenario(view) => Ok(view.document),
        _ => Err("unexpected core snapshot".into()),
    }
}

#[tokio::test]
async fn native_readiness_requires_explicit_full_validation_and_cancelled_claim_never_runs_it()
-> TestResult {
    let fixture = fixture().await?;
    let before = document(&fixture).await?;
    assert_eq!(summary(&fixture)?.result["full"]["state"], "notRun");
    let operation_id = prepare(
        &fixture,
        &json!({"kind":"fullValidation"}),
        Some(Revision::INITIAL),
    )?;
    let cancelled: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "operation_cancel",
        &json!({"requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":1, "operationId":operation_id}),
    )?;
    assert_eq!(cancelled.result["acknowledgement"], "cancellationRequested");
    let error = invoke_ipc_args(&fixture.window, "scenario_validate", json!({"onProgress":"__CHANNEL__:42", "request":{
        "requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":2, "scenarioId":fixture.scenario_id, "expectedRevision":0, "operationId":operation_id
    }}))?.err().ok_or("cancelled validation cannot run")?;
    assert_eq!(error["code"], "operation.cancelled");
    assert_eq!(summary(&fixture)?.result["full"]["state"], "notRun");
    let operation_id = prepare(
        &fixture,
        &json!({"kind":"fullValidation"}),
        Some(Revision::INITIAL),
    )?;
    let validated = heavy(
        &fixture.window,
        "scenario_validate",
        &json!({
            "requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":2, "scenarioId":fixture.scenario_id, "expectedRevision":0, "operationId":operation_id
        }),
    )?;
    let issues = validated.result["report"]["issues"]
        .as_array()
        .ok_or("missing complete report")?;
    let after = summary(&fixture)?;
    assert_eq!(after.result["full"]["state"], "completed");
    assert_eq!(after.result["full"]["stale"], false);
    assert_eq!(after.result["full"]["inputRevision"], 0);
    let errors = issues
        .iter()
        .filter(|issue| issue["severity"] == "error")
        .count();
    assert_eq!(
        after.result["full"]["counts"]["errors"].as_u64(),
        Some(u64::try_from(errors)?)
    );
    assert_eq!(document(&fixture).await?, before);
    Ok(())
}

#[tokio::test]
async fn native_preview_is_nonpublishing_and_progress_is_bound_to_actual_invoking_webview()
-> TestResult {
    let fixture = fixture().await?;
    let before = document(&fixture).await?;
    let view_id = "eutheto.setup.command_changes";
    let operation_id = prepare(
        &fixture,
        &json!({"kind":"commandPreview", "viewId":view_id}),
        Some(Revision::INITIAL),
    )?;
    let mut settings = before.settings.clone();
    settings.locale = "en-GB".parse()?;
    let request_id = RequestId::new(&SystemIdGenerator)?;
    let request = json!({
        "requestId":request_id, "schemaVersion":2, "scenarioId":fixture.scenario_id, "expectedRevision":0, "operationId":operation_id,
        "source":{"kind":"commandPreview", "command":{"type":"setScenarioSettings", "payload":{"settings":settings, "restoration":null}}},
        "query":{"schemaVersion":1, "viewId":view_id, "parameters":{"limit":50}, "continuation":null}
    });
    // Reusing the owner's callback number in another real invoking webview cannot select its owner.
    let denied = invoke_ipc_args(
        &fixture.observer,
        "scenario_get_view",
        json!({"request":request, "onProgress":"__CHANNEL__:42"}),
    )?
    .err()
    .ok_or("wrong window must not claim")?;
    assert_eq!(denied["code"], "operation.not_active");
    let result = heavy(&fixture.window, "scenario_get_view", &request)?;
    assert_eq!(result.request_id, request_id);
    assert_eq!(
        result.result["view"]["data"]["result"]["kind"],
        "commandChanges"
    );
    let changes = result.result["view"]["data"]["result"]["data"]["items"]
        .as_array()
        .ok_or("missing reviewed changes")?;
    assert!(
        changes
            .iter()
            .any(|row| row["change"]["after"]["locale"] == "en-GB")
    );
    assert_eq!(document(&fixture).await?, before);
    let events = fixture.events.lock().map_err(boxed)?;
    let first = events.first().ok_or("no progress dispatched")?;
    assert_eq!(first.3["operationId"], operation_id.to_string());
    for (owner, callback, index, event) in events.iter() {
        assert_eq!(owner, "main");
        assert_eq!(*callback, 42);
        assert_eq!(event["windowLabel"], "main");
        assert_eq!(event["requestId"], request_id.to_string());
        assert_eq!(
            event["context"]["scenarioId"],
            fixture.scenario_id.to_string()
        );
        assert_eq!(event["timestamp"], "2026-09-10T12:00:00Z");
        assert_eq!(event["sequence"].as_u64(), Some(u64::try_from(index + 1)?));
    }
    Ok(())
}

#[test]
fn setup_wire_preserves_signed_offsets_and_wide_values_and_rejects_partial_frames() -> TestResult {
    let value = json!({"offsetSeconds":-18000, "minimum":i64::MIN, "maximum":u64::MAX, "safe":9_007_199_254_740_991_u64});
    let response = encode(&value, 1024, None).map_err(boxed)?.body()?;
    let decoded: Value = response.deserialize()?;
    assert_eq!(decoded["offsetSeconds"], -18000);
    assert_eq!(decoded["minimum"], i64::MIN.to_string());
    assert_eq!(decoded["maximum"], u64::MAX.to_string());
    assert_eq!(decoded["safe"], 9_007_199_254_740_991_u64);
    assert_eq!(
        encode(&"é", 4, None)
            .map_err(boxed)?
            .body()?
            .deserialize::<String>()?,
        "é"
    );
    assert_eq!(
        encode(&"é", 3, None)
            .err()
            .ok_or("oversize frame accepted")?
            .code,
        "operation.resource_limit"
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        encode(&value, 1024, Some(cancellation))
            .err()
            .ok_or("cancelled read encoded")?
            .code,
        "operation.cancelled"
    );
    Ok(())
}

#[test]
fn progress_never_enters_shared_large_message_fetch_queue() -> TestResult {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let channel = tauri::ipc::Channel::new({
        let sent = sent.clone();
        move |body| {
            sent.lock()
                .map_err(|_| std::io::Error::other("capture poisoned"))?
                .push(body);
            Ok(())
        }
    });
    let sink = progress(channel);
    let mut event = OperationProgressV1 {
        event_version: 1,
        timestamp: "2026-09-10T12:00:00Z".parse()?,
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: RequestId::new(&SystemIdGenerator)?,
        window_label: "main".to_owned(),
        context: OperationContextV1::Scenario {
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            expected_revision: Some(Revision::INITIAL),
        },
        sequence: 1,
        phase: OperationPhaseV1::WaitingForAdmission,
    };
    sink(&event);
    let first = sent
        .lock()
        .map_err(boxed)?
        .pop()
        .ok_or("valid progress lost")?;
    assert_eq!(
        first.deserialize::<Value>()?["operationId"],
        event.operation_id.to_string()
    );
    event.window_label = "x".repeat(PROGRESS_WIRE_BYTES);
    sink(&event);
    assert!(
        sent.lock().map_err(boxed)?.is_empty(),
        "oversize progress reached channel fetch transport"
    );
    Ok(())
}

async fn occupy_heavy_slot(
    fixture: &Fixture,
) -> TestResult<(
    tokio::task::JoinHandle<Result<(), ApiError>>,
    tokio::sync::oneshot::Sender<()>,
)> {
    let operation_id = prepare(fixture, &json!({"kind":"scenarioSummary"}), None)?;
    let operation = claim(
        operation_id,
        RequestId::new(&SystemIdGenerator)?,
        OperationPurposeV1::ScenarioSummary,
        fixture.scenario_id,
        None,
    );
    let registry = fixture.state.operations.clone();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let running = tokio::spawn(async move {
        registry
            .run(
                "main",
                operation,
                None,
                OperationPhaseV1::CapturingSnapshot,
                ((), None),
                |(), _| async move {
                    let _ = entered.send(());
                    let _ = released.await;
                    Ok(())
                },
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), started).await??;
    Ok((running, release))
}

#[tokio::test]
async fn invalid_native_query_retires_before_heavy_admission() -> TestResult {
    let fixture = fixture().await?;
    let first = occupy_heavy_slot(&fixture).await?;
    let second = occupy_heavy_slot(&fixture).await?;
    let view_id = "eutheto.setup.entity_page";
    let operation_id = prepare(
        &fixture,
        &json!({"kind":"setupView", "viewId":view_id}),
        Some(Revision::INITIAL),
    )?;
    let request = json!({
        "requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":2, "scenarioId":fixture.scenario_id, "expectedRevision":0, "operationId":operation_id,
        "source":{"kind":"stored"}, "query":{"schemaVersion":1, "viewId":view_id, "parameters":{"entityKind":"person", "search":"x".repeat(257), "limit":50}, "continuation":null}
    });
    let window = fixture.window.clone();
    let checking = tokio::task::spawn_blocking(move || {
        invoke_ipc_args(
            &window,
            "scenario_get_view",
            json!({"request":request, "onProgress":"__CHANNEL__:42"}),
        )
        .map_err(|error| error.to_string())
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), checking)
        .await??
        .map_err(boxed)?;
    let error = result.err().ok_or("invalid query succeeded")?;
    assert_eq!(error["category"], "validation");
    let retired: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "operation_cancel",
        &json!({"requestId":RequestId::new(&SystemIdGenerator)?, "schemaVersion":1, "operationId":operation_id}),
    )?;
    assert_eq!(retired.result["acknowledgement"], "notActive");
    first.1.send(()).map_err(boxed)?;
    second.1.send(()).map_err(boxed)?;
    first.0.await?.map_err(boxed)?;
    second.0.await?.map_err(boxed)?;
    Ok(())
}
