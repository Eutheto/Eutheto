use super::*;
use crate::operations::OperationRegistry;
use crate::setup_boundary::{operation_cancel, operation_prepare, operation_release};
use crate::tests::{invoke_ipc_args, invoke_ok};
use eutheto_core::{AppDependencies, AppPaths, EuthetoApp};
use eutheto_types::{
    ApiResponseDto, CancellationToken, EventPayload, EventTopic, FixedClock, FixedMonotonicClock,
    SystemIdGenerator,
};
use serde_json::json;
use std::{error::Error, sync::Mutex, time::Duration};
use tokio::sync::oneshot;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
type Window = tauri::WebviewWindow<tauri::test::MockRuntime>;
type ObservedProgress = (String, u32, Value);
const SETTLEMENT_LIMIT: Duration = Duration::from_secs(10);
const UPDATED: &str = "2026-09-01T00:00:00Z";

struct Fixture {
    _directory: tempfile::TempDir,
    _desktop: tauri::App<tauri::test::MockRuntime>,
    state: DesktopState,
    window: Window,
    observer: Window,
    progress: Arc<Mutex<Vec<ObservedProgress>>>,
}

fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

fn next() -> TestResult<RequestId> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}

fn creator() -> TestResult<Creator> {
    Ok(Creator {
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: next()?,
    })
}

async fn fixture() -> TestResult<Fixture> {
    let directory = tempfile::tempdir()?;
    let clock = Arc::new(FixedClock::new("2026-09-12T12:00:00Z".parse()?));
    let monotonic = Arc::new(FixedMonotonicClock::default());
    let ids = Arc::new(SystemIdGenerator);
    let app = EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("settings.sqlite3"),
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
    let progress = Arc::new(Mutex::new(Vec::new()));
    let desktop = tauri::test::mock_builder()
        .channel_interceptor({
            let progress = Arc::clone(&progress);
            move |window, callback, _, body| {
                if let tauri::ipc::InvokeResponseBody::Json(body) = body
                    && let Ok(message) = serde_json::from_str::<Value>(body)
                    && let Ok(mut progress) = progress.lock()
                {
                    progress.push((window.label().to_owned(), callback.0, message));
                }
                true
            }
        })
        .invoke_handler(tauri::generate_handler![
            operation_prepare,
            operation_cancel,
            operation_release,
            settings_import_nonsecret,
            settings_export_nonsecret,
            crate::settings_get,
            crate::settings_update,
            crate::settings_reset_section,
            crate::app_get_paths_summary,
            crate::app_get_capabilities,
            crate::app_get_license_inventory,
        ])
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let window =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    let observer =
        tauri::WebviewWindowBuilder::new(&desktop, "observer", tauri::WebviewUrl::default())
            .build()?;
    Ok(Fixture {
        _directory: directory,
        _desktop: desktop,
        state,
        window,
        observer,
        progress,
    })
}

fn prepared(window: &Window, purpose: &str, revision: Option<Revision>) -> TestResult<OperationId> {
    let response: ApiResponseDto<Value> = invoke_ok(
        window,
        "operation_prepare",
        &json!({
            "schemaVersion":1, "requestId":next()?, "purpose":{"kind":purpose},
            "context":{"kind":"library", "expectedLibraryRevision":revision}
        }),
    )?;
    Ok(response.result["operationId"]
        .as_str()
        .ok_or("missing operation")?
        .parse()?)
}

fn heavy(
    window: &Window,
    command: &str,
    request: &Value,
) -> TestResult<Result<ApiResponseDto<Value>, Value>> {
    Ok(
        match invoke_ipc_args(
            window,
            command,
            json!({
                "request":request, "onProgress":"__CHANNEL__:42"
            }),
        )? {
            Ok(body) => Ok(body.deserialize()?),
            Err(error) => Err(error),
        },
    )
}

fn apply_request(
    preview: &SettingsImportPreviewDtoV1,
    operation: OperationId,
) -> TestResult<Value> {
    Ok(json!({
        "schemaVersion":1, "action":"apply", "requestId":next()?, "operationId":operation,
        "previewId":preview.preview_id, "approvalSha256":preview.approval_sha256,
        "expectedLibraryRevision":preview.library_revision
    }))
}

fn approval(preview: &SettingsImportPreviewDtoV1) -> TestResult<SettingsImportApplyRequestV1> {
    Ok(SettingsImportApplyRequestV1 {
        schema_version: 1,
        request_id: next()?,
        preview_id: preview.preview_id,
        approval_sha256: preview.approval_sha256.clone(),
        expected_library_revision: preview.library_revision,
    })
}

