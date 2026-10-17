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

struct NativeCsvFixture {
    directory: tempfile::TempDir,
    state: DesktopState,
    _desktop: tauri::App<tauri::test::MockRuntime>,
    window: tauri::WebviewWindow<tauri::test::MockRuntime>,
    observer: tauri::WebviewWindow<tauri::test::MockRuntime>,
    scenario: ScenarioId,
    close_on_response: Arc<std::sync::atomic::AtomicBool>,
}

async fn native_fixture() -> TestResult<NativeCsvFixture> {
    native_fixture_in_zone("UTC").await
}

#[allow(clippy::too_many_lines)]
async fn native_fixture_in_zone(time_zone: &str) -> TestResult<NativeCsvFixture> {
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
    let close_on_response = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let desktop = tauri::test::mock_builder()
        .channel_interceptor({
            let close_on_response = Arc::clone(&close_on_response);
            let state = state.clone();
            move |window, _, _, body| {
                if close_on_response.load(std::sync::atomic::Ordering::SeqCst)
                    && let tauri::ipc::InvokeResponseBody::Json(body) = body
                    && let Ok(event) = serde_json::from_str::<Value>(body)
                    && event["phase"] == "preparingResponse"
                    && close_on_response.swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    // Deterministic window teardown after publication, before response encoding.
                    state.operations.cancel_window(window.label());
                    state.csv.close_window(window.label());
                }
                true
            }
        })
        .invoke_handler(tauri::generate_handler![
            crate::project_create,
            crate::scenario_undo,
            crate::scenario_apply_batch,
            crate::scenario_redo,
            operation_prepare,
            operation_cancel,
            operation_release,
            csv_source_open,
            eligibility_matrix_source_open,
            csv_source_close,
            csv_detect,
            csv_record_sample,
            people_csv_preview,
            assignment_csv_preview,
            eligibility_csv_preview,
            eligibility_matrix_preview,
            shift_csv_preview,
            availability_csv_preview,
            csv_apply,
            csv_preview_discard,
            csv_report,
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
            "settings":{"timeZone":time_zone,"locale":"en-US","units":"metric",
                "firstDate":"2026-09-01","lastDate":"2026-09-01","gapPolicy":"reject","overlapPolicy":"earlier"}
        }),
    )?;
    let scenario: ScenarioId = created.result["scenarioId"]
        .as_str()
        .ok_or("missing scenario")?
        .parse()?;
    Ok(NativeCsvFixture {
        directory,
        state,
        _desktop: desktop,
        window,
        observer,
        scenario,
        close_on_response,
    })
}

// Keep snapshot, reviewed commit, retained report and undo evidence in one native flow.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn csv_native_snapshot_approval_commit_reports_and_undo_remain_separate() -> TestResult {
    let NativeCsvFixture {
        directory,
        state,
        _desktop,
        window,
        observer,
        scenario,
        ..
    } = native_fixture().await?;

    // The cancelled command cannot reach the OS picker; this mock app has no dialog plugin.
    let operation = prepared(&window, scenario, 0, "csvSourceOpen")?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &window,
        "operation_cancel",
        &json!({
            "schemaVersion":1,"requestId":next()?,"operationId":operation
        }),
    )?;
    let cancelled = heavy(&window, "csv_source_open", json!({
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
    let detection = heavy(&window, "csv_detect", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":0,"sourceId":source,"family":"people"
    }))?.map_err(boxed)?;
    assert_eq!(detection.current_revision, None);
    assert_eq!(
        detection.result["data"]["dialects"][0]["samples"][0]["cells"][0]["text"],
        "external"
    );
    let operation = prepared(&window, scenario, 0, "csvSample")?;
    let invalid = heavy(
        &window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,
            "expectedRevision":0,"sourceId":source,"readFormat":{"family":"people","dialect":"comma"},"record":0
        }),
    )?
    .err()
    .ok_or("zero is not a logical record")?;
    assert_eq!(invalid["code"], "people_csv.invalid_record");
    let sample = heavy(
        &window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,
            "expectedRevision":0,"sourceId":source,"readFormat":{"family":"people","dialect":"comma"},"record":3
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(sample.current_revision, None);
    assert_eq!(
        sample.result,
        json!({
            "schemaVersion":1,"sourceId":source,"readFormat":{"family":"people","dialect":"comma"},"record":3,
            "cells":[{"text":"bad","truncated":false},{"text":"","truncated":false}]
        })
    );
    let absent = heavy(&window, "csv_record_sample", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":prepared(&window, scenario, 0, "csvSample")?,
        "scenarioId":scenario,"expectedRevision":0,"sourceId":source,"readFormat":{"family":"people","dialect":"comma"},"record":4
    }))?.map_err(boxed)?;
    assert_eq!(absent.result["cells"], Value::Null);
    let wrong_context = heavy(&window, "csv_record_sample", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":prepared(&window, scenario, 0, "csvSample")?,
        "scenarioId":scenario,"expectedRevision":1,"sourceId":source,"readFormat":{"family":"people","dialect":"comma"},"record":3
    }))?.err().ok_or("request revision must match prepared context")?;
    assert_eq!(wrong_context["code"], "operation.claim_mismatch");
    let denied = heavy(&observer, "csv_record_sample", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":prepared(&observer, scenario, 0, "csvSample")?,
        "scenarioId":scenario,"expectedRevision":0,"sourceId":source,"readFormat":{"family":"people","dialect":"comma"},"record":3
    }))?.err().ok_or("another window must not inspect the source")?;
    assert_eq!(denied["code"], "people_csv.source_unavailable");
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
        "csv_report",
        json!({"request":{
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview_id
        }}),
    )?
    .err()
    .ok_or("another window must not read a retained report")?;
    assert_eq!(forbidden["code"], "people_csv.preview_unavailable");
    let operation = prepared(&window, scenario, 1, "csvApply")?;
    let wrong_revision = heavy(&window, "csv_apply", json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,"scenarioId":scenario,"expectedRevision":1,
        "sourceId":source,"previewId":preview_id,"approvedDigest":approved,"commandId":CommandId::new(&SystemIdGenerator)?,
        "actor":{"actorId":null,"displayName":"Native CSV check"},"truncateRedo":false
    }))?.err().ok_or("a different revision must not reuse approval")?;
    assert_eq!(wrong_revision["code"], "people_csv.preview_unavailable");
    std::fs::write(&path, b"external,name\none,Changed after selection\n")?;
    let operation = prepared(&window, scenario, 0, "csvApply")?;
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(&window, "csv_apply", json!({
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
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview_id
        }),
    )?;
    assert_eq!(report.current_revision, None);
    assert_eq!(report.result["consumed"], true);
    assert_eq!(report.result["rows"]["family"], "people");
    assert_eq!(
        report.result["rows"]["rejectedRows"],
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
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview_id
        }),
    )?;
    assert_eq!(retained.result, report.result);
    Ok(())
}

fn retained_source(fixture: &NativeCsvFixture, bytes: Vec<u8>) -> TestResult<SourceId> {
    fixture
        .state
        .csv
        .reserve_source(
            "main",
            fixture.scenario,
            Creator {
                operation_id: OperationId::new(&SystemIdGenerator)?,
                request_id: next()?,
            },
            CancellationToken::new(),
        )
        .map_err(boxed)?
        .publish(bytes)
        .map_err(boxed)
}

#[allow(clippy::too_many_arguments)]
fn assignment_preview_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    source: SourceId,
    operation: OperationId,
    mapping: &Value,
    target: &Value,
    skips: &Value,
    allow_empty: bool,
) -> TestResult<Value> {
    Ok(json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"sourceId":source,
        "mapping":mapping,"target":target,"skippedRecords":skips,"allowEmpty":allow_empty
    }))
}

fn apply_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    source: SourceId,
    preview: &Value,
    command: CommandId,
) -> TestResult<Value> {
    Ok(json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, revision, "csvApply")?,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"sourceId":source,
        "previewId":preview["previewId"],"approvedDigest":preview["preview"]["approvalDigest"],
        "commandId":command,"actor":{"actorId":null,"displayName":"Native assignment import"},
        "truncateRedo":false
    }))
}

async fn document(fixture: &NativeCsvFixture) -> TestResult<eutheto_types::ScenarioDocument> {
    match fixture
        .state
        .app
        .query(AppQuery::ScenarioView(fixture.scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::Scenario(view) => Ok(view.document),
        _ => Err("unexpected scenario query".into()),
    }
}

async fn assert_history(fixture: &NativeCsvFixture, count: usize, last: CommandId) -> TestResult {
    match fixture
        .state
        .app
        .query(AppQuery::History(fixture.scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::History(history) => {
            assert_eq!(history.len(), count);
            assert_eq!(history.last().ok_or("missing history")?.id, last);
            Ok(())
        }
        _ => Err("unexpected history query".into()),
    }
}

fn endpoint(index: u32) -> String {
    format!("018f7b40-a000-7000-8000-{index:012x}")
}

fn seed_assignment_endpoints(fixture: &NativeCsvFixture) -> TestResult<CommandId> {
    let mut entities = vec![
        json!({"kind":"person","id":endpoint(1),"name":"River","activeRange":{"kind":"always"},
            "qualificationGrants":[],"eligibleAssignmentTypeIds":[],
            "workloadWeight":{"numerator":1,"denominator":1},"tags":[],"teamIds":[]}),
        json!({"kind":"assignmentType","id":endpoint(4),"name":"Clinic","category":"clinic",
            "timeBehavior":"elapsed","defaultDurationMinutes":60,
            "qualifications":{"kind":"unconstrained"},
            "locationBehavior":{"kind":"required"},"workloadBucketIds":[]}),
        json!({"kind":"location","id":endpoint(5),"name":"North","transitions":[]}),
    ];
    for (index, start, end) in [(8, "09:00:00", "10:00:00"), (9, "10:00:00", "11:00:00")] {
        entities.push(json!({"kind":"shiftInstance","id":endpoint(index),
            "assignmentTypeId":endpoint(4),"locationId":endpoint(5),
            "startsAt":{"instant":format!("2026-09-01T{start}Z"),"local":format!("2026-09-01T{start}"),"offsetSeconds":0},
            "endsAt":{"instant":format!("2026-09-01T{end}Z"),"local":format!("2026-09-01T{end}"),"offsetSeconds":0},
            "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]},"tags":[],
            "reportingAttribution":"startLocalDate","origin":{"kind":"manual"}}));
    }
    let command = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":command,"scenarioId":fixture.scenario,"expectedRevision":0,
            "actor":{"actorId":null,"displayName":"Seed native endpoints"},"truncateRedo":false,
            "label":"Create assignment import endpoints",
            "commands":entities.into_iter().map(|entity| json!({
                "type":"applyDomainCommand",
                "payload":{"commandType":"official.workforce.add_entity","payload":{"entity":entity}}
            })).collect::<Vec<_>>()
        }),
    )?;
    Ok(command)
}

const ASSIGNMENT_FRAMING: &str = "eutheto/assignments,1\nassignment_id,person_id,shift_id\n";

