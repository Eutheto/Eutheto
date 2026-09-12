use super::*;
use crate::operations::OperationRegistry;
use crate::setup_boundary::{operation_cancel, operation_prepare, operation_release};
use crate::tests::{invoke_ipc_args, invoke_ok};
use eutheto_core::{AppDependencies, AppPaths, AppQuery, AppQueryResult};
use eutheto_types::{ApiResponseDto, FixedClock, FixedMonotonicClock, SystemIdGenerator};
use serde_json::json;
use std::error::Error;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}
fn next() -> TestResult<RequestId> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}
fn prepared(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    scenario: ScenarioId,
    revision: u32,
    kind: &str,
) -> TestResult<OperationId> {
    let response: ApiResponseDto<Value> = invoke_ok(
        window,
        "operation_prepare",
        &json!({
            "requestId":next()?, "schemaVersion":1, "purpose":{"kind":kind},
            "context":{"kind":"scenario","scenarioId":scenario,"expectedRevision":revision}
        }),
    )?;
    Ok(response.result["operationId"]
        .as_str()
        .ok_or("missing operation")?
        .parse()?)
}
fn heavy(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    request: Value,
) -> TestResult<Result<ApiResponseDto<Value>, Value>> {
    Ok(
        match invoke_ipc_args(
            window,
            command,
            Value::Object(serde_json::Map::from_iter([
                ("request".to_owned(), request),
                (
                    "onProgress".to_owned(),
                    Value::String("__CHANNEL__:42".to_owned()),
                ),
            ])),
        )? {
            Ok(body) => Ok(body.deserialize()?),
            Err(error) => Err(error),
        },
    )
}
async fn people(state: &DesktopState, scenario: ScenarioId) -> TestResult<Value> {
    match state
        .app
        .query(AppQuery::ScenarioView(scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::Scenario(view) => Ok(serde_json::to_value(view.document.domain.entities)?),
        _ => Err("unexpected scenario query".into()),
    }
}

// Keep snapshot, reviewed commit, retained report and undo evidence in one native flow.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn csv_native_snapshot_approval_commit_reports_and_undo_remain_separate() -> TestResult {
    let directory = tempfile::tempdir()?;
    let clock = Arc::new(FixedClock::new("2026-09-10T12:00:00Z".parse()?));
    let monotonic = Arc::new(FixedMonotonicClock::default());
    let ids = Arc::new(SystemIdGenerator);
    let app = eutheto_core::EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("csv.sqlite3"),
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
    let desktop = tauri::test::mock_builder()
        .channel_interceptor(|_, _, _, _| true)
        .invoke_handler(tauri::generate_handler![
            crate::project_create,
            crate::scenario_undo,
            crate::scenario_redo,
            operation_prepare,
            operation_cancel,
            operation_release,
            people_csv_source_open,
            people_csv_source_close,
            people_csv_detect,
            people_csv_preview,
            people_csv_apply,
            people_csv_preview_discard,
            people_csv_rejected_rows,
        ])
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let window =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    let observer =
        tauri::WebviewWindowBuilder::new(&desktop, "observer", tauri::WebviewUrl::default())
            .build()?;
    let created: ApiResponseDto<Value> = invoke_ok(
        &window,
        "project_create",
        &json!({
            "schemaVersion":1,
            "requestId":next()?, "title":"CSV native integration", "description":"",
            "domainPack":{"id":"official.workforce","schemaVersion":1},
            "settings":{"timeZone":"UTC","locale":"en-US","units":"metric",
                "firstDate":"2026-09-01","lastDate":"2026-09-01","gapPolicy":"reject","overlapPolicy":"earlier"}
        }),
    )?;
    let scenario: ScenarioId = created.result["scenarioId"]
        .as_str()
        .ok_or("missing scenario")?
        .parse()?;

    // The cancelled command cannot reach the OS picker; this mock app has no dialog plugin.
    let operation = prepared(&window, scenario, 0, "csvSourceOpen")?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &window,
        "operation_cancel",
        &json!({
            "schemaVersion":1,"requestId":next()?,"operationId":operation
        }),
    )?;
    let cancelled = heavy(&window, "people_csv_source_open", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":0
    }))?.err().ok_or("cancelled acquisition must not select a file")?;
    assert_eq!(cancelled["code"], "operation.cancelled");

    // Real path acquisition supplies custody. Picker selection itself is a separate native smoke.
    let path = directory.path().join("people.csv");
    std::fs::write(&path, b"external,name\none,River\nbad,\n")?;
    let source = state
        .csv
        .reserve_source(
            "main",
            scenario,
            Creator {
                operation_id: OperationId::new(&SystemIdGenerator)?,
                request_id: next()?,
            },
            CancellationToken::new(),
        )
        .map_err(boxed)?
        .publish(
            crate::native_file::read_bounded_file(
                &path,
                MAX_CSV_SOURCE_BYTES,
                &CancellationToken::new(),
            )
            .map_err(boxed)?,
        )
        .map_err(boxed)?;
    let operation = prepared(&window, scenario, 0, "csvDetect")?;
    let detection = heavy(&window, "people_csv_detect", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":0,"sourceId":source
    }))?.map_err(boxed)?;
    assert_eq!(detection.current_revision, None);
    assert_eq!(
        detection.result["dialects"][0]["samples"][0]["cells"][0]["text"],
        "external"
    );
    let person = next()?;
    let rejected_person = next()?;
    let mapping = json!({"dialect":"comma","hasHeader":true,"expectedColumns":2,
        "columns":[{"index":0,"field":"externalId","blank":"preserve"},{"index":1,"field":"name","blank":"preserve"}],
        "newPersonDefaults":{"activeRange":{"kind":"always"},"qualificationGrants":[],"eligibleAssignmentTypeIds":[],
            "workloadWeight":{"numerator":1,"denominator":1},"tags":[],"teamIds":[]},"referenceMappings":{}});
    let operation = prepared(&window, scenario, 0, "csvPreview")?;
    let oversized = heavy(&window, "people_csv_preview", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":0,"sourceId":source,
        "mapping":mapping,"decisions":vec![json!({"record":2,"decision":{"kind":"skip"}}); 10_001]
    }))?.err().ok_or("oversized CSV input must be rejected before reserving heavy work")?;
    assert_eq!(oversized["code"], "people_csv.request_too_large");
    // Rejected admission leaves the same prepared operation available for bounded input.
    let preview = heavy(&window, "people_csv_preview", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":0,"sourceId":source,
        "mapping":mapping,"decisions":[{"record":2,"decision":{"kind":"add","personId":person}},
            {"record":3,"decision":{"kind":"add","personId":rejected_person}}]
    }))?.map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "reviewable");
    assert_eq!(
        preview.result["preview"]["rejectedRows"],
        json!([{"record":3,"code":"missingName"}])
    );
    assert!(
        people(&state, scenario)
            .await?
            .get(person.to_string())
            .is_none()
    );
    let preview_id: RequestId = preview.result["previewId"]
        .as_str()
        .ok_or("missing preview")?
        .parse()?;
    let approved = preview.result["preview"]["approvalDigest"]
        .as_str()
        .ok_or("missing approval")?;
    let forbidden = invoke_ipc_args(
        &observer,
        "people_csv_rejected_rows",
        json!({"request":{
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview_id
        }}),
    )?
    .err()
    .ok_or("another window must not read a retained report")?;
    assert_eq!(forbidden["code"], "people_csv.preview_unavailable");
    let operation = prepared(&window, scenario, 1, "csvApply")?;
    let wrong_revision = heavy(&window, "people_csv_apply", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":1,
        "sourceId":source,"previewId":preview_id,"approvedDigest":approved,"commandId":CommandId::new(&SystemIdGenerator)?,
        "actor":{"actorId":null,"displayName":"Native CSV check"},"truncateRedo":false
    }))?.err().ok_or("a different revision must not reuse approval")?;
    assert_eq!(wrong_revision["code"], "people_csv.preview_unavailable");
    std::fs::write(&path, b"external,name\none,Changed after selection\n")?;
    let operation = prepared(&window, scenario, 0, "csvApply")?;
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(&window, "people_csv_apply", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":0,
        "sourceId":source,"previewId":preview_id,"approvedDigest":approved,"commandId":command,
        "actor":{"actorId":null,"displayName":"Native CSV check"},"truncateRedo":false
    }))?.map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":1})
    );
    assert_eq!(
        people(&state, scenario).await?[person.to_string()]["name"],
        "River"
    );
    assert!(state.csv.source("main", scenario, source).is_err());
    let report: ApiResponseDto<Value> = invoke_ok(
        &window,
        "people_csv_rejected_rows",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview_id
        }),
    )?;
    assert_eq!(report.current_revision, None);
    assert_eq!(report.result["consumed"], true);
    assert_eq!(
        report.result["rejectedRows"],
        json!([{"record":3,"code":"missingName"}])
    );
    let _: ApiResponseDto<Value> = invoke_ok(
        &window,
        "scenario_undo",
        &json!({"requestId":next()?,"scenarioId":scenario,"expectedRevision":1}),
    )?;
    assert!(
        people(&state, scenario)
            .await?
            .get(person.to_string())
            .is_none()
    );
    let _: ApiResponseDto<Value> = invoke_ok(
        &window,
        "scenario_redo",
        &json!({"requestId":next()?,"scenarioId":scenario,"expectedRevision":2}),
    )?;
    assert_eq!(
        people(&state, scenario).await?[person.to_string()]["name"],
        "River"
    );
    let retained: ApiResponseDto<Value> = invoke_ok(
        &window,
        "people_csv_rejected_rows",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview_id
        }),
    )?;
    assert_eq!(retained.result, report.result);
    Ok(())
}