fn wrong_digest(preview: &SettingsImportPreviewDtoV1) -> String {
    let mut digest = preview.approval_sha256.clone();
    digest.replace_range(..1, if digest.starts_with('0') { "1" } else { "0" });
    digest
}

async fn core_preview(
    state: &DesktopState,
    settings: Value,
) -> TestResult<SettingsImportPreviewDtoV1> {
    state
        .app
        .preview_nonsecret_settings(
            serde_json::to_vec(&json!({
                "format":"eutheto/application-settings", "schemaVersion":1, "settings":settings
            }))?,
            CancellationToken::new(),
        )
        .await
        .map_err(boxed)
}

async fn retained(fixture: &Fixture, settings: Value) -> TestResult<SettingsImportPreviewDtoV1> {
    // Seed real core authority, not a fake picker or a client-authored apply payload.
    let reservation = fixture
        .state
        .settings
        .reserve("main", creator()?, CancellationToken::new())
        .map_err(boxed)?;
    let preview = core_preview(&fixture.state, settings).await?;
    reservation.publish(&preview).map_err(boxed)?;
    Ok(preview)
}

fn apply(
    window: &Window,
    preview: &SettingsImportPreviewDtoV1,
) -> TestResult<Result<ApiResponseDto<Value>, Value>> {
    let operation = prepared(
        window,
        "settingsImportApply",
        Some(preview.library_revision),
    )?;
    heavy(
        window,
        "settings_import_nonsecret",
        &apply_request(preview, operation)?,
    )
}

fn setting(window: &Window, key: &str) -> TestResult<Value> {
    let response: ApiResponseDto<Value> = invoke_ok(
        window,
        "settings_get",
        &json!({"requestId":next()?, "key":key}),
    )?;
    assert_eq!(response.current_revision, None);
    Ok(response.result["setting"].clone())
}

fn update(window: &Window, key: &str, value: &Value) -> TestResult {
    let _: ApiResponseDto<Value> = invoke_ok(
        window,
        "settings_update",
        &json!({
            "requestId":next()?, "key":key, "value":value
        }),
    )?;
    Ok(())
}

async fn snapshot(state: &DesktopState) -> TestResult<(Revision, Value)> {
    let snapshot = state
        .app
        .export_nonsecret_settings(CancellationToken::new())
        .await
        .map_err(boxed)?;
    Ok((
        snapshot.library_revision,
        serde_json::to_value(snapshot.document)?,
    ))
}

// Poll only asynchronous finalization, using a deliberately incorrect approval that
// cannot consume a live review. No timing assumption establishes a race ordering.
async fn discarded(state: &DesktopState, preview: &SettingsImportPreviewDtoV1) -> TestResult {
    let mut request = approval(preview)?;
    request.approval_sha256 = wrong_digest(preview);
    tokio::time::timeout(SETTLEMENT_LIMIT, async {
        loop {
            let error = state
                .app
                .apply_nonsecret_settings(request.clone(), CancellationToken::new())
                .await
                .err()
                .ok_or("incorrect approval unexpectedly applied")?;
            let error = map_app_error(error);
            if error.code == "settings.preview_unavailable" {
                return Ok(());
            }
            assert_eq!(error.code, "settings.approval_mismatch");
            tokio::task::yield_now().await;
        }
    })
    .await?
}