fn canonical_row(person: u32, shift: u32) -> String {
    format!(
        "official.workforce.assignment.{}.{},{},{}\n",
        endpoint(person),
        endpoint(shift),
        endpoint(person),
        endpoint(shift)
    )
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn assignment_csv_native_complete_replacement_custody_receipt_and_undo() -> TestResult {
    let fixture = native_fixture().await?;
    let seed = seed_assignment_endpoints(&fixture)?;
    let before = document(&fixture).await?;
    let bytes = format!(
        "{ASSIGNMENT_FRAMING}{}{}",
        canonical_row(1, 8),
        canonical_row(999, 8)
    );
    let source = retained_source(&fixture, bytes.as_bytes().to_vec())?;
    let detection = heavy(&fixture.window, "csv_detect", json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvDetect")?,
        "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,"family":"assignments"
    }))?.map_err(boxed)?;
    assert_eq!(detection.result["family"], "assignments");
    assert_eq!(detection.result["data"]["format"], "canonicalV1");
    assert_eq!(detection.result["data"]["source"]["logicalRecords"], 4);
    let sample = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvSample")?,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
            "readFormat":{"family":"assignments","format":{"format":"canonicalV1"}},"record":3
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(sample.result["cells"][1]["text"], endpoint(1));
    let sample_operation = prepared(&fixture.window, fixture.scenario, 1, "csvSample")?;
    let error = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,"operationId":sample_operation,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
            "readFormat":{"family":"assignments","format":{"format":"canonicalV1"}},"record":10_003
        }),
    )?
    .err()
    .ok_or("canonical sample bound must refuse above the framing allowance")?;
    assert_eq!(error["code"], "people_csv.invalid_record");
    let absent = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,"operationId":sample_operation,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
            "readFormat":{"family":"assignments","format":{"format":"canonicalV1"}},"record":10_002
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(
        absent.result["cells"],
        Value::Null,
        "canonical sampling admits two framing records"
    );
    let target = json!({"kind":"create","id":endpoint(20),"name":"Unverified native plan"});
    let operation = prepared(&fixture.window, fixture.scenario, 1, "csvPreview")?;
    let blocked = heavy(
        &fixture.window,
        "assignment_csv_preview",
        assignment_preview_request(
            &fixture,
            1,
            source,
            operation,
            &json!({"format":"canonicalV1"}),
            &target,
            &json!([]),
            false,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(blocked.result["preview"]["disposition"], "blocked");
    assert_eq!(blocked.result["preview"]["approvalDigest"], Value::Null);
    let preview = heavy(
        &fixture.window,
        "assignment_csv_preview",
        assignment_preview_request(
            &fixture,
            1,
            source,
            prepared(&fixture.window, fixture.scenario, 1, "csvPreview")?,
            &json!({"format":"canonicalV1"}),
            &target,
            &json!([4]),
            false,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "reviewable");
    assert_eq!(
        preview.result["preview"]["excludedRows"],
        json!([{"record":4,"skipped":true,"issue":"unresolvedPerson"}])
    );
    assert_eq!(document(&fixture).await?, before);
    assert_history(&fixture, 1, seed).await?;

    let foreign_scenario = ScenarioId::new(&SystemIdGenerator)?;
    assert_eq!(
        fixture
            .state
            .csv
            .source("main", foreign_scenario, source)
            .err()
            .ok_or("source custody must bind the exact scenario")?
            .code,
        "people_csv.source_unavailable"
    );
    for (window, scenario) in [
        (&fixture.observer, fixture.scenario),
        (&fixture.window, foreign_scenario),
    ] {
        let denied = invoke_ipc_args(window, "csv_report", json!({"request":{
            "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,"previewId":preview.result["previewId"]
        }}))?.err().ok_or("report owner mismatch must refuse")?;
        assert_eq!(denied["code"], "people_csv.preview_unavailable");
    }
    let observer_preview = heavy(&fixture.observer, "assignment_csv_preview", json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.observer, fixture.scenario, 1, "csvPreview")?,
        "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
        "mapping":{"format":"canonicalV1"},"target":{"kind":"create","id":endpoint(21),"name":"Denied"},
        "skippedRecords":[4],"allowEmpty":false
    }))?.err().ok_or("source cannot transfer windows")?;
    assert_eq!(observer_preview["code"], "people_csv.source_unavailable");
    let command = CommandId::new(&SystemIdGenerator)?;
    let other_source = retained_source(&fixture, bytes.into_bytes())?;
    let denied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 1, other_source, &preview.result, command)?,
    )?
    .err()
    .ok_or("identical bytes do not transfer preview source custody")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    fixture.state.csv.close_source(
        "main",
        fixture.scenario,
        &SourceTarget::Source {
            source_id: other_source,
        },
    );
    let denied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &preview.result, command)?,
    )?
    .err()
    .ok_or("preview approval cannot move revisions")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    let mut wrong_digest = apply_request(&fixture, 1, source, &preview.result, command)?;
    wrong_digest["approvedDigest"] = json!("0".repeat(64));
    let denied = heavy(&fixture.window, "csv_apply", wrong_digest)?
        .err()
        .ok_or("digest mismatch must refuse without consuming review")?;
    assert_eq!(denied["code"], "people_csv.approval_mismatch");
    assert_history(&fixture, 1, seed).await?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 1, source, &preview.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":2})
    );
    assert_eq!(applied.current_revision, Some(Revision::new(2)));
    assert_eq!(applied.result["report"]["rows"]["family"], "assignments");
    assert_eq!(
        applied.result["report"]["rows"]["excludedRows"],
        preview.result["preview"]["excludedRows"]
    );
    assert_eq!(applied.result["report"]["consumed"], true);
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_err()
    );
    assert_history(&fixture, 2, command).await?;
    let imported = document(&fixture).await?;
    let plan = &imported.domain.entities[&endpoint(20).parse()?];
    assert_eq!(plan["kind"], "externalStartingPlan");
    assert_eq!(
        plan["assignments"],
        json!([{"personId":endpoint(1),"shiftId":endpoint(8)}])
    );
    assert!(plan.get("sourceSolutionId").is_none() && plan.get("sourceRevision").is_none());
    assert_eq!(
        plan["originalImport"]["sourceBlake3"],
        preview.result["preview"]["source"]["blake3"]
    );
    assert_eq!(
        imported.domain.locked_assignments,
        before.domain.locked_assignments
    );
    assert_eq!(imported.domain.rules, before.domain.rules);
    assert!(
        imported
            .domain
            .entities
            .values()
            .all(|entity| entity["kind"] != "baseSchedule" && entity["kind"] != "manualSelection")
    );
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,"previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_undo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":2
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, before.domain);
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_redo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":3
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, imported.domain);
    // Mapped aliases replace the whole pair set, not a patch that leaves the old shift behind.
    let mapped_bytes = b"person;shift\nreviewed person;reviewed work\n".to_vec();
    let mapped_source = retained_source(&fixture, mapped_bytes)?;
    let mapping = json!({"format":"mapped","dialect":"semicolon","hasHeader":true,"expectedColumns":2,
        "personColumn":0,"shiftColumn":1,"personMappings":{"reviewed person":endpoint(1)},
        "shiftMappings":{"reviewed work":endpoint(9)}});
    let mapped = heavy(
        &fixture.window,
        "assignment_csv_preview",
        assignment_preview_request(
            &fixture,
            4,
            mapped_source,
            prepared(&fixture.window, fixture.scenario, 4, "csvPreview")?,
            &mapping,
            &json!({"kind":"replace","id":endpoint(20),"name":"Reviewed replacement"}),
            &json!([]),
            false,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(mapped.result["preview"]["disposition"], "reviewable");
    assert_eq!(
        mapped.result["preview"]["changes"]["removedPairs"],
        json!([{"personId":endpoint(1),"shiftId":endpoint(8)}])
    );
    assert_eq!(
        mapped.result["preview"]["changes"]["addedPairs"],
        json!([{"personId":endpoint(1),"shiftId":endpoint(9)}])
    );
    let replacement_command = CommandId::new(&SystemIdGenerator)?;
    let replaced = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            4,
            mapped_source,
            &mapped.result,
            replacement_command,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(replaced.result["outcome"]["revision"], 5);
    assert_history(&fixture, 3, replacement_command).await?;
    let replacement = document(&fixture).await?;
    assert_eq!(
        replacement.domain.entities[&endpoint(20).parse()?]["assignments"],
        json!([{"personId":endpoint(1),"shiftId":endpoint(9)}])
    );
    assert_eq!(
        replacement.domain.locked_assignments,
        before.domain.locked_assignments
    );
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_undo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":5
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, imported.domain);
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_redo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":6
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, replacement.domain);
    let retained: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,"previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(
        retained.result, report.result,
        "Undo/Redo cannot rewrite a committed import receipt"
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn assignment_csv_native_complete_exclusions_and_predecode_admission() -> TestResult {
    let fixture = native_fixture().await?;
    let before = document(&fixture).await?;
    let bytes = format!("person,shift\n{}", "unknown,unknown\n".repeat(10_000));
    let source = retained_source(&fixture, bytes.into_bytes())?;
    let operation = prepared(&fixture.window, fixture.scenario, 0, "csvPreview")?;
    let mapping = json!({"format":"mapped","dialect":"comma","hasHeader":true,"expectedColumns":2,
        "personColumn":0,"shiftColumn":1,"personMappings":{},"shiftMappings":{}});
    let target =
        json!({"kind":"create","id":endpoint(20),"name":"Explicitly empty unverified plan"});
    let request = assignment_preview_request(
        &fixture,
        0,
        source,
        operation,
        &mapping,
        &target,
        &json!((2..=10_001).collect::<Vec<_>>()),
        true,
    )?;
    // These values are deliberately not typed inputs: bounds must win before deserialization.
    let mut oversized_decisions = request.clone();
    oversized_decisions["skippedRecords"] = json!(vec!["not a record"; 10_001]);
    let mut oversized_mapping = request.clone();
    oversized_mapping["mapping"] = json!({"invalid":"x".repeat(MAX_CSV_MAPPING_BYTES + 1)});
    let mut oversized_aliases = request.clone();
    oversized_aliases["mapping"]["personMappings"] = Value::Object(
        (0..10_001)
            .map(|index| (index.to_string(), Value::Null))
            .collect(),
    );
    for invalid in [oversized_decisions, oversized_mapping, oversized_aliases] {
        let error = heavy(&fixture.window, "assignment_csv_preview", invalid)?
            .err()
            .ok_or("predecode admission must refuse oversized values")?;
        assert_eq!(error["code"], "people_csv.request_too_large");
    }
    // Admission refusals must not claim the prepared operation or mutate the document.
    let preview = heavy(&fixture.window, "assignment_csv_preview", request)?.map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "reviewable");
    let excluded = preview.result["preview"]["excludedRows"]
        .as_array()
        .ok_or("missing excluded rows")?;
    assert_eq!(excluded.len(), 10_000);
    assert_eq!(
        excluded.first(),
        Some(&json!({"record":2,"skipped":true,"issue":"unresolvedPerson"}))
    );
    assert_eq!(
        excluded.last(),
        Some(&json!({"record":10_001,"skipped":true,"issue":"unresolvedPerson"}))
    );
    assert_eq!(document(&fixture).await?, before);
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 0, source, &preview.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":1})
    );
    assert_eq!(applied.result["report"]["rows"]["family"], "assignments");
    assert_eq!(
        applied.result["report"]["rows"]["excludedRows"],
        preview.result["preview"]["excludedRows"]
    );
    assert!(
        serde_json::to_vec(&applied.result)?.len() > 128 * 1024,
        "this must exercise the assignment receipt bound, not the People bound"
    );
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,"previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    assert_history(&fixture, 1, command).await?;
    let imported = document(&fixture).await?;
    assert_eq!(
        imported.domain.entities[&endpoint(20).parse()?]["assignments"],
        json!([])
    );
    assert_eq!(
        imported.domain.locked_assignments,
        before.domain.locked_assignments
    );
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_undo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":1
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, before.domain);
    Ok(())
}

