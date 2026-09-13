use std::{error::Error, sync::Arc};

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, EuthetoApp, HistoryPageContinuationV1,
    HistoryPageDtoV1,
};
use eutheto_types::{
    ApiResponseDto, CancellationToken, CommandId, CommandResult, DomainPackRef, EntityId,
    RequestId, Revision, ScenarioId, ScenarioSettings, SystemClock, SystemIdGenerator,
};
use serde_json::{Value, json};

use super::tests::{invoke_ipc_args, invoke_ok};
use super::{
    DesktopState, decode_history_request, scenario_apply_command, scenario_get_history_page,
    scenario_redo, scenario_undo,
};

fn request(
    scenario_id: ScenarioId,
    revision: u64,
    limit: u32,
    continuation: Option<&HistoryPageContinuationV1>,
) -> Result<Value, Box<dyn Error>> {
    Ok(json!({
        "requestId": RequestId::new(&SystemIdGenerator)?,
        "schemaVersion": 1,
        "scenarioId": scenario_id,
        "expectedRevision": revision,
        "limit": limit,
        "continuation": continuation,
    }))
}

#[test]
fn history_admission_rejects_oversize_before_decode_and_invalid_context()
-> Result<(), Box<dyn Error>> {
    let scenario_id = "01900000-0000-7000-8000-000000000001".parse()?;
    let valid = request(scenario_id, 0, 100, None)?;
    let mut oversized = valid.clone();
    // JSON escaping, not just raw text length, consumes the admission budget.
    oversized["privateExtra"] = json!("\0".repeat(1024));
    let error = decode_history_request(Some(oversized))
        .err()
        .ok_or("oversized request admitted")?;
    assert_eq!(error.code, "history.request_too_large");
    assert!(!format!("{error:?}").contains("privateExtra"));

    let mut malformed = valid.clone();
    malformed["privateExtra"] = json!(true);
    assert_eq!(
        decode_history_request(Some(malformed))
            .err()
            .ok_or("unknown field admitted")?
            .code,
        "history.request_invalid"
    );
    let mut newer = valid.clone();
    newer["schemaVersion"] = json!(2);
    assert_eq!(
        decode_history_request(Some(newer))
            .err()
            .ok_or("newer version admitted")?
            .code,
        "history.schema_version_unsupported"
    );
    let mut over_limit = valid.clone();
    over_limit["limit"] = json!(101);
    assert_eq!(
        decode_history_request(Some(over_limit))
            .err()
            .ok_or("excess page limit admitted")?
            .code,
        "history.limit_invalid"
    );
    let mut stale_cursor = valid;
    stale_cursor["continuation"] =
        json!({"scenarioId": scenario_id, "revision": 1, "beforeSequence": 1});
    assert_eq!(
        decode_history_request(Some(stale_cursor))
            .err()
            .ok_or("mismatched continuation admitted")?
            .code,
        "history.continuation_invalid"
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real IPC journal lifecycle keeps revision and privacy evidence together.
async fn history_ipc_pages_metadata_and_refreshes_after_undo_redo() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let app = EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("history.sqlite3"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::default(),
    })
    .await
    .map_err(|error| format!("history app setup failed: {error:?}"))?;
    let settings: ScenarioSettings = serde_json::from_value(json!({
        "timeZone": "UTC", "locale": "en-US", "units": "metric",
        "horizon": {"start": "2026-09-01T00:00:00Z", "end": "2026-10-01T00:00:00Z"},
        "gapPolicy": "reject", "overlapPolicy": "earlier",
    }))?;
    let AppCommandResult::Project(project) = app
        .execute(AppCommand::CreateProject {
            request_id: RequestId::new(&SystemIdGenerator)?,
            title: "History fixture".into(),
            description: String::new(),
            domain_pack: DomainPackRef {
                id: "official.test".parse()?,
                schema_version: 1,
            },
            settings,
        })
        .await
        .map_err(|error| format!("history project setup failed: {error:?}"))?
    else {
        return Err("unexpected project result".into());
    };
    let scenario_id = project.scenario_id;
    let desktop = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            scenario_get_history_page,
            scenario_apply_command,
            scenario_undo,
            scenario_redo
        ])
        .manage(DesktopState::new(
            app,
            directory.path().join("cache"),
            directory.path().join("backups"),
        ))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let webview =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    let empty: ApiResponseDto<HistoryPageDtoV1> = invoke_ok(
        &webview,
        "scenario_get_history_page",
        &request(scenario_id, 0, 1, None)?,
    )?;
    assert!(empty.result.entries.is_empty());
    assert!(!empty.result.undo_available && !empty.result.redo_available);

    let mut command_ids = Vec::new();
    for revision in 0..2 {
        let entity_id = EntityId::new(&SystemIdGenerator)?;
        let command_id = CommandId::new(&SystemIdGenerator)?;
        command_ids.push(command_id);
        let committed: ApiResponseDto<CommandResult> = invoke_ok(
            &webview,
            "scenario_apply_command",
            &json!({
                "requestId": RequestId::new(&SystemIdGenerator)?, "commandId": command_id,
                "scenarioId": scenario_id, "expectedRevision": revision,
                "actor": {"actorId": "private-actor-marker", "displayName": "Private actor marker"},
                "command": {"type": "addEntity", "payload": {"entityId": entity_id, "value": {"id": entity_id, "name": "private-payload-marker"}}},
                "truncateRedo": false,
            }),
        )?;
        assert_eq!(committed.result.new_revision, Revision::new(revision + 1));
    }
    let first_request = request(scenario_id, 2, 1, None)?;
    let first: ApiResponseDto<HistoryPageDtoV1> =
        invoke_ok(&webview, "scenario_get_history_page", &first_request)?;
    assert_eq!(first.current_revision, Some(Revision::new(2)));
    assert_eq!(first.result.revision, Revision::new(2));
    assert_eq!(
        first
            .result
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![command_ids[1]]
    );
    assert!(first.result.undo_available && !first.result.redo_available);
    let cursor = first.result.continuation.ok_or("missing next page")?;
    let second: ApiResponseDto<Value> = invoke_ok(
        &webview,
        "scenario_get_history_page",
        &request(scenario_id, 2, 1, Some(&cursor))?,
    )?;
    let wire = serde_json::to_string(&second)?;
    assert!(!wire.contains("private-payload-marker"));
    assert!(!wire.contains("private-actor-marker"));
    let second_page: HistoryPageDtoV1 = serde_json::from_value(second.result.clone())?;
    assert_eq!(
        second_page
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![command_ids[0]]
    );
    assert!(second_page.continuation.is_none());
    for forbidden in ["command", "inverse", "actor"] {
        assert!(second.result["entries"][0].get(forbidden).is_none());
    }

    let undone: ApiResponseDto<CommandResult> = invoke_ok(
        &webview,
        "scenario_undo",
        &json!({"requestId": RequestId::new(&SystemIdGenerator)?, "scenarioId": scenario_id, "expectedRevision": 2}),
    )?;
    assert_eq!(undone.result.new_revision, Revision::new(3));
    let stale = invoke_ipc_args(
        &webview,
        "scenario_get_history_page",
        json!({"request": first_request}),
    )?;
    let Err(stale) = stale else {
        return Err("stale history revision admitted".into());
    };
    assert_eq!(stale["category"], "conflict");
    let moved_cursor = invoke_ipc_args(
        &webview,
        "scenario_get_history_page",
        json!({"request": request(scenario_id, 3, 1, Some(&cursor))?}),
    )?;
    let Err(moved_cursor) = moved_cursor else {
        return Err("cursor moved across revisions".into());
    };
    assert_eq!(moved_cursor["code"], "history.continuation_invalid");
    let after_undo: ApiResponseDto<HistoryPageDtoV1> = invoke_ok(
        &webview,
        "scenario_get_history_page",
        &request(scenario_id, 3, 1, None)?,
    )?;
    assert!(!after_undo.result.entries[0].applied);
    assert!(after_undo.result.undo_available && after_undo.result.redo_available);
    let redone: ApiResponseDto<CommandResult> = invoke_ok(
        &webview,
        "scenario_redo",
        &json!({"requestId": RequestId::new(&SystemIdGenerator)?, "scenarioId": scenario_id, "expectedRevision": 3}),
    )?;
    assert_eq!(redone.result.new_revision, Revision::new(4));
    let after_redo: ApiResponseDto<HistoryPageDtoV1> = invoke_ok(
        &webview,
        "scenario_get_history_page",
        &request(scenario_id, 4, 2, None)?,
    )?;
    assert!(after_redo.result.entries.iter().all(|entry| entry.applied));
    assert!(after_redo.result.undo_available && !after_redo.result.redo_available);
    Ok(())
}