async fn available_slot(
    state: &DesktopState,
    owner: &str,
) -> TestResult<custody::PreviewReservation> {
    let creator = creator()?;
    tokio::time::timeout(SETTLEMENT_LIMIT, async {
        loop {
            match state
                .settings
                .reserve(owner, creator, CancellationToken::new())
            {
                Ok(reservation) => return Ok(reservation),
                Err(error) => assert_eq!(error.code, "settings.preview_capacity"),
            }
            tokio::task::yield_now().await;
        }
    })
    .await?
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn native_apply_preserves_review_on_bad_approval_then_commits_once_with_library_progress()
-> TestResult {
    let fixture = fixture().await?;
    let entries = json!({
        "appearance":{"value":{"theme":"dark","reducedMotion":true},"updatedAt":UPDATED},
        "locale":{"value":"en-GB","updatedAt":UPDATED},
        "units":{"value":"metric","updatedAt":UPDATED}
    });
    let preview = retained(&fixture, entries.clone()).await?;
    let before = snapshot(&fixture.state).await?;
    let mut events = fixture
        .state
        .app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;

    assert_eq!(
        apply(&fixture.observer, &preview)?
            .err()
            .ok_or("foreign review applied")?["code"],
        "settings.preview_unavailable"
    );
    let operation = prepared(
        &fixture.window,
        "settingsImportApply",
        Some(preview.library_revision),
    )?;
    let mut request = apply_request(&preview, operation)?;
    request["approvalSha256"] = json!(wrong_digest(&preview));
    assert_eq!(
        heavy(&fixture.window, "settings_import_nonsecret", &request)?
            .err()
            .ok_or("wrong digest applied")?["code"],
        "settings.approval_mismatch"
    );
    let wrong_revision = Revision::new(preview.library_revision.value() + 1);
    let operation = prepared(&fixture.window, "settingsImportApply", Some(wrong_revision))?;
    let mut request = apply_request(&preview, operation)?;
    request["expectedLibraryRevision"] = json!(wrong_revision);
    assert_eq!(
        heavy(&fixture.window, "settings_import_nonsecret", &request)?
            .err()
            .ok_or("wrong retained revision applied")?["code"],
        "settings.approval_mismatch"
    );
    assert_eq!(snapshot(&fixture.state).await?, before);
    assert!(events.try_recv().map_err(boxed)?.is_none());

    let operation = prepared(
        &fixture.window,
        "settingsImportApply",
        Some(preview.library_revision),
    )?;
    let request = apply_request(&preview, operation)?;
    let request_id: RequestId = serde_json::from_value(request["requestId"].clone())?;
    let applied = heavy(&fixture.window, "settings_import_nonsecret", &request)?.map_err(boxed)?;
    let committed_revision = Revision::new(preview.library_revision.value() + 1);
    assert_eq!(applied.current_revision, Some(committed_revision));
    assert_eq!(
        applied.result,
        json!({
            "kind":"applied", "schemaVersion":1, "previewId":preview.preview_id,
            "libraryRevision":committed_revision, "changed":true
        })
    );
    for (key, entry) in entries.as_object().ok_or("entries missing")? {
        assert_eq!(setting(&fixture.window, key)?, *entry);
    }
    assert!(
        matches!(events.try_recv().map_err(boxed)?.map(|event| event.payload),
        Some(EventPayload::AppNotification { context, code, .. })
        if context.request_id == Some(request_id) && code == "settings.updated")
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());
    {
        let progress = fixture.progress.lock().map_err(boxed)?;
        let messages: Vec<_> = progress
            .iter()
            .filter(|(_, _, value)| value["operationId"] == json!(operation))
            .collect();
        assert!(
            messages
                .iter()
                .any(|(_, _, value)| value["phase"] == "applyingPreview")
        );
        for (owner, callback, message) in messages {
            assert_eq!(owner, "main");
            assert_eq!(*callback, 42);
            assert_eq!(message["requestId"], json!(request_id));
            assert_eq!(
                message["context"],
                json!({"kind":"library", "expectedLibraryRevision":preview.library_revision})
            );
        }
    }
    assert_eq!(
        apply(&fixture.window, &preview)?
            .err()
            .ok_or("review replayed")?["code"],
        "settings.preview_unavailable"
    );

    let unchanged = retained(&fixture, entries).await?;
    let response = apply(&fixture.window, &unchanged)?.map_err(boxed)?;
    assert_eq!(response.result["changed"], false);
    assert_eq!(response.current_revision, Some(committed_revision));
    assert_eq!(
        response.result["libraryRevision"],
        json!(committed_revision)
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());

    let clear = retained(&fixture, json!({})).await?;
    let response = apply(&fixture.window, &clear)?.map_err(boxed)?;
    assert_eq!(response.result["changed"], true);
    assert_eq!(
        response.current_revision,
        Some(Revision::new(committed_revision.value() + 1))
    );
    for key in ["appearance", "locale", "units"] {
        assert_eq!(setting(&fixture.window, key)?, Value::Null);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn native_closed_actions_versions_sizes_and_optional_channels_reject_before_consumption()
-> TestResult {
    let fixture = fixture().await?;
    let preview = retained(
        &fixture,
        json!({"units":{"value":"metric","updatedAt":UPDATED}}),
    )
    .await?;
    let before = snapshot(&fixture.state).await?;
    let operation = prepared(
        &fixture.window,
        "settingsImportApply",
        Some(preview.library_revision),
    )?;
    let request = apply_request(&preview, operation)?;
    let discard = json!({"schemaVersion":1,"action":"discard","requestId":next()?,
        "target":{"kind":"preview","previewId":preview.preview_id}});
    let preview_operation = prepared(&fixture.window, "settingsImportPreview", None)?;
    let preview_request = json!({"schemaVersion":1,"action":"preview","requestId":next()?,"operationId":preview_operation});
    for (base, field, value) in [
        (request.clone(), "settings", json!({})),
        (
            preview_request.clone(),
            "path",
            json!("/private/do-not-open.json"),
        ),
        (discard.clone(), "operationId", json!(operation)),
    ] {
        let mut invalid = base;
        invalid[field] = value;
        assert!(heavy(&fixture.window, "settings_import_nonsecret", &invalid)?.is_err());
    }
    let mut invalid_target = discard.clone();
    invalid_target["target"]["owner"] = json!("observer");
    assert!(
        heavy(
            &fixture.window,
            "settings_import_nonsecret",
            &invalid_target
        )?
        .is_err()
    );
    for base in [&request, &preview_request, &discard] {
        let mut invalid = base.clone();
        invalid["schemaVersion"] = json!(2);
        assert_eq!(
            heavy(&fixture.window, "settings_import_nonsecret", &invalid)?
                .err()
                .ok_or("unknown version accepted")?["code"],
            "operation.version_unsupported"
        );
        let mut oversized = base.clone();
        oversized["padding"] = json!("x".repeat(64 * 1024));
        assert_eq!(
            heavy(&fixture.window, "settings_import_nonsecret", &oversized)?
                .err()
                .ok_or("oversize accepted")?["code"],
            "settings.request_too_large"
        );
    }
    let export_operation = prepared(&fixture.window, "settingsExport", None)?;
    let export_request =
        json!({"schemaVersion":1,"requestId":next()?,"operationId":export_operation});
    let mut invalid_export = export_request.clone();
    invalid_export["path"] = json!("/private/do-not-write.json");
    assert!(
        heavy(
            &fixture.window,
            "settings_export_nonsecret",
            &invalid_export
        )?
        .is_err()
    );
    for (command, request) in [
        ("settings_import_nonsecret", &request),
        ("settings_import_nonsecret", &preview_request),
        ("settings_export_nonsecret", &export_request),
    ] {
        for value in [
            None,
            Some(Value::Null),
            Some(json!({"secret":"channel-secret"})),
            Some(json!("channel-secret")),
            Some(json!(format!("__CHANNEL__:{}", "7".repeat(65)))),
        ] {
            let mut args = json!({"request":request});
            if let Some(value) = value {
                args["onProgress"] = value;
            }
            let error = invoke_ipc_args(&fixture.window, command, args)?
                .err()
                .ok_or("invalid progress channel accepted")?;
            assert_eq!(error["code"], "operation.progress_channel_invalid");
            assert!(!error.to_string().contains("channel-secret"));
            assert!(!error.to_string().contains("__CHANNEL__"));
        }
    }
    assert_eq!(snapshot(&fixture.state).await?, before);
    // Valid optional-Value channel binding reaches cancellation before any picker.
    // The mock app deliberately has no dialog plugin; real picker coverage is separate.
    for (command, request, operation) in [
        (
            "settings_import_nonsecret",
            preview_request,
            preview_operation,
        ),
        (
            "settings_export_nonsecret",
            export_request,
            export_operation,
        ),
    ] {
        let cancelled: ApiResponseDto<Value> = invoke_ok(
            &fixture.window,
            "operation_cancel",
            &json!({
                "schemaVersion":1,"requestId":next()?,"operationId":operation
            }),
        )?;
        assert_eq!(cancelled.result["acknowledgement"], "cancellationRequested");
        assert_eq!(
            heavy(&fixture.window, command, &request)?
                .err()
                .ok_or("cancelled picker ran")?["code"],
            "operation.cancelled"
        );
    }
    // All admission failures leave both the prepared operation and real review usable.
    let applied = heavy(&fixture.window, "settings_import_nonsecret", &request)?.map_err(boxed)?;
    assert_eq!(applied.result["changed"], true);
    assert_eq!(setting(&fixture.window, "units")?["value"], "metric");
    Ok(())
}

// Dropping the release sender also releases the owned work on an assertion/error path.
async fn occupy_heavy_slot(
    fixture: &Fixture,
) -> TestResult<(
    tokio::task::JoinHandle<Result<(), ApiError>>,
    oneshot::Sender<()>,
)> {
    let operation = prepared(&fixture.window, "settingsExport", None)?;
    let registry = Arc::clone(&fixture.state.operations);
    let operation = claim(operation, next()?, OperationPurposeV1::SettingsExport, None);
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
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
    tokio::time::timeout(SETTLEMENT_LIMIT, started).await??;
    Ok((running, release))
}

#[tokio::test]
async fn discard_is_unrevisioned_owner_private_and_needs_neither_channel_nor_heavy_capacity()
-> TestResult {
    let fixture = fixture().await?;
    let preview = retained(
        &fixture,
        json!({"locale":{"value":"en-GB","updatedAt":UPDATED}}),
    )
    .await?;
    let first = occupy_heavy_slot(&fixture).await?;
    let second = occupy_heavy_slot(&fixture).await?;
    let request_id = next()?;
    let request = json!({"schemaVersion":1,"action":"discard","requestId":request_id,
        "target":{"kind":"preview","previewId":preview.preview_id}});
    let observer = fixture.observer.clone();
    let unknown = next()?;
    let checking = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let foreign: ApiResponseDto<Value> =
            invoke_ok(&observer, "settings_import_nonsecret", &request)
                .map_err(|error| error.to_string())?;
        let mut missing_request = request.clone();
        missing_request["target"]["previewId"] = json!(unknown);
        let missing: ApiResponseDto<Value> =
            invoke_ok(&observer, "settings_import_nonsecret", &missing_request)
                .map_err(|error| error.to_string())?;
        assert_eq!(
            serde_json::to_value(&foreign).map_err(|error| error.to_string())?,
            serde_json::to_value(&missing).map_err(|error| error.to_string())?
        );
        assert_eq!(foreign.current_revision, None);
        assert_eq!(
            foreign.result,
            json!({"kind":"discarded","schemaVersion":1})
        );
        Ok(())
    });
    tokio::time::timeout(SETTLEMENT_LIMIT, checking)
        .await??
        .map_err(boxed)?;
    first.1.send(()).map_err(boxed)?;
    second.1.send(()).map_err(boxed)?;
    first.0.await?.map_err(boxed)?;
    second.0.await?.map_err(boxed)?;
    assert_eq!(
        apply(&fixture.window, &preview)?.map_err(boxed)?.result["changed"],
        true
    );

    let review = retained(&fixture, json!({})).await?;
    let request = json!({"schemaVersion":1,"action":"discard","requestId":next()?,
        "target":{"kind":"preview","previewId":review.preview_id}});
    let discarded_once: ApiResponseDto<Value> =
        invoke_ok(&fixture.window, "settings_import_nonsecret", &request)?;
    let discarded_again: ApiResponseDto<Value> =
        invoke_ok(&fixture.window, "settings_import_nonsecret", &request)?;
    assert_eq!(
        serde_json::to_value(discarded_once)?,
        serde_json::to_value(discarded_again)?
    );
    assert_eq!(
        apply(&fixture.window, &review)?
            .err()
            .ok_or("discarded review applied")?["code"],
        "settings.preview_unavailable"
    );
    discarded(&fixture.state, &review).await?;
    assert_eq!(setting(&fixture.window, "locale")?["value"], "en-GB");
    Ok(())
}

#[tokio::test]
async fn existing_setting_commands_and_about_reads_preserve_nonsecret_contracts() -> TestResult {
    let fixture = fixture().await?;
    update(
        &fixture.window,
        "appearance",
        &json!({"theme":"dark","reducedMotion":true}),
    )?;
    assert_eq!(
        setting(&fixture.window, "appearance")?["value"],
        json!({"theme":"dark","reducedMotion":true})
    );
    let before = snapshot(&fixture.state).await?;
    for (key, value) in [
        ("credentials", json!("do-not-store")),
        ("units", json!("imperial")),
    ] {
        assert!(
            invoke_ipc_args(
                &fixture.window,
                "settings_update",
                json!({"request":{
                    "requestId":next()?,"key":key,"value":value
                }})
            )?
            .is_err()
        );
    }
    assert_eq!(snapshot(&fixture.state).await?, before);
    for existed in [true, false] {
        let response: ApiResponseDto<Value> = invoke_ok(
            &fixture.window,
            "settings_reset_section",
            &json!({"requestId":next()?,"key":"appearance"}),
        )?;
        assert_eq!(response.result["existed"], existed);
        assert_eq!(setting(&fixture.window, "appearance")?, Value::Null);
    }
    // Local setting validation is unchanged: portability must not silently tighten it.
    update(&fixture.window, "locale", &json!("con"))?;
    assert_eq!(setting(&fixture.window, "locale")?["value"], "con");
    assert!(
        fixture
            .state
            .app
            .export_nonsecret_settings(CancellationToken::new())
            .await
            .is_err()
    );

    let paths: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "app_get_paths_summary",
        &json!({"requestId":next()?}),
    )?;
    assert_eq!(paths.current_revision, None);
    assert_eq!(
        paths.result,
        json!({"appDataConfigured":true,"cacheConfigured":true,"backupConfigured":true})
    );
    let capabilities: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "app_get_capabilities",
        &json!({"requestId":next()?}),
    )?;
    let available = capabilities.result["availableCommands"]
        .as_array()
        .ok_or("available commands missing")?;
    let unavailable = capabilities.result["unavailableCommands"]
        .as_array()
        .ok_or("unavailable commands missing")?;
    for name in [
        "settings_get",
        "settings_update",
        "settings_reset_section",
        "settings_import_nonsecret",
        "settings_export_nonsecret",
        "app_get_paths_summary",
        "app_get_license_inventory",
    ] {
        assert!(available.contains(&json!(name)));
        assert!(!unavailable.contains(&json!(name)));
    }
    let inventory: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "app_get_license_inventory",
        &json!({"requestId":next()?}),
    )?;
    assert_eq!(inventory.current_revision, None);
    assert_eq!(inventory.result["scope"], "lockedWorkspace");
    assert_eq!(inventory.result["schemaVersion"], 2);
    // Compare decoded authoritative data, not source code or a pinned package count.
    let mut expected: Value = serde_json::from_str(include_str!(
        "../../../../xtask/generated/license-inventory.json"
    ))?;
    expected["scope"] = json!("lockedWorkspace");
    assert_eq!(inventory.result, expected);
    assert!(
        inventory.result["packages"]
            .as_array()
            .ok_or("packages missing")?
            .iter()
            .any(|package| package["licenseConcluded"] == "NOASSERTION")
    );
    Ok(())
}