#[tokio::test]
async fn assignment_csv_native_skips_never_rescue_malformed_reserved_source() -> TestResult {
    let fixture = native_fixture().await?;
    let before = document(&fixture).await?;
    let source = retained_source(
        &fixture,
        format!("{ASSIGNMENT_FRAMING}not-an-assignment,person,shift\n").into_bytes(),
    )?;
    let operation = prepared(&fixture.window, fixture.scenario, 0, "csvPreview")?;
    let target = json!({"kind":"create","id":endpoint(20),"name":"Must not commit"});
    let error = heavy(
        &fixture.window,
        "assignment_csv_preview",
        assignment_preview_request(
            &fixture,
            0,
            source,
            operation,
            &json!({"format":"canonicalV1"}),
            &target,
            &json!([3]),
            true,
        )?,
    )?
    .err()
    .ok_or("skips cannot bypass canonical identity checks")?;
    assert_eq!(error["code"], "assignment_csv.invalidIdentity");
    let error = heavy(
        &fixture.window,
        "assignment_csv_preview",
        assignment_preview_request(
            &fixture,
            0,
            source,
            prepared(&fixture.window, fixture.scenario, 0, "csvPreview")?,
            &json!({"format":"mapped","dialect":"comma","hasHeader":false,"expectedColumns":3,
            "personColumn":1,"shiftColumn":2,"personMappings":{},"shiftMappings":{}}),
            &target,
            &json!([1, 2, 3]),
            true,
        )?,
    )?
    .err()
    .ok_or("reserved input cannot downgrade into permissive mapped mode")?;
    assert!(
        error["code"]
            .as_str()
            .is_some_and(|code| code.starts_with("assignment_csv.")),
        "reserved source must fail native assignment admission, never return a reviewable mapped proposal"
    );
    assert_eq!(document(&fixture).await?, before);
    match fixture
        .state
        .app
        .query(AppQuery::History(fixture.scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::History(history) => assert!(history.is_empty()),
        _ => return Err("unexpected history query".into()),
    }
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_ok()
    );
    Ok(())
}

fn seed_eligibility_endpoints(fixture: &NativeCsvFixture) -> TestResult<CommandId> {
    let mut entities = vec![
        json!({"kind":"location","id":endpoint(5),"name":"North","transitions":[]}),
        json!({"kind":"person","id":endpoint(1),
        "name":"River","externalId":"staff-01","activeRange":{"kind":"always"},
        "qualificationGrants":[],"eligibleAssignmentTypeIds":[],
        "homeLocationId":endpoint(5).replace('-', ""),
        "workloadWeight":{"numerator":3,"denominator":2},"tags":["night","reviewed"],
        "teamIds":[],"display":{"color":"#102030","avatarInitials":"RV"}}),
    ];
    for index in [4, 21] {
        entities.push(json!({"kind":"assignmentType","id":endpoint(index),
            "name":format!("Clinic {index}"),"category":"clinic","timeBehavior":"elapsed",
            "defaultDurationMinutes":60,"qualifications":{"kind":"unconstrained"},
            "locationBehavior":{"kind":"required"},"workloadBucketIds":[]}));
    }
    let command = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":command,"scenarioId":fixture.scenario,"expectedRevision":0,
            "actor":{"actorId":null,"displayName":"Seed eligibility endpoints"},"truncateRedo":false,
            "label":"Create eligibility endpoints",
            "commands":entities.into_iter().map(|entity| json!({
                "type":"applyDomainCommand",
                "payload":{"commandType":"official.workforce.add_entity","payload":{"entity":entity}}
            })).collect::<Vec<_>>()
        }),
    )?;
    Ok(command)
}

fn eligibility_native_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    source: SourceId,
    operation: OperationId,
    skipped: &[u32],
) -> TestResult<Value> {
    Ok(json!({
        "schemaVersion":1,"requestId":next()?,"operationId":operation,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"sourceId":source,
        "mapping":{"dialect":"comma","hasHeader":true,"expectedColumns":3,
            "personColumn":0,"assignmentTypeColumn":1,"eligibleColumn":2,
            "personMappings":{},"assignmentTypeMappings":{}},"skippedRecords":skipped
    }))
}

fn eligibility_custody_refusals(
    fixture: &NativeCsvFixture,
    source: SourceId,
    preview: &Value,
    bytes: &[u8],
) -> TestResult {
    let observer = heavy(
        &fixture.observer,
        "eligibility_csv_preview",
        eligibility_native_request(
            fixture,
            1,
            source,
            prepared(&fixture.observer, fixture.scenario, 1, "csvPreview")?,
            &[4],
        )?,
    )?
    .err()
    .ok_or("foreign window cannot preview retained source")?;
    assert_eq!(observer["code"], "people_csv.source_unavailable");
    let denied = invoke_ipc_args(
        &fixture.observer,
        "csv_report",
        json!({"request":{
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview["previewId"]
        }}),
    )?
    .err()
    .ok_or("foreign window cannot read eligibility report")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    let command = CommandId::new(&SystemIdGenerator)?;
    let other = retained_source(fixture, bytes.to_vec())?;
    let denied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(fixture, 1, other, preview, command)?,
    )?
    .err()
    .ok_or("identical bytes cannot replace source custody")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    fixture.state.csv.close_source(
        "main",
        fixture.scenario,
        &SourceTarget::Source { source_id: other },
    );
    let denied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(fixture, 2, source, preview, command)?,
    )?
    .err()
    .ok_or("approval cannot move to a different revision")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    let mut forged = apply_request(fixture, 1, source, preview, command)?;
    forged["family"] = json!("assignments");
    let denied = heavy(&fixture.window, "csv_apply", forged)?
        .err()
        .ok_or("caller cannot choose retained-family dispatch")?;
    assert_eq!(denied["code"], "setup.invalid_request");
    Ok(())
}

async fn csv_native_undo_redo(
    fixture: &NativeCsvFixture,
    before: &eutheto_types::ScenarioDocument,
    imported: &eutheto_types::ScenarioDocument,
) -> TestResult {
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_undo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":2
        }),
    )?;
    assert_eq!(document(fixture).await?.domain, before.domain);
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_redo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":3
        }),
    )?;
    assert_eq!(document(fixture).await?.domain, imported.domain);
    Ok(())
}

fn assert_eligibility_native_reads(fixture: &NativeCsvFixture, source: SourceId) -> TestResult {
    let detection = heavy(&fixture.window, "csv_detect", json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvDetect")?,
        "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,"family":"eligibility"
    }))?.map_err(boxed)?;
    assert_eq!(detection.result["family"], "eligibility");
    assert_eq!(
        detection.result["data"]["dialects"][0]["samples"][0]["cells"][2]["text"],
        "eligible"
    );
    let sample = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvSample")?,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
            "readFormat":{"family":"eligibility","dialect":"comma"},"record":2
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(sample.result["cells"][0]["text"], endpoint(1));
    assert_eq!(sample.result["cells"][2]["text"], "true");
    Ok(())
}

#[tokio::test]
async fn eligibility_csv_native_detection_sample_grouped_apply_and_report() -> TestResult {
    let fixture = native_fixture().await?;
    let seed = seed_eligibility_endpoints(&fixture)?;
    let before = document(&fixture).await?;
    let bytes = format!(
        "person,type,eligible\n{},{},true\n{},{},true\nunknown,unknown,true\n",
        endpoint(1),
        endpoint(4),
        endpoint(1).to_ascii_uppercase(),
        endpoint(21)
    );
    let source = retained_source(&fixture, bytes.as_bytes().to_vec())?;
    assert_eligibility_native_reads(&fixture, source)?;
    let preview = heavy(
        &fixture.window,
        "eligibility_csv_preview",
        eligibility_native_request(
            &fixture,
            1,
            source,
            prepared(&fixture.window, fixture.scenario, 1, "csvPreview")?,
            &[4],
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(preview.current_revision, Some(Revision::new(1)));
    assert_eq!(preview.result["preview"]["disposition"], "reviewable");
    assert_eq!(
        preview.result["preview"]["batch"]["commands"]
            .as_array()
            .ok_or("batch")?
            .len(),
        1
    );
    assert_eq!(preview.result["preview"]["rows"][0]["before"], false);
    assert_eq!(preview.result["preview"]["rows"][0]["after"], true);
    assert_eq!(
        preview.result["preview"]["excludedRows"],
        json!([{"record":4,"skipped":true,"issue":"unresolvedPerson"}])
    );
    assert_eq!(document(&fixture).await?, before);
    eligibility_custody_refusals(&fixture, source, &preview.result, bytes.as_bytes())?;
    assert_history(&fixture, 1, seed).await?;
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 1, source, &preview.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":2})
    );
    assert_eq!(applied.current_revision, Some(Revision::new(2)));
    assert_eq!(applied.result["report"]["rows"]["family"], "eligibility");
    assert_eq!(
        applied.result["report"]["rows"]["excludedRows"],
        preview.result["preview"]["excludedRows"]
    );
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_err()
    );
    let imported = document(&fixture).await?;
    let mut expected = before.clone();
    expected
        .domain
        .entities
        .get_mut(&endpoint(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([endpoint(4), endpoint(21)]);
    assert_eq!(imported.domain, expected.domain);
    assert_history(&fixture, 2, command).await?;
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    csv_native_undo_redo(&fixture, &before, &imported).await?;
    let retained: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(retained.result, report.result);
    Ok(())
}

#[tokio::test]
async fn eligibility_csv_native_full_exclusions_and_pretyped_alias_admission() -> TestResult {
    let fixture = native_fixture().await?;
    let before = document(&fixture).await?;
    let source = retained_source(
        &fixture,
        format!(
            "person,type,eligible\n{}",
            "unknown,unknown,true\n".repeat(10_000)
        )
        .into_bytes(),
    )?;
    let operation = prepared(&fixture.window, fixture.scenario, 0, "csvPreview")?;
    let request = eligibility_native_request(
        &fixture,
        0,
        source,
        operation,
        &(2..=10_001).collect::<Vec<_>>(),
    )?;
    for endpoint_field in ["personMappings", "assignmentTypeMappings"] {
        let mut invalid = request.clone();
        // Invalid typed values prove cardinality wins before endpoint deserialization.
        invalid["mapping"][endpoint_field] = Value::Object(
            (0..10_001)
                .map(|index| (index.to_string(), Value::Null))
                .collect(),
        );
        assert!(serde_json::to_vec(&invalid["mapping"])?.len() < MAX_CSV_MAPPING_BYTES);
        let denied = heavy(&fixture.window, "eligibility_csv_preview", invalid)?
            .err()
            .ok_or("10001 aliases must fail pretyped admission")?;
        assert_eq!(denied["code"], "people_csv.request_too_large");
    }
    let preview = heavy(&fixture.window, "eligibility_csv_preview", request)?.map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "noChanges");
    let excluded = preview.result["preview"]["excludedRows"]
        .as_array()
        .ok_or("excluded rows")?;
    assert_eq!(excluded.len(), 10_000);
    assert_eq!(
        excluded[0],
        json!({"record":2,"skipped":true,"issue":"unresolvedPerson"})
    );
    assert_eq!(
        excluded[9_999],
        json!({"record":10_001,"skipped":true,"issue":"unresolvedPerson"})
    );
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            0,
            source,
            &preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"noChanges","revision":0})
    );
    assert_eq!(applied.result["report"]["rows"]["family"], "eligibility");
    assert_eq!(
        applied.result["report"]["rows"]["excludedRows"],
        preview.result["preview"]["excludedRows"]
    );
    assert!(serde_json::to_vec(&applied.result)?.len() > 128 * 1024);
    assert_eq!(document(&fixture).await?, before);
    match fixture
        .state
        .app
        .query(AppQuery::History(fixture.scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::History(history) => assert!(history.is_empty()),
        _ => return Err("unexpected history query".into()),
    }
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    assert_eq!(report.result["consumed"], true);
    Ok(())
}