#[tokio::test]
async fn creating_ready_active_and_closing_reviews_all_charge_the_three_slot_limit() -> TestResult {
    let fixture = fixture().await?;
    let creating_id = creator()?;
    let creating = fixture
        .state
        .settings
        .reserve("main", creating_id, CancellationToken::new())
        .map_err(boxed)?;
    let ready = retained(&fixture, json!({})).await?;
    let active = retained(&fixture, json!({})).await?;
    let lease = fixture
        .state
        .settings
        .acquire_apply(
            "main",
            active.preview_id,
            &active.approval_sha256,
            active.library_revision,
        )
        .map_err(boxed)?;
    let capacity = || -> TestResult {
        assert_eq!(
            fixture
                .state
                .settings
                .reserve("observer", creator()?, CancellationToken::new())
                .err()
                .ok_or("fourth review admitted")?
                .code,
            "settings.preview_capacity"
        );
        Ok(())
    };
    capacity()?;
    let target = PreviewTarget::Creator {
        operation_id: creating_id.operation_id,
        request_id: creating_id.request_id,
    };
    fixture.state.settings.discard("main", &target);
    fixture.state.settings.discard(
        "main",
        &PreviewTarget::Preview {
            preview_id: active.preview_id,
        },
    );
    capacity()?;
    assert!(
        fixture
            .state
            .settings
            .acquire_apply(
                "main",
                active.preview_id,
                &active.approval_sha256,
                active.library_revision
            )
            .is_err()
    );
    // Abandoning an unpublished creator frees exactly its own slot.
    drop(creating);
    let replacement = fixture
        .state
        .settings
        .reserve("observer", creator()?, CancellationToken::new())
        .map_err(boxed)?;
    capacity()?;
    // Discard cannot revoke core authority while its already-owned native use lives.
    let applied = fixture
        .state
        .app
        .apply_nonsecret_settings(approval(&active)?, CancellationToken::new())
        .await
        .map_err(boxed)?;
    assert!(!applied.changed);
    capacity()?;
    drop(lease);
    let settled = available_slot(&fixture.state, "observer").await?;
    let ready_use = fixture
        .state
        .settings
        .acquire_apply(
            "main",
            ready.preview_id,
            &ready.approval_sha256,
            ready.library_revision,
        )
        .map_err(boxed)?;
    drop(ready_use);
    discarded(&fixture.state, &ready).await?;
    drop((replacement, settled));
    Ok(())
}