#[tokio::test]
async fn eligibility_csv_native_source_survives_revision_but_approval_does_not() -> TestResult {
    let fixture = native_fixture().await?;
    seed_eligibility_endpoints(&fixture)?;
    let source = retained_source(
        &fixture,
        format!(
            "person,type,eligible\n{},{},true\n",
            endpoint(1),
            endpoint(4)
        )
        .into_bytes(),
    )?;
    let first = heavy(
        &fixture.window,
        "eligibility_csv_preview",
        eligibility_native_request(
            &fixture,
            1,
            source,
            prepared(&fixture.window, fixture.scenario, 1, "csvPreview")?,
            &[],
        )?,
    )?
    .map_err(boxed)?;
    let mut person = document(&fixture).await?.domain.entities[&endpoint(1).parse()?].clone();
    person["name"] = json!("Changed after review");
    let concurrent = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":concurrent,"scenarioId":fixture.scenario,"expectedRevision":1,
            "actor":{"actorId":null,"displayName":"Concurrent edit"},"truncateRedo":false,
            "label":"Change person name","commands":[{"type":"applyDomainCommand",
                "payload":{"commandType":"official.workforce.update_entity","payload":{"entity":person}}}]
        }),
    )?;
    let denied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            1,
            source,
            &first.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .err()
    .ok_or("actual revision must refuse stale approval")?;
    assert_eq!(denied["code"], "scenario.revision_conflict");
    assert_history(&fixture, 2, concurrent).await?;
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_ok()
    );
    let renewed = heavy(
        &fixture.window,
        "eligibility_csv_preview",
        eligibility_native_request(
            &fixture,
            2,
            source,
            prepared(&fixture.window, fixture.scenario, 2, "csvPreview")?,
            &[],
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(renewed.result["revision"], 2);
    assert_ne!(
        renewed.result["preview"]["approvalDigest"],
        first.result["preview"]["approvalDigest"]
    );
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &renewed.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":3})
    );
    person["eligibleAssignmentTypeIds"] = json!([endpoint(4)]);
    assert_eq!(
        document(&fixture).await?.domain.entities[&endpoint(1).parse()?],
        person
    );
    assert_history(&fixture, 3, command).await?;
    Ok(())
}

fn native_shift_mapping() -> Value {
    json!({
        "dialect":"tab","hasHeader":true,"expectedColumns":8,"identityColumn":0,
        "columns":[
            {"index":1,"field":"assignmentTypeId","blank":"preserve"},
            {"index":2,"field":"locationId","blank":"preserve"},
            {"index":3,"field":"startsAt","blank":"preserve"},
            {"index":4,"field":"endsAt","blank":"preserve"},
            {"index":5,"field":"coverage","blank":"preserve"},
            {"index":6,"field":"tags","blank":"preserve"},
            {"index":7,"field":"reportingAttribution","blank":"preserve"}
        ],
        "referenceMappings":{}
    })
}

fn native_shift_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    source: SourceId,
    decisions: Value,
) -> TestResult<Value> {
    let mut request = json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, revision, "csvPreview")?,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"sourceId":source,
        "mapping":native_shift_mapping()
    });
    request["decisions"] = decisions;
    Ok(request)
}

fn native_shift_source() -> String {
    format!(
        "id\ttype\tlocation\tstart\tend\tcoverage\ttags\tattribution\n{}\t{}\t{}\t2026-09-01T09:00:00\t2026-09-01T10:00:00\t{{\"kind\":\"exact\",\"count\":1,\"qualificationMinimums\":[]}}\t[\"changed\"]\tstartLocalDate\n{}\t{}\t{}\t2026-09-01T11:00:00\t2026-09-01T12:00:00\t{{\"kind\":\"atLeast\",\"minimum\":1,\"preferredCount\":2,\"maximumCount\":3,\"qualificationMinimums\":[]}}\t[]\tstartLocalDate\n",
        endpoint(8),
        endpoint(4),
        endpoint(5),
        endpoint(9),
        endpoint(4),
        endpoint(5),
    )
}

fn seed_native_shift(fixture: &NativeCsvFixture) -> TestResult<CommandId> {
    let entities = vec![
        json!({"kind":"assignmentType","id":endpoint(4),"name":"Clinic","category":"clinic",
            "timeBehavior":"elapsed","defaultDurationMinutes":60,
            "qualifications":{"kind":"unconstrained"},
            "locationBehavior":{"kind":"required"},"workloadBucketIds":[]}),
        json!({"kind":"location","id":endpoint(5),"name":"North","transitions":[]}),
        json!({"kind":"shiftInstance","id":endpoint(8),
            "assignmentTypeId":endpoint(4).to_ascii_uppercase(),
            "locationId":endpoint(5).replace('-', ""),
            "startsAt":{"instant":"2026-09-01T09:00:00-04:00",
                "local":"2026-09-01T09:00:00.000","offsetSeconds":-14400},
            "endsAt":{"instant":"2026-09-01T10:00:00-04:00",
                "local":"2026-09-01T10:00:00.000","offsetSeconds":-14400},
            "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]},
            "tags":["authored","order"],"reportingAttribution":"startLocalDate",
            "origin":{"kind":"manual"}}),
    ];
    let command = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":command,"scenarioId":fixture.scenario,"expectedRevision":0,
            "actor":{"actorId":null,"displayName":"Seed shift endpoints"},"truncateRedo":false,
            "label":"Shift endpoints","commands":entities.into_iter().map(|entity| json!({
                "type":"applyDomainCommand",
                "payload":{"commandType":"official.workforce.add_entity","payload":{"entity":entity}}
            })).collect::<Vec<_>>()
        }),
    )?;
    Ok(command)
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn shift_csv_native_detection_same_id_add_raw_preservation_and_undo() -> TestResult {
    let fixture = native_fixture_in_zone("America/New_York").await?;
    seed_native_shift(&fixture)?;
    let before = document(&fixture).await?;
    let bytes = native_shift_source().into_bytes();
    let source = retained_source(&fixture, bytes.clone())?;
    let detection = heavy(
        &fixture.window,
        "csv_detect",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvDetect")?,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,"family":"shifts"
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(detection.result["family"], "shifts");
    let sample = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvSample")?,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
            "readFormat":{"family":"shifts","dialect":"tab"},"record":2
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(sample.result["cells"][0]["text"], endpoint(8));
    let unresolved = heavy(
        &fixture.window,
        "shift_csv_preview",
        native_shift_request(&fixture, 1, source, json!([]))?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        unresolved.result["preview"]["rows"][1]["status"],
        "unresolved"
    );
    assert_eq!(
        unresolved.result["preview"]["rows"][1]["identity"],
        json!({"kind":"supplied","shiftId":endpoint(9)})
    );
    let decisions = json!([{"record":3,"decision":{"kind":"add","shiftId":endpoint(9)}}]);
    let preview_request = native_shift_request(&fixture, 1, source, decisions.clone())?;
    let preview = heavy(&fixture.window, "shift_csv_preview", preview_request)?.map_err(boxed)?;
    let rows = preview.result["preview"]["rows"]
        .as_array()
        .ok_or("shift rows")?;
    assert_eq!(rows[0]["status"], "updated");
    assert_eq!(rows[1]["status"], "added");
    assert_eq!(rows[1]["shiftId"], endpoint(9));
    assert_eq!(rows[1]["after"]["startsAt"]["offsetSeconds"], -14400);
    let mut expected = before.domain.clone();
    expected
        .entities
        .get_mut(&endpoint(8).parse()?)
        .ok_or("stored shift")?["tags"] = json!(["changed"]);
    expected
        .entities
        .insert(endpoint(9).parse()?, rows[1]["after"].clone());
    assert_eq!(rows[0]["after"], expected.entities[&endpoint(8).parse()?]);
    assert_eq!(document(&fixture).await?, before);
    let mut observer_request = native_shift_request(&fixture, 1, source, decisions)?;
    observer_request["operationId"] = json!(prepared(
        &fixture.observer,
        fixture.scenario,
        1,
        "csvPreview"
    )?);
    let denied = heavy(&fixture.observer, "shift_csv_preview", observer_request)?
        .err()
        .ok_or("wrong window must refuse source")?;
    assert_eq!(denied["code"], "people_csv.source_unavailable");
    let command = CommandId::new(&SystemIdGenerator)?;
    let mut wrong_family = apply_request(&fixture, 1, source, &preview.result, command)?;
    wrong_family["family"] = json!("people");
    assert_eq!(
        heavy(&fixture.window, "csv_apply", wrong_family)?
            .err()
            .ok_or("family cannot be caller-selected")?["code"],
        "setup.invalid_request"
    );
    let wrong_revision = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &preview.result, command)?,
    )?
    .err()
    .ok_or("wrong approval revision")?;
    assert_eq!(wrong_revision["code"], "people_csv.preview_unavailable");
    let identical_source = retained_source(&fixture, bytes)?;
    let wrong_source = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 1, identical_source, &preview.result, command)?,
    )?
    .err()
    .ok_or("identical bytes cannot replace source custody")?;
    assert_eq!(wrong_source["code"], "people_csv.preview_unavailable");
    fixture.state.csv.close_source(
        "main",
        fixture.scenario,
        &SourceTarget::Source {
            source_id: identical_source,
        },
    );
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 1, source, &preview.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":2})
    );
    assert_eq!(applied.result["report"]["rows"]["family"], "shifts");
    let imported = document(&fixture).await?;
    assert_eq!(imported.domain, expected);
    assert_history(&fixture, 2, command).await?;
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    let denied = invoke_ipc_args(
        &fixture.observer,
        "csv_report",
        json!({"request":{
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }}),
    )?
    .err()
    .ok_or("wrong window cannot read retained shift report")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    csv_native_undo_redo(&fixture, &before, &imported).await?;
    Ok(())
}

#[tokio::test]
async fn shift_csv_native_full_reports_and_pretyped_policy_limits() -> TestResult {
    let fixture = native_fixture().await?;
    let before = document(&fixture).await?;
    let source = retained_source(
        &fixture,
        format!(
            "id\ttype\tlocation\tstart\tend\tcoverage\ttags\tattribution\n{}",
            "bad\tunknown\t\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\n".repeat(10_000)
        )
        .into_bytes(),
    )?;
    let request = native_shift_request(
        &fixture,
        0,
        source,
        json!(
            (2..=10_001)
                .map(|record| json!({"record":record,"decision":{"kind":"skip"}}))
                .collect::<Vec<_>>()
        ),
    )?;
    let mut too_many_aliases = request.clone();
    too_many_aliases["mapping"]["referenceMappings"] = Value::Object(
        (0..10_001)
            .map(|index| (index.to_string(), Value::Null))
            .collect(),
    );
    assert!(serde_json::to_vec(&too_many_aliases["mapping"])?.len() < MAX_CSV_MAPPING_BYTES);
    let mut too_many_decisions = request.clone();
    too_many_decisions["decisions"] = json!(vec![Value::Null; MAX_CSV_DECISIONS + 1]);
    let mut oversized_mapping = request.clone();
    oversized_mapping["mapping"]["referenceMappings"] =
        json!({"name":"x".repeat(MAX_CSV_MAPPING_BYTES)});
    let mut oversized_decisions = request.clone();
    oversized_decisions["decisions"] = json!(["x".repeat(MAX_CSV_DECISION_BYTES)]);
    for invalid in [
        too_many_aliases,
        too_many_decisions,
        oversized_mapping,
        oversized_decisions,
    ] {
        assert_eq!(
            heavy(&fixture.window, "shift_csv_preview", invalid)?
                .err()
                .ok_or("policy must fail before typed allocation")?["code"],
            "people_csv.request_too_large"
        );
    }
    let preview = heavy(&fixture.window, "shift_csv_preview", request)?.map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "noChanges");
    let excluded = preview.result["preview"]["excludedRows"]
        .as_array()
        .ok_or("shift exclusions")?;
    assert_eq!(excluded.len(), 10_000);
    assert_eq!(excluded[0]["record"], 2);
    assert_eq!(excluded[9_999]["record"], 10_001);
    assert!(excluded.iter().all(|row| row["skipped"] == true));
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            0,
            source,
            &preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"noChanges","revision":0})
    );
    assert_eq!(applied.result["report"]["rows"]["family"], "shifts");
    assert_eq!(
        applied.result["report"]["rows"]["excludedRows"],
        preview.result["preview"]["excludedRows"]
    );
    assert!(serde_json::to_vec(&applied.result)?.len() > 128 * 1024);
    assert_eq!(document(&fixture).await?, before);
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    match fixture
        .state
        .app
        .query(AppQuery::History(fixture.scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::History(history) => assert!(history.is_empty()),
        _ => return Err("unexpected history query".into()),
    }
    Ok(())
}

#[tokio::test]
async fn shift_csv_native_source_survives_revision_and_requires_rereview() -> TestResult {
    let fixture = native_fixture_in_zone("America/New_York").await?;
    seed_native_shift(&fixture)?;
    let bytes = native_shift_source().into_bytes();
    let source = retained_source(&fixture, bytes)?;
    let decisions = json!([{"record":3,"decision":{"kind":"add","shiftId":endpoint(9)}}]);
    let first = heavy(
        &fixture.window,
        "shift_csv_preview",
        native_shift_request(&fixture, 1, source, decisions.clone())?,
    )?
    .map_err(boxed)?;
    let mut shift = document(&fixture).await?.domain.entities[&endpoint(8).parse()?].clone();
    shift["tags"] = json!(["concurrent"]);
    let concurrent = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":concurrent,"scenarioId":fixture.scenario,"expectedRevision":1,
            "actor":{"actorId":null,"displayName":"Concurrent edit"},"truncateRedo":false,
            "label":"Change shift tags","commands":[{"type":"applyDomainCommand",
                "payload":{"commandType":"official.workforce.update_entity","payload":{"entity":shift}}}]
        }),
    )?;
    let denied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            1,
            source,
            &first.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .err()
    .ok_or("stale shift approval must fail")?;
    assert_eq!(denied["code"], "scenario.revision_conflict");
    assert_history(&fixture, 2, concurrent).await?;
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_ok()
    );
    let renewed = heavy(
        &fixture.window,
        "shift_csv_preview",
        native_shift_request(&fixture, 2, source, decisions)?,
    )?
    .map_err(boxed)?;
    assert_eq!(renewed.result["revision"], 2);
    assert_ne!(
        renewed.result["preview"]["approvalDigest"],
        first.result["preview"]["approvalDigest"]
    );
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &renewed.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":3})
    );
    assert_history(&fixture, 3, command).await?;
    Ok(())
}

fn native_availability_mapping() -> Value {
    let fields = [
        "personId",
        "availabilityKind",
        "startsAt",
        "endsAt",
        "weeklyWindows",
        "effectiveStartDate",
        "effectiveEndDateExclusive",
        "assignmentTypeIds",
        "locationIds",
        "source",
        "note",
    ];
    json!({
        "dialect":"tab","hasHeader":true,"expectedColumns":12,"identityColumn":0,
        "columns":fields.into_iter().zip(1_u16..).map(|(field, index)|
            json!({"index":index,"field":field,"blank":"preserve"})).collect::<Vec<_>>(),
        "personMappings":{"same":endpoint(1)},
        "assignmentTypeMappings":{"same":endpoint(4)},
        "locationMappings":{"same":endpoint(5)}
    })
}

fn native_availability_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    source: SourceId,
    decisions: Value,
) -> TestResult<Value> {
    let mut request = json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, revision, "csvPreview")?,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"sourceId":source,
        "mapping":native_availability_mapping()
    });
    request["decisions"] = decisions;
    Ok(request)
}

fn native_availability_row(index: u32, note: &str) -> String {
    format!(
        "{}\tsame\tapprovedTimeOff\t2026-09-01T09:00:00.123\t2026-09-01T10:00:00.456\t\t2026-09-01\t2026-10-01\t[\"same\"]\t[\"same\"]\tauthored\t{note}\n",
        endpoint(index)
    )
}

const AVAILABILITY_HEADER: &str =
    "id\tperson\tkind\tstart\tend\tweekly\tfrom\tuntil\ttypes\tlocations\tsource\tnote\n";

fn native_availability_source() -> String {
    AVAILABILITY_HEADER.to_owned()
        + &native_availability_row(8, "changed")
        + &format!(
            "{}\tsame\trequestedTimeOff\t\t\t[{{\"weekdays\":[\"friday\",\"monday\"],\"startTime\":\"09:00:00\",\"endTime\":\"10:00:00\",\"endDayOffset\":0}}]\t2026-09-01\t2026-10-01\t[\"same\"]\t[\"same\"]\t weekly source \t weekly note \n",
            endpoint(9)
        )
        + &native_availability_row(10, "original")
}

fn seed_native_availability(fixture: &NativeCsvFixture) -> TestResult<CommandId> {
    let mut entities = vec![
        json!({"kind":"person","id":endpoint(1),"name":"River","activeRange":{"kind":"always"},
            "qualificationGrants":[],"eligibleAssignmentTypeIds":[],
            "workloadWeight":{"numerator":1,"denominator":1},"tags":[],"teamIds":[]}),
        json!({"kind":"assignmentType","id":endpoint(4),"name":"Clinic","category":"clinic",
            "timeBehavior":"elapsed","defaultDurationMinutes":60,
            "qualifications":{"kind":"unconstrained"},"locationBehavior":{"kind":"required"},
            "workloadBucketIds":[]}),
        json!({"kind":"location","id":endpoint(5),"name":"North","transitions":[]}),
    ];
    for index in [8, 10] {
        entities.push(json!({"kind":"availability","id":endpoint(index),
            "personId":endpoint(1).to_ascii_uppercase(),"availabilityKind":"approvedTimeOff",
            "timeWindow":{"kind":"instant","startsAt":"2026-09-01T09:00:00.123-04:00",
                "endsAt":"2026-09-01T10:00:00.456-04:00"},
            "effectiveRange":{"startDate":"2026-09-01","endDateExclusive":"2026-10-01"},
            "assignmentTypeIds":[endpoint(4).to_ascii_uppercase()],
            "locationIds":[endpoint(5).replace('-', "")],"source":"authored","note":"original"}));
    }
    let command = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":command,"scenarioId":fixture.scenario,"expectedRevision":0,
            "actor":{"actorId":null,"displayName":"Availability seed"},"truncateRedo":false,
            "label":"Availability seed","commands":entities.into_iter().map(|entity| json!({
                "type":"applyDomainCommand","payload":{
                    "commandType":"official.workforce.add_entity","payload":{"entity":entity}}
            })).collect::<Vec<_>>()
        }),
    )?;
    Ok(command)
}

fn availability_native_custody_refusals(
    fixture: &NativeCsvFixture,
    source: SourceId,
    preview: &Value,
    bytes: &[u8],
) -> TestResult {
    let mut observer = native_availability_request(fixture, 1, source, json!([]))?;
    observer["operationId"] = json!(prepared(
        &fixture.observer,
        fixture.scenario,
        1,
        "csvPreview"
    )?);
    assert_eq!(
        heavy(&fixture.observer, "availability_csv_preview", observer)?
            .err()
            .ok_or("window custody")?["code"],
        "people_csv.source_unavailable"
    );
    let denied = invoke_ipc_args(
        &fixture.observer,
        "csv_report",
        json!({"request":{
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview["previewId"]
        }}),
    )?
    .err()
    .ok_or("report window custody")?;
    assert_eq!(denied["code"], "people_csv.preview_unavailable");
    let command = CommandId::new(&SystemIdGenerator)?;
    let mut wrong_family = apply_request(fixture, 1, source, preview, command)?;
    wrong_family["family"] = json!("shifts");
    assert_eq!(
        heavy(&fixture.window, "csv_apply", wrong_family)?
            .err()
            .ok_or("family authority")?["code"],
        "setup.invalid_request"
    );
    assert_eq!(
        heavy(
            &fixture.window,
            "csv_apply",
            apply_request(fixture, 2, source, preview, command)?
        )?
        .err()
        .ok_or("revision custody")?["code"],
        "people_csv.preview_unavailable"
    );
    let identical = retained_source(fixture, bytes.to_vec())?;
    assert_eq!(
        heavy(
            &fixture.window,
            "csv_apply",
            apply_request(fixture, 1, identical, preview, command)?
        )?
        .err()
        .ok_or("source custody")?["code"],
        "people_csv.preview_unavailable"
    );
    fixture.state.csv.close_source(
        "main",
        fixture.scenario,
        &SourceTarget::Source {
            source_id: identical,
        },
    );
    let mut tampered = apply_request(fixture, 1, source, preview, command)?;
    tampered["approvedDigest"] = json!("0".repeat(64));
    assert_eq!(
        heavy(&fixture.window, "csv_apply", tampered)?
            .err()
            .ok_or("approval binding")?["code"],
        "people_csv.approval_mismatch"
    );
    Ok(())
}

fn availability_native_reads(fixture: &NativeCsvFixture, source: SourceId) -> TestResult {
    let detection = heavy(&fixture.window, "csv_detect", json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvDetect")?,
        "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,"family":"availability"
    }))?.map_err(boxed)?;
    assert_eq!(detection.result["family"], "availability");
    let sample = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 1, "csvSample")?,
            "scenarioId":fixture.scenario,"expectedRevision":1,"sourceId":source,
            "readFormat":{"family":"availability","dialect":"tab"},"record":2
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(sample.result["cells"][0]["text"], endpoint(8));
    Ok(())
}

fn availability_native_expected(
    before: &eutheto_types::ScenarioDocument,
    preview: &Value,
) -> TestResult<eutheto_types::ScenarioDocument> {
    let rows = preview["preview"]["rows"]
        .as_array()
        .ok_or("availability rows")?;
    assert_eq!(rows[0]["status"], "updated");
    assert_eq!(rows[1]["status"], "added");
    assert_eq!(rows[2]["status"], "unchanged");
    assert_eq!(rows[0]["resolvedStartsAt"]["offsetSeconds"], -14400);
    assert_eq!(rows[2]["resolvedEndsAt"]["offsetSeconds"], -14400);
    assert_eq!(
        rows[0]["after"]["timeWindow"]["startsAt"],
        "2026-09-01T09:00:00.123-04:00"
    );
    assert_eq!(rows[1]["after"]["timeWindow"]["kind"], "weekly");
    assert_eq!(
        rows[1]["after"]["timeWindow"]["windows"][0]["weekdays"],
        json!(["friday", "monday"])
    );
    assert_eq!(rows[1]["after"]["source"], " weekly source ");
    assert_eq!(rows[1]["after"]["note"], " weekly note ");
    let mut expected = before.clone();
    expected
        .domain
        .entities
        .get_mut(&endpoint(8).parse()?)
        .ok_or("availability")?["note"] = json!("changed");
    assert_eq!(
        rows[0]["after"],
        expected.domain.entities[&endpoint(8).parse()?]
    );
    assert_eq!(
        rows[2]["before"],
        before.domain.entities[&endpoint(10).parse()?]
    );
    expected
        .domain
        .entities
        .insert(endpoint(9).parse()?, rows[1]["after"].clone());
    Ok(expected)
}

fn availability_native_no_op(fixture: &NativeCsvFixture) -> TestResult {
    let source = retained_source(
        fixture,
        (AVAILABILITY_HEADER.to_owned() + &native_availability_row(10, "original")).into_bytes(),
    )?;
    let preview = heavy(
        &fixture.window,
        "availability_csv_preview",
        native_availability_request(fixture, 2, source, json!([]))?,
    )?
    .map_err(boxed)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            fixture,
            2,
            source,
            &preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"noChanges","revision":2})
    );
    Ok(())
}