#[tokio::test]
async fn creator_cleanup_is_owner_bound_and_late_publication_cannot_resurrect_a_review()
-> TestResult {
    let fixture = fixture().await?;
    let creator = creator()?;
    let reservation = fixture
        .state
        .settings
        .reserve("main", creator, CancellationToken::new())
        .map_err(boxed)?;
    let preview = core_preview(&fixture.state, json!({})).await?;
    let request = json!({"schemaVersion":1,"action":"discard","requestId":next()?,
        "target":{"kind":"creator","operationId":creator.operation_id,"requestId":creator.request_id}});
    let _: ApiResponseDto<Value> =
        invoke_ok(&fixture.observer, "settings_import_nonsecret", &request)?;
    reservation.publish(&preview).map_err(boxed)?;
    let lease = fixture
        .state
        .settings
        .acquire_apply(
            "main",
            preview.preview_id,
            &preview.approval_sha256,
            preview.library_revision,
        )
        .map_err(boxed)?;
    drop(lease);
    discarded(&fixture.state, &preview).await?;

    let creator = self::creator()?;
    let reservation = fixture
        .state
        .settings
        .reserve("main", creator, CancellationToken::new())
        .map_err(boxed)?;
    let preview = core_preview(&fixture.state, json!({})).await?;
    let request = json!({"schemaVersion":1,"action":"discard","requestId":next()?,
        "target":{"kind":"creator","operationId":creator.operation_id,"requestId":creator.request_id}});
    let _: ApiResponseDto<Value> =
        invoke_ok(&fixture.window, "settings_import_nonsecret", &request)?;
    assert_eq!(
        reservation
            .publish(&preview)
            .err()
            .ok_or("late publication resurrected review")?
            .code,
        "operation.cancelled"
    );
    assert!(
        fixture
            .state
            .settings
            .acquire_apply(
                "main",
                preview.preview_id,
                &preview.approval_sha256,
                preview.library_revision
            )
            .is_err()
    );
    discarded(&fixture.state, &preview).await?;
    // Creator cleanup remains idempotent after the late owner has settled.
    let _: ApiResponseDto<Value> =
        invoke_ok(&fixture.window, "settings_import_nonsecret", &request)?;

    // An abandonment can arrive before native reservation. The late-response
    // cleanup repeats the same creator target rather than depending on its ID.
    let creator = self::creator()?;
    let request = json!({"schemaVersion":1,"action":"discard","requestId":next()?,
        "target":{"kind":"creator","operationId":creator.operation_id,"requestId":creator.request_id}});
    let _: ApiResponseDto<Value> =
        invoke_ok(&fixture.window, "settings_import_nonsecret", &request)?;
    let reservation = fixture
        .state
        .settings
        .reserve("main", creator, CancellationToken::new())
        .map_err(boxed)?;
    let preview = core_preview(&fixture.state, json!({})).await?;
    reservation.publish(&preview).map_err(boxed)?;
    let _: ApiResponseDto<Value> =
        invoke_ok(&fixture.window, "settings_import_nonsecret", &request)?;
    assert!(
        fixture
            .state
            .settings
            .acquire_apply(
                "main",
                preview.preview_id,
                &preview.approval_sha256,
                preview.library_revision
            )
            .is_err()
    );
    discarded(&fixture.state, &preview).await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn teardown_denies_new_uses_but_keeps_active_authority_until_its_owner_settles() -> TestResult
{
    for shutdown in [false, true] {
        let fixture = fixture().await?;
        let active = retained(
            &fixture,
            json!({"units":{"value":"metric","updatedAt":UPDATED}}),
        )
        .await?;
        let first = fixture
            .state
            .settings
            .reserve("observer", creator()?, CancellationToken::new())
            .map_err(boxed)?;
        let second = fixture
            .state
            .settings
            .reserve("observer", creator()?, CancellationToken::new())
            .map_err(boxed)?;
        let lease = fixture
            .state
            .settings
            .acquire_apply(
                "main",
                active.preview_id,
                &active.approval_sha256,
                active.library_revision,
            )
            .map_err(boxed)?;
        let app = fixture.state.app.clone();
        let request = approval(&active)?;
        let (release, released) = oneshot::channel();
        let owned = tokio::spawn(async move {
            let _ = released.await;
            let result = app
                .apply_nonsecret_settings(request, CancellationToken::new())
                .await;
            drop(lease);
            result
        });
        if shutdown {
            fixture.state.settings.shutdown();
        } else {
            fixture.state.settings.close_window("main");
        }
        assert!(
            fixture
                .state
                .settings
                .acquire_apply(
                    "main",
                    active.preview_id,
                    &active.approval_sha256,
                    active.library_revision
                )
                .is_err()
        );
        assert_eq!(
            fixture
                .state
                .settings
                .reserve("main", creator()?, CancellationToken::new())
                .err()
                .ok_or("closed owner admitted")?
                .code,
            "operation.cancelled"
        );
        let expected = if shutdown {
            "operation.cancelled"
        } else {
            "settings.preview_capacity"
        };
        assert_eq!(
            fixture
                .state
                .settings
                .reserve("observer", creator()?, CancellationToken::new())
                .err()
                .ok_or("active closing slot retired early")?
                .code,
            expected
        );
        release.send(()).map_err(boxed)?;
        let applied = tokio::time::timeout(SETTLEMENT_LIMIT, owned)
            .await??
            .map_err(boxed)?;
        assert!(applied.changed);
        assert_eq!(setting(&fixture.window, "units")?["value"], "metric");
        assert!(
            fixture
                .state
                .settings
                .acquire_apply(
                    "main",
                    active.preview_id,
                    &active.approval_sha256,
                    active.library_revision
                )
                .is_err()
        );
        if !shutdown {
            drop(available_slot(&fixture.state, "observer").await?);
        }
        drop((first, second));
    }
    Ok(())
}

#[tokio::test]
async fn closed_or_cancelled_creators_retire_ready_and_late_core_previews() -> TestResult {
    for closure in ["window", "shutdown", "cancellation"] {
        let fixture = fixture().await?;
        let ready = retained(&fixture, json!({})).await?;
        let token = CancellationToken::new();
        let reservation = fixture
            .state
            .settings
            .reserve("main", creator()?, token.clone())
            .map_err(boxed)?;
        let late = core_preview(&fixture.state, json!({})).await?;
        match closure {
            "window" => fixture.state.settings.close_window("main"),
            "shutdown" => fixture.state.settings.shutdown(),
            _ => token.cancel(),
        }
        assert_eq!(
            reservation
                .publish(&late)
                .err()
                .ok_or("closed creator published")?
                .code,
            "operation.cancelled"
        );
        assert!(
            fixture
                .state
                .settings
                .acquire_apply(
                    "main",
                    late.preview_id,
                    &late.approval_sha256,
                    late.library_revision
                )
                .is_err()
        );
        discarded(&fixture.state, &late).await?;
        if closure == "cancellation" {
            // Cancelling a creator must not close an unrelated ready review.
            assert_eq!(
                apply(&fixture.window, &ready)?.map_err(boxed)?.result["changed"],
                false
            );
        } else {
            assert!(
                fixture
                    .state
                    .settings
                    .acquire_apply(
                        "main",
                        ready.preview_id,
                        &ready.approval_sha256,
                        ready.library_revision
                    )
                    .is_err()
            );
            discarded(&fixture.state, &ready).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn core_eviction_cannot_free_native_capacity_or_authorize_an_unowned_preview() -> TestResult {
    let fixture = fixture().await?;
    let mut reviews = Vec::new();
    for _ in 0..3 {
        reviews.push(retained(&fixture, json!({})).await?);
    }
    // The shared core cache removes the smallest retained preview ID; choose it
    // from the actual IDs rather than assuming timing or UUID generation order.
    let evicted = reviews
        .iter()
        .min_by_key(|preview| preview.preview_id)
        .ok_or("reviews missing")?;
    let unowned = core_preview(
        &fixture.state,
        json!({"locale":{"value":"en-GB","updatedAt":UPDATED}}),
    )
    .await?;
    assert_eq!(
        fixture
            .state
            .settings
            .reserve("main", creator()?, CancellationToken::new())
            .err()
            .ok_or("core eviction silently freed a native slot")?
            .code,
        "settings.preview_capacity"
    );
    assert_eq!(
        apply(&fixture.window, &unowned)?
            .err()
            .ok_or("core preview bypassed native custody")?["code"],
        "settings.preview_unavailable"
    );
    assert_eq!(setting(&fixture.window, "locale")?, Value::Null);
    assert_eq!(
        apply(&fixture.window, evicted)?
            .err()
            .ok_or("evicted authority applied")?["code"],
        "settings.preview_unavailable"
    );
    drop(available_slot(&fixture.state, "main").await?);
    // A denied native claim did not consume the independently retained core review.
    assert!(
        fixture
            .state
            .app
            .apply_nonsecret_settings(approval(&unowned)?, CancellationToken::new())
            .await
            .map_err(boxed)?
            .changed
    );
    for review in &reviews {
        fixture.state.settings.discard(
            "main",
            &PreviewTarget::Preview {
                preview_id: review.preview_id,
            },
        );
        discarded(&fixture.state, review).await?;
    }
    Ok(())
}

#[tokio::test]
async fn stale_library_apply_consumes_owned_review_without_overwriting_newer_settings() -> TestResult
{
    let fixture = fixture().await?;
    let review = retained(
        &fixture,
        json!({"locale":{"value":"en-GB","updatedAt":UPDATED}}),
    )
    .await?;
    update(&fixture.window, "units", &json!("us-customary"))?;
    let before = snapshot(&fixture.state).await?;
    let mut events = fixture
        .state
        .app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;
    assert_eq!(
        apply(&fixture.window, &review)?
            .err()
            .ok_or("stale review applied")?["category"],
        "conflict"
    );
    assert_eq!(snapshot(&fixture.state).await?, before);
    assert!(events.try_recv().map_err(boxed)?.is_none());
    assert_eq!(
        apply(&fixture.window, &review)?
            .err()
            .ok_or("failed owned review replayed")?["code"],
        "settings.preview_unavailable"
    );
    discarded(&fixture.state, &review).await?;
    Ok(())
}