#[tokio::test]
async fn availability_csv_native_envelopes_preserve_raw_weekly_and_signed_resolution() -> TestResult
{
    let fixture = native_fixture_in_zone("America/New_York").await?;
    seed_native_availability(&fixture)?;
    let before = document(&fixture).await?;
    let bytes = native_availability_source().into_bytes();
    let source = retained_source(&fixture, bytes.clone())?;
    availability_native_reads(&fixture, source)?;
    let unresolved = heavy(
        &fixture.window,
        "availability_csv_preview",
        native_availability_request(&fixture, 1, source, json!([]))?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        unresolved.result["preview"]["rows"][1]["status"],
        "unresolved"
    );
    assert_eq!(
        unresolved.result["preview"]["rows"][1]["identity"],
        json!({"kind":"supplied","availabilityId":endpoint(9)})
    );
    let preview = heavy(
        &fixture.window,
        "availability_csv_preview",
        native_availability_request(
            &fixture,
            1,
            source,
            json!([{"record":3,"decision":{"kind":"add","availabilityId":endpoint(9)}}]),
        )?,
    )?
    .map_err(boxed)?;
    let expected = availability_native_expected(&before, &preview.result)?;
    assert_eq!(document(&fixture).await?, before);
    availability_native_custody_refusals(&fixture, source, &preview.result, &bytes)?;
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 1, source, &preview.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":2})
    );
    assert_eq!(applied.result["report"]["rows"]["family"], "availability");
    let imported = document(&fixture).await?;
    assert_eq!(imported.domain, expected.domain);
    assert_history(&fixture, 2, command).await?;
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    availability_native_no_op(&fixture)?;
    assert_history(&fixture, 2, command).await?;
    csv_native_undo_redo(&fixture, &before, &imported).await?;
    Ok(())
}

#[tokio::test]
async fn availability_csv_native_complete_reports_and_pretyped_role_alias_admission() -> TestResult
{
    let fixture = native_fixture().await?;
    let before = document(&fixture).await?;
    let source = retained_source(&fixture, (AVAILABILITY_HEADER.to_owned()
        + &"bad\tunknown\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\t\t\n".repeat(10_000)).into_bytes())?;
    let request = native_availability_request(
        &fixture,
        0,
        source,
        json!(
            (2..=10_001)
                .map(|record| json!({"record":record,"decision":{"kind":"skip"}}))
                .collect::<Vec<_>>()
        ),
    )?;
    for field in [
        "personMappings",
        "assignmentTypeMappings",
        "locationMappings",
    ] {
        let mut invalid = request.clone();
        invalid["mapping"][field] = Value::Object(
            (0..10_001)
                .map(|index| (index.to_string(), Value::Null))
                .collect(),
        );
        assert!(serde_json::to_vec(&invalid["mapping"])?.len() < MAX_CSV_MAPPING_BYTES);
        assert_eq!(
            heavy(&fixture.window, "availability_csv_preview", invalid)?
                .err()
                .ok_or("pretyped role aliases")?["code"],
            "people_csv.request_too_large"
        );
    }
    let mut aggregate = request.clone();
    for field in [
        "personMappings",
        "assignmentTypeMappings",
        "locationMappings",
    ] {
        aggregate["mapping"][field] = json!({"alias":"x".repeat(MAX_CSV_MAPPING_BYTES / 2)});
    }
    assert_eq!(
        heavy(&fixture.window, "availability_csv_preview", aggregate)?
            .err()
            .ok_or("aggregate mapping admission")?["code"],
        "people_csv.request_too_large"
    );
    let mut decisions = request.clone();
    decisions["decisions"] = json!(vec![Value::Null; MAX_CSV_DECISIONS + 1]);
    assert_eq!(
        heavy(&fixture.window, "availability_csv_preview", decisions)?
            .err()
            .ok_or("decision admission")?["code"],
        "people_csv.request_too_large"
    );
    let preview = heavy(&fixture.window, "availability_csv_preview", request)?.map_err(boxed)?;
    let excluded = preview.result["preview"]["excludedRows"]
        .as_array()
        .ok_or("exclusions")?;
    assert_eq!(excluded.len(), 10_000);
    assert_eq!(excluded[0]["record"], 2);
    assert_eq!(excluded[9_999]["record"], 10_001);
    assert!(excluded.iter().all(|row| row["skipped"] == true));
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            0,
            source,
            &preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"noChanges","revision":0})
    );
    assert_eq!(applied.result["report"]["rows"]["family"], "availability");
    assert_eq!(
        applied.result["report"]["rows"]["excludedRows"],
        preview.result["preview"]["excludedRows"]
    );
    assert!(serde_json::to_vec(&applied.result)?.len() > 128 * 1024);
    assert_eq!(document(&fixture).await?, before);
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,"previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    Ok(())
}

#[tokio::test]
async fn availability_csv_native_source_survives_revision_and_requires_rereview() -> TestResult {
    let fixture = native_fixture_in_zone("America/New_York").await?;
    seed_native_availability(&fixture)?;
    let source = retained_source(
        &fixture,
        (AVAILABILITY_HEADER.to_owned() + &native_availability_row(8, "changed")).into_bytes(),
    )?;
    let first = heavy(
        &fixture.window,
        "availability_csv_preview",
        native_availability_request(&fixture, 1, source, json!([]))?,
    )?
    .map_err(boxed)?;
    let mut entity = document(&fixture).await?.domain.entities[&endpoint(8).parse()?].clone();
    entity["note"] = json!("concurrent");
    let concurrent = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":concurrent,"scenarioId":fixture.scenario,"expectedRevision":1,
            "actor":{"actorId":null,"displayName":"Concurrent edit"},"truncateRedo":false,
            "label":"Change availability note","commands":[{"type":"applyDomainCommand","payload":{
                "commandType":"official.workforce.update_entity","payload":{"entity":entity}}}]
        }),
    )?;
    assert_eq!(
        heavy(
            &fixture.window,
            "csv_apply",
            apply_request(
                &fixture,
                1,
                source,
                &first.result,
                CommandId::new(&SystemIdGenerator)?
            )?
        )?
        .err()
        .ok_or("stale approval")?["code"],
        "scenario.revision_conflict"
    );
    assert_history(&fixture, 2, concurrent).await?;
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_ok()
    );
    let renewed = heavy(
        &fixture.window,
        "availability_csv_preview",
        native_availability_request(&fixture, 2, source, json!([]))?,
    )?
    .map_err(boxed)?;
    assert_ne!(
        renewed.result["preview"]["approvalDigest"],
        first.result["preview"]["approvalDigest"]
    );
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &renewed.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":3})
    );
    assert_history(&fixture, 3, command).await?;
    Ok(())
}

fn matrix_source_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    operation: OperationId,
    request_id: RequestId,
    text: &str,
) -> Value {
    json!({"schemaVersion":1,"requestId":request_id,"operationId":operation,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"text":text})
}

fn matrix_source(fixture: &NativeCsvFixture, revision: u32, text: &str) -> TestResult<SourceId> {
    let opened = heavy(
        &fixture.window,
        "eligibility_matrix_source_open",
        matrix_source_request(
            fixture,
            revision,
            prepared(&fixture.window, fixture.scenario, revision, "csvSourceOpen")?,
            next()?,
            text,
        ),
    )?
    .map_err(boxed)?;
    assert_eq!(opened.result["byteCount"], text.len());
    Ok(SourceId(
        opened.result["sourceId"]
            .as_str()
            .ok_or("matrix source")?
            .parse()?,
    ))
}

fn matrix_native_mapping() -> Value {
    json!({"personMappings":{"shared":endpoint(1)},"assignmentTypeMappings":{"shared":endpoint(21)}})
}

fn matrix_native_request(
    fixture: &NativeCsvFixture,
    revision: u32,
    source: SourceId,
    mapping: Value,
    skipped_cells: Value,
) -> TestResult<Value> {
    let mut request = json!({"schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, revision, "csvPreview")?,
        "scenarioId":fixture.scenario,"expectedRevision":revision,"sourceId":source});
    request["mapping"] = mapping;
    request["skippedCells"] = skipped_cells;
    Ok(request)
}

fn matrix_native_text() -> String {
    format!(
        "personId\tshared\t{}\t{}\nshared\ttrue\t\ttrue\n{}\ttrue\t\t\n{}\tfalse\tfalse\tTRUE\n<img src=x>\t\t\t\n",
        endpoint(4),
        endpoint(23),
        endpoint(1).to_ascii_uppercase(),
        endpoint(13)
    )
}

fn matrix_native_skips() -> Value {
    json!([{"record":3,"column":1},{"record":3,"column":2},{"record":3,"column":3},
        {"record":4,"column":3},{"record":5,"column":1},{"record":5,"column":2},{"record":5,"column":3}])
}

async fn seed_matrix_endpoints(fixture: &NativeCsvFixture) -> TestResult<CommandId> {
    seed_eligibility_endpoints(fixture)?;
    let seeded = document(fixture).await?;
    let mut person = seeded.domain.entities[&endpoint(1).parse()?].clone();
    person["eligibleAssignmentTypeIds"] = json!([endpoint(4).to_ascii_uppercase()]);
    let mut second = person.clone();
    second["id"] = json!(endpoint(13));
    second["name"] = json!("Alex");
    second["externalId"] = json!("staff-13");
    let mut assignment_type = seeded.domain.entities[&endpoint(21).parse()?].clone();
    assignment_type["id"] = json!(endpoint(23));
    assignment_type["name"] = json!("Clinic 23");
    let command = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":command,"scenarioId":fixture.scenario,"expectedRevision":1,
            "actor":{"actorId":null,"displayName":"Seed matrix endpoints"},"truncateRedo":false,
            "label":"Seed matrix raw memberships","commands":[
                {"type":"applyDomainCommand","payload":{"commandType":"official.workforce.update_entity",
                    "payload":{"entity":person}}},
                {"type":"applyDomainCommand","payload":{"commandType":"official.workforce.add_entity",
                    "payload":{"entity":second}}},
                {"type":"applyDomainCommand","payload":{"commandType":"official.workforce.add_entity",
                    "payload":{"entity":assignment_type}}}
            ]
        }),
    )?;
    Ok(command)
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn eligibility_matrix_native_pasted_source_full_flow() -> TestResult {
    let fixture = native_fixture().await?;
    let seed = seed_matrix_endpoints(&fixture).await?;
    let before = document(&fixture).await?;
    let text = matrix_native_text();
    let source_opened = heavy(
        &fixture.window,
        "eligibility_matrix_source_open",
        matrix_source_request(
            &fixture,
            2,
            prepared(&fixture.window, fixture.scenario, 2, "csvSourceOpen")?,
            next()?,
            &text,
        ),
    )?
    .map_err(boxed)?;
    assert_eq!(source_opened.result["byteCount"], text.len());
    let source = SourceId(
        source_opened.result["sourceId"]
            .as_str()
            .ok_or("matrix source")?
            .parse()?,
    );
    let detection = heavy(&fixture.window, "csv_detect", json!({
        "schemaVersion":1,"requestId":next()?,
        "operationId":prepared(&fixture.window, fixture.scenario, 2, "csvDetect")?,
        "scenarioId":fixture.scenario,"expectedRevision":2,"sourceId":source,"family":"eligibilityMatrix"
    }))?.map_err(boxed)?;
    assert_eq!(detection.result["family"], "eligibilityMatrix");
    assert_eq!(detection.result["data"]["columns"], 4);
    assert_eq!(detection.result["data"]["cells"], 12);
    assert_eq!(detection.result["data"]["source"]["logicalRecords"], 5);
    assert_eq!(
        detection.result["data"]["samples"][0]["cells"][1]["text"],
        "shared"
    );
    let sample = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 2, "csvSample")?,
            "scenarioId":fixture.scenario,"expectedRevision":2,"sourceId":source,
            "readFormat":{"family":"eligibilityMatrix"},"record":4
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(sample.result["record"], 4);
    assert_eq!(sample.result["cells"][0]["text"], endpoint(13));
    assert_eq!(sample.result["cells"][3]["text"], "TRUE");
    let inert_sample = heavy(
        &fixture.window,
        "csv_record_sample",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 2, "csvSample")?,
            "scenarioId":fixture.scenario,"expectedRevision":2,"sourceId":source,
            "readFormat":{"family":"eligibilityMatrix"},"record":5
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(inert_sample.result["cells"][0]["text"], "<img src=x>");
    let unresolved_preview = heavy(
        &fixture.window,
        "eligibility_matrix_preview",
        matrix_native_request(
            &fixture,
            2,
            source,
            json!({"personMappings":{},"assignmentTypeMappings":{}}),
            json!([]),
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        unresolved_preview.result["preview"]["disposition"],
        "blocked"
    );
    assert_eq!(
        unresolved_preview.result["preview"]["rows"][0]["issue"],
        "unresolvedPerson"
    );
    let preview = heavy(
        &fixture.window,
        "eligibility_matrix_preview",
        matrix_native_request(
            &fixture,
            2,
            source,
            matrix_native_mapping(),
            matrix_native_skips(),
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "reviewable");
    assert_eq!(
        preview.result["preview"]["rows"][0]["personId"],
        endpoint(1)
    );
    assert_eq!(
        preview.result["preview"]["rows"][0]["assignmentTypeId"],
        endpoint(21)
    );
    assert_eq!(
        preview.result["preview"]["rows"][0]["cell"],
        json!({"record":2,"column":1})
    );
    assert_eq!(preview.result["preview"]["rows"][1]["before"], true);
    assert_eq!(preview.result["preview"]["rows"][1]["after"], true);
    assert_eq!(
        preview.result["preview"]["batch"]["commands"]
            .as_array()
            .ok_or("batch")?
            .len(),
        2
    );
    assert_eq!(
        preview.result["preview"]["excludedCells"],
        json!([
            {"cell":{"record":3,"column":1},"skipped":true,"issue":"duplicatePair"},
            {"cell":{"record":3,"column":2},"skipped":true,"issue":"duplicatePair"},
            {"cell":{"record":3,"column":3},"skipped":true,"issue":"duplicatePair"},
            {"cell":{"record":4,"column":3},"skipped":true,"issue":"invalidMembership"},
            {"cell":{"record":5,"column":1},"skipped":true,"issue":"unresolvedPerson"},
            {"cell":{"record":5,"column":2},"skipped":true,"issue":"unresolvedPerson"},
            {"cell":{"record":5,"column":3},"skipped":true,"issue":"unresolvedPerson"}
        ])
    );
    assert_eq!(document(&fixture).await?, before);
    assert_history(&fixture, 2, seed).await?;
    let other = matrix_source(&fixture, 2, &text)?;
    let wrong_source = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            2,
            other,
            &preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .err()
    .ok_or("identical text must not replace source custody")?;
    assert_eq!(wrong_source["code"], "people_csv.preview_unavailable");
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_source_close",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "target":{"kind":"source","sourceId":other}
        }),
    )?;
    let mut wrong_approval = apply_request(
        &fixture,
        2,
        source,
        &preview.result,
        CommandId::new(&SystemIdGenerator)?,
    )?;
    wrong_approval["approvedDigest"] = json!("0".repeat(64));
    assert_eq!(
        heavy(&fixture.window, "csv_apply", wrong_approval)?
            .err()
            .ok_or("wrong approval")?["code"],
        "people_csv.approval_mismatch"
    );
    // A separate custody owner keeps this assertion independent of the three-preview limit.
    let csv_fixture = native_fixture().await?;
    seed_eligibility_endpoints(&csv_fixture)?;
    let csv_source = matrix_source(
        &csv_fixture,
        1,
        &format!(
            "person,type,eligible\n{},{},true\n",
            endpoint(1),
            endpoint(21)
        ),
    )?;
    let csv_preview = heavy(
        &csv_fixture.window,
        "eligibility_csv_preview",
        eligibility_native_request(
            &csv_fixture,
            1,
            csv_source,
            prepared(&csv_fixture.window, csv_fixture.scenario, 1, "csvPreview")?,
            &[],
        )?,
    )?
    .map_err(boxed)?;
    let mut cross_family = apply_request(
        &fixture,
        2,
        source,
        &preview.result,
        CommandId::new(&SystemIdGenerator)?,
    )?;
    cross_family["approvedDigest"] = csv_preview.result["preview"]["approvalDigest"].clone();
    assert_eq!(
        heavy(&fixture.window, "csv_apply", cross_family)?
            .err()
            .ok_or("CSV approval cannot approve matrix")?["code"],
        "people_csv.approval_mismatch"
    );
    let mut forged = apply_request(
        &fixture,
        2,
        source,
        &preview.result,
        CommandId::new(&SystemIdGenerator)?,
    )?;
    forged["family"] = json!("eligibility");
    assert_eq!(
        heavy(&fixture.window, "csv_apply", forged)?
            .err()
            .ok_or("caller-selected family")?["code"],
        "setup.invalid_request"
    );
    let command = CommandId::new(&SystemIdGenerator)?;
    let apply = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &preview.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        apply.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":3})
    );
    assert_eq!(
        apply.result["report"]["rows"]["family"],
        "eligibilityMatrix"
    );
    assert_eq!(
        apply.result["report"]["rows"]["excludedCells"],
        preview.result["preview"]["excludedCells"]
    );
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_err()
    );
    let imported = document(&fixture).await?;
    let mut expected = before.clone();
    expected
        .domain
        .entities
        .get_mut(&endpoint(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] =
        json!([endpoint(4).to_ascii_uppercase(), endpoint(21), endpoint(23)]);
    expected
        .domain
        .entities
        .get_mut(&endpoint(13).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([]);
    assert_eq!(imported.domain, expected.domain);
    assert_history(&fixture, 3, command).await?;
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, apply.result["report"]);
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_undo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":3
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, before.domain);
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_redo",
        &json!({
            "requestId":next()?,"scenarioId":fixture.scenario,"expectedRevision":4
        }),
    )?;
    assert_eq!(document(&fixture).await?.domain, imported.domain);
    let no_changes_source_opened = heavy(
        &fixture.window,
        "eligibility_matrix_source_open",
        matrix_source_request(
            &fixture,
            5,
            prepared(&fixture.window, fixture.scenario, 5, "csvSourceOpen")?,
            next()?,
            &text,
        ),
    )?
    .map_err(boxed)?;
    let no_changes_source = SourceId(
        no_changes_source_opened.result["sourceId"]
            .as_str()
            .ok_or("no changes source")?
            .parse()?,
    );
    let no_changes_detection = heavy(
        &fixture.window,
        "csv_detect",
        json!({
            "schemaVersion":1,"requestId":next()?,
            "operationId":prepared(&fixture.window, fixture.scenario, 5, "csvDetect")?,
            "scenarioId":fixture.scenario,"expectedRevision":5,"sourceId":no_changes_source,
            "family":"eligibilityMatrix"
        }),
    )?
    .map_err(boxed)?;
    assert_eq!(
        no_changes_detection.result["data"]["source"],
        detection.result["data"]["source"]
    );
    let no_changes_preview = heavy(
        &fixture.window,
        "eligibility_matrix_preview",
        matrix_native_request(
            &fixture,
            5,
            no_changes_source,
            matrix_native_mapping(),
            matrix_native_skips(),
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        no_changes_preview.result["preview"]["disposition"],
        "noChanges"
    );
    let no_changes_apply = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            5,
            no_changes_source,
            &no_changes_preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        no_changes_apply.result["outcome"],
        json!({"kind":"noChanges","revision":5})
    );
    assert_eq!(
        no_changes_apply.result["report"]["rows"]["family"],
        "eligibilityMatrix"
    );
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, no_changes_source)
            .is_err()
    );
    assert_history(&fixture, 3, command).await?;
    assert_eq!(document(&fixture).await?.domain, imported.domain);
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn eligibility_matrix_native_pretyped_bounds_and_owner_creator_closure() -> TestResult {
    let fixture = native_fixture().await?;
    seed_matrix_endpoints(&fixture).await?;
    let text = matrix_native_text();
    let operation = prepared(&fixture.window, fixture.scenario, 2, "csvSourceOpen")?;
    let creator_request = next()?;
    let request = matrix_source_request(&fixture, 2, operation, creator_request, &text);
    for (field, value) in [
        (
            "text",
            json!("x".repeat(MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + 1)),
        ),
        (
            "padding",
            json!("x".repeat(6 * MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + 64 * 1024 + 1)),
        ),
    ] {
        let mut oversized = request.clone();
        oversized[field] = value;
        oversized["requestId"] = Value::Null;
        oversized["scenarioId"] = Value::Null;
        assert_eq!(
            heavy(&fixture.window, "eligibility_matrix_source_open", oversized)?
                .err()
                .ok_or("admission must precede typed IDs and operation claim")?["code"],
            "people_csv.request_too_large"
        );
    }
    // Both refusals left the same operation unclaimed.
    let opened =
        heavy(&fixture.window, "eligibility_matrix_source_open", request)?.map_err(boxed)?;
    let source = SourceId(
        opened.result["sourceId"]
            .as_str()
            .ok_or("source")?
            .parse()?,
    );
    let foreign_operation = prepared(&fixture.observer, fixture.scenario, 2, "csvSourceOpen")?;
    assert_eq!(
        heavy(
            &fixture.window,
            "eligibility_matrix_source_open",
            matrix_source_request(&fixture, 2, foreign_operation, next()?, &text)
        )?
        .err()
        .ok_or("operation window binding")?["code"],
        "operation.not_active"
    );
    let mut observer_preview = matrix_native_request(
        &fixture,
        2,
        source,
        matrix_native_mapping(),
        matrix_native_skips(),
    )?;
    observer_preview["operationId"] = json!(prepared(
        &fixture.observer,
        fixture.scenario,
        2,
        "csvPreview"
    )?);
    assert_eq!(
        heavy(
            &fixture.observer,
            "eligibility_matrix_preview",
            observer_preview
        )?
        .err()
        .ok_or("actual window source binding")?["code"],
        "people_csv.source_unavailable"
    );
    // Create a real other scenario, not a guessed identifier.
    let created: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "project_create",
        &json!({
            "schemaVersion":1,"requestId":next()?,"title":"Other matrix scenario","description":"",
            "domainPack":{"id":"official.workforce","schemaVersion":1},
            "settings":{"timeZone":"UTC","locale":"en-US","units":"metric",
                "firstDate":"2026-09-01","lastDate":"2026-09-01","gapPolicy":"reject","overlapPolicy":"earlier"}
        }),
    )?;
    let other: ScenarioId = created.result["scenarioId"]
        .as_str()
        .ok_or("other scenario")?
        .parse()?;
    let mut wrong_scenario = matrix_native_request(
        &fixture,
        2,
        source,
        matrix_native_mapping(),
        matrix_native_skips(),
    )?;
    wrong_scenario["scenarioId"] = json!(other);
    wrong_scenario["expectedRevision"] = json!(0);
    wrong_scenario["operationId"] = json!(prepared(&fixture.window, other, 0, "csvPreview")?);
    assert_eq!(
        heavy(
            &fixture.window,
            "eligibility_matrix_preview",
            wrong_scenario
        )?
        .err()
        .ok_or("actual scenario source binding")?["code"],
        "people_csv.source_unavailable"
    );
    for (window, scenario, request_id) in [
        (&fixture.observer, fixture.scenario, creator_request),
        (&fixture.window, other, creator_request),
        (&fixture.window, fixture.scenario, next()?),
    ] {
        let _: ApiResponseDto<Value> = invoke_ok(
            window,
            "csv_source_close",
            &json!({
                "schemaVersion":1,"requestId":next()?,"scenarioId":scenario,
                "target":{"kind":"creator","operationId":operation,"requestId":request_id}
            }),
        )?;
        assert!(
            fixture
                .state
                .csv
                .source("main", fixture.scenario, source)
                .is_ok()
        );
    }
    let preview_operation = prepared(&fixture.window, fixture.scenario, 2, "csvPreview")?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "operation_cancel",
        &json!({
            "schemaVersion":1,"requestId":next()?,"operationId":preview_operation
        }),
    )?;
    let mut cancelled_preview = matrix_native_request(
        &fixture,
        2,
        source,
        matrix_native_mapping(),
        matrix_native_skips(),
    )?;
    cancelled_preview["operationId"] = json!(preview_operation);
    assert_eq!(
        heavy(
            &fixture.window,
            "eligibility_matrix_preview",
            cancelled_preview
        )?
        .err()
        .ok_or("cancelled preview")?["code"],
        "operation.cancelled"
    );
    let duplicate_preview = heavy(
        &fixture.window,
        "eligibility_matrix_preview",
        matrix_native_request(&fixture, 2, source, matrix_native_mapping(), json!([]))?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        duplicate_preview.result["preview"]["disposition"],
        "blocked"
    );
    assert_eq!(
        duplicate_preview.result["preview"]["rows"][0]["issue"],
        "duplicatePair"
    );
    assert_eq!(
        duplicate_preview.result["preview"]["rows"][1]["issue"],
        "duplicatePair"
    );
    assert_eq!(
        duplicate_preview.result["preview"]["rows"][8]["issue"],
        "invalidMembership"
    );
    // Closing refuses new readers; an already admitted lease retains immutable original bytes.
    let lease = fixture
        .state
        .csv
        .source("main", fixture.scenario, source)
        .map_err(boxed)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_source_close",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,
            "target":{"kind":"creator","operationId":operation,"requestId":creator_request}
        }),
    )?;
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_err()
    );
    let mut original = String::new();
    std::io::Read::read_to_string(&mut lease.reader(), &mut original)?;
    assert_eq!(original, text);
    assert_eq!(
        heavy(
            &fixture.window,
            "eligibility_matrix_preview",
            matrix_native_request(
                &fixture,
                2,
                source,
                matrix_native_mapping(),
                matrix_native_skips()
            )?
        )?
        .err()
        .ok_or("closed source")?["code"],
        "people_csv.source_unavailable"
    );
    let cancelled = prepared(&fixture.window, fixture.scenario, 2, "csvSourceOpen")?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "operation_cancel",
        &json!({
            "schemaVersion":1,"requestId":next()?,"operationId":cancelled
        }),
    )?;
    assert_eq!(
        heavy(
            &fixture.window,
            "eligibility_matrix_source_open",
            matrix_source_request(&fixture, 2, cancelled, next()?, &text)
        )?
        .err()
        .ok_or("cancelled source")?["code"],
        "operation.cancelled"
    );
    Ok(())
}

#[tokio::test]
async fn eligibility_matrix_native_window_teardown_refuses_late_source_and_preview_responses()
-> TestResult {
    for preview in [false, true] {
        let fixture = native_fixture().await?;
        let request = if preview {
            seed_eligibility_endpoints(&fixture)?;
            let text = format!("personId\t{}\n{}\ttrue\n", endpoint(21), endpoint(1));
            let source = matrix_source(&fixture, 1, &text)?;
            matrix_native_request(
                &fixture,
                1,
                source,
                json!({"personMappings":{},"assignmentTypeMappings":{}}),
                json!([]),
            )?
        } else {
            matrix_source_request(
                &fixture,
                0,
                prepared(&fixture.window, fixture.scenario, 0, "csvSourceOpen")?,
                next()?,
                "personId\tunknown\nunknown\ttrue\n",
            )
        };
        fixture
            .close_on_response
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let command = if preview {
            "eligibility_matrix_preview"
        } else {
            "eligibility_matrix_source_open"
        };
        let refused = heavy(&fixture.window, command, request)?
            .err()
            .ok_or("late publication must not escape destroyed owner")?;
        assert_eq!(refused["code"], "operation.cancelled");
        assert!(
            !fixture
                .close_on_response
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        // Late cancellation did not strand a shared native source slot.
        let revision = u32::from(preview);
        for _ in 0..3 {
            let opened = heavy(
                &fixture.observer,
                "eligibility_matrix_source_open",
                matrix_source_request(
                    &fixture,
                    revision,
                    prepared(
                        &fixture.observer,
                        fixture.scenario,
                        revision,
                        "csvSourceOpen",
                    )?,
                    next()?,
                    "personId\tunknown\n",
                ),
            )?
            .map_err(boxed)?;
            assert!(opened.result["sourceId"].is_string());
        }
        assert!(prepared(&fixture.window, fixture.scenario, revision, "csvSourceOpen").is_err());
    }
    Ok(())
}

#[tokio::test]
async fn eligibility_matrix_native_stale_rereview_preserves_original_paste() -> TestResult {
    let fixture = native_fixture().await?;
    seed_eligibility_endpoints(&fixture)?;
    let text = format!("personId\t{}\n{}\ttrue\n", endpoint(21), endpoint(1));
    let source = matrix_source(&fixture, 1, &text)?;
    let first = heavy(
        &fixture.window,
        "eligibility_matrix_preview",
        matrix_native_request(
            &fixture,
            1,
            source,
            json!({"personMappings":{},"assignmentTypeMappings":{}}),
            json!([]),
        )?,
    )?
    .map_err(boxed)?;
    let mut person = document(&fixture).await?.domain.entities[&endpoint(1).parse()?].clone();
    person["name"] = json!("Changed after matrix review");
    let concurrent = CommandId::new(&SystemIdGenerator)?;
    let _: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "scenario_apply_batch",
        &json!({
            "requestId":next()?,"commandId":concurrent,"scenarioId":fixture.scenario,"expectedRevision":1,
            "actor":{"actorId":null,"displayName":"Concurrent edit"},"truncateRedo":false,
            "label":"Change matrix person name","commands":[{"type":"applyDomainCommand",
                "payload":{"commandType":"official.workforce.update_entity","payload":{"entity":person}}}]
        }),
    )?;
    assert_eq!(
        heavy(
            &fixture.window,
            "csv_apply",
            apply_request(
                &fixture,
                1,
                source,
                &first.result,
                CommandId::new(&SystemIdGenerator)?
            )?
        )?
        .err()
        .ok_or("stale matrix approval")?["code"],
        "scenario.revision_conflict"
    );
    assert_history(&fixture, 2, concurrent).await?;
    let mut original = String::new();
    let source_lease = fixture
        .state
        .csv
        .source("main", fixture.scenario, source)
        .map_err(boxed)?;
    std::io::Read::read_to_string(&mut source_lease.reader(), &mut original)?;
    assert_eq!(original, text);
    let renewed = heavy(
        &fixture.window,
        "eligibility_matrix_preview",
        matrix_native_request(
            &fixture,
            2,
            source,
            json!({"personMappings":{},"assignmentTypeMappings":{}}),
            json!([]),
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        renewed.result["preview"]["source"],
        first.result["preview"]["source"]
    );
    assert_ne!(
        renewed.result["preview"]["approvalDigest"],
        first.result["preview"]["approvalDigest"]
    );
    let command = CommandId::new(&SystemIdGenerator)?;
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(&fixture, 2, source, &renewed.result, command)?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"applied","commandId":command,"revision":3})
    );
    person["eligibleAssignmentTypeIds"] = json!([endpoint(21)]);
    assert_eq!(
        document(&fixture).await?.domain.entities[&endpoint(1).parse()?],
        person
    );
    assert_history(&fixture, 3, command).await?;
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_err()
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn eligibility_matrix_native_complete_exclusions_and_pretyped_policy_admission() -> TestResult
{
    let fixture = native_fixture().await?;
    let before = document(&fixture).await?;
    let text = "personId\tunknown\n".to_owned() + &"unknown\ttrue\n".repeat(10_000);
    let source = matrix_source(&fixture, 0, &text)?;
    let request = matrix_native_request(
        &fixture,
        0,
        source,
        json!({"personMappings":{},"assignmentTypeMappings":{}}),
        json!(
            (2..=10_001)
                .map(|record| json!({"record":record,"column":1}))
                .collect::<Vec<_>>()
        ),
    )?;
    for field in ["personMappings", "assignmentTypeMappings"] {
        let mut invalid = request.clone();
        invalid["mapping"][field] = Value::Object(
            (0..10_001)
                .map(|index| (index.to_string(), Value::Null))
                .collect(),
        );
        assert_eq!(
            heavy(&fixture.window, "eligibility_matrix_preview", invalid)?
                .err()
                .ok_or("pretyped role alias cardinality")?["code"],
            "people_csv.request_too_large"
        );
    }
    let mut oversized_skips = request.clone();
    oversized_skips["skippedCells"] = json!(vec![Value::Null; MAX_CSV_DECISIONS + 1]);
    assert_eq!(
        heavy(
            &fixture.window,
            "eligibility_matrix_preview",
            oversized_skips
        )?
        .err()
        .ok_or("pretyped matrix skips")?["code"],
        "people_csv.request_too_large"
    );
    // Admission did not claim the original preview operation.
    let preview = heavy(&fixture.window, "eligibility_matrix_preview", request)?.map_err(boxed)?;
    assert_eq!(preview.result["preview"]["disposition"], "noChanges");
    let exclusions = preview.result["preview"]["excludedCells"]
        .as_array()
        .ok_or("full exclusions")?;
    assert_eq!(exclusions.len(), 10_000);
    assert_eq!(exclusions[0]["cell"], json!({"record":2,"column":1}));
    assert_eq!(
        exclusions[9_999]["cell"],
        json!({"record":10_001,"column":1})
    );
    let applied = heavy(
        &fixture.window,
        "csv_apply",
        apply_request(
            &fixture,
            0,
            source,
            &preview.result,
            CommandId::new(&SystemIdGenerator)?,
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(
        applied.result["outcome"],
        json!({"kind":"noChanges","revision":0})
    );
    assert_eq!(
        applied.result["report"]["rows"]["family"],
        "eligibilityMatrix"
    );
    assert_eq!(
        applied.result["report"]["rows"]["excludedCells"],
        preview.result["preview"]["excludedCells"]
    );
    assert!(serde_json::to_vec(&applied.result)?.len() > 128 * 1024);
    let report: ApiResponseDto<Value> = invoke_ok(
        &fixture.window,
        "csv_report",
        &json!({
            "schemaVersion":1,"requestId":next()?,"scenarioId":fixture.scenario,"previewId":preview.result["previewId"]
        }),
    )?;
    assert_eq!(report.result, applied.result["report"]);
    assert_eq!(report.result["consumed"], true);
    assert!(
        fixture
            .state
            .csv
            .source("main", fixture.scenario, source)
            .is_err()
    );
    assert_eq!(document(&fixture).await?, before);
    match fixture
        .state
        .app
        .query(AppQuery::History(fixture.scenario))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::History(history) => assert!(history.is_empty()),
        _ => return Err("unexpected history query".into()),
    }
    Ok(())
}
