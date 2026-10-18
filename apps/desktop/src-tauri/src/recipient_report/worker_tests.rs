#![cfg(all(test, feature = "bundled-ortools"))]

use super::*;
use custody::ReportCustody;
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, CapturedRecipientReport,
    CapturedScheduleExport, EuthetoApp, ScheduleExportFormatV1, ScheduleExportPreviewV1,
    ScheduleExportRequestV1, StoredSolveRequest,
};
use eutheto_solver_api::{OutputError, ProgressSink, SolveProgressEvent};
use eutheto_solver_ortools::{ORTOOLS_BACKEND_ID, VerifiedWorkerArtifact, registry_with_ortools};
use eutheto_types::{
    ActorRef, BackendSelection, CancellationToken, CommandBatch, CommandEnvelope, CommandId,
    CommandSource, DomainCommandEnvelope, ExplanationMode, FixedMonotonicClock, PreservationPolicy,
    ReproducibilityMode, ResourceLimits, ScenarioCommand, SolveMode, SolveOptions, SystemClock,
    SystemIdGenerator, SystemMonotonicClock, WorkerThreadPolicy,
};
use serde_json::{Value, json};
use std::{error::Error, io, sync::Arc};
use tauri::ipc::{InvokeResponseBody, IpcResponse};

#[path = "../../../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const BYTE_LIMIT: usize = 64 * 1024 * 1024;

fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    io::Error::other(format!("{error:?}")).into()
}

fn request_id() -> TestResult<RequestId> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}

fn assert_code<T>(result: Result<T, ApiError>, code: &str) {
    assert!(matches!(result, Err(error) if error.code == code));
}

struct Progress;
impl ProgressSink for Progress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}

// Field order releases Core's database actor before removing its files, including on failure.
struct Fixture {
    app: EuthetoApp,
    custody: Arc<ReportCustody>,
    clock: Arc<FixedMonotonicClock>,
    scope: Scope,
    solution_id: SolutionId,
    directory: tempfile::TempDir,
}

#[allow(clippy::too_many_lines)]
async fn fixture() -> TestResult<Fixture> {
    // Approved local invocation: EUTHETO_TEST_ORTOOLS_ARTIFACT=
    // /nix/store/q5znqcb68i0kawbc87z84jivm4nalksd-eutheto-ortools-worker-0.1.0
    let root = PathBuf::from(std::env::var_os("EUTHETO_TEST_ORTOOLS_ARTIFACT").ok_or(
        "EUTHETO_TEST_ORTOOLS_ARTIFACT must name an approved installed real worker; no helper fallback",
    )?);
    let bytes = std::fs::read(root.join("solver-manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    if manifest["approval"].is_null()
        || manifest["backend_source"]["sha256"]
            .as_str()
            .is_none_or(|value| value.len() != 64)
    {
        return Err("native recipient evidence requires an approved real worker artifact".into());
    }
    let hex = eutheto_export::sha256_hex(&bytes);
    let mut digest = [0_u8; 32];
    for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    let artifact = VerifiedWorkerArtifact::verify(root, digest).await?;
    let registry = registry_with_ortools(artifact)?;
    let runtime = registry
        .get(&ORTOOLS_BACKEND_ID.parse()?)
        .ok_or("missing registered OR-Tools runtime")?
        .runtime_identity()
        .clone();
    let directory = tempfile::Builder::new()
        .prefix("eutheto-native-recipient-")
        .tempdir_in(std::env::var_os("HOME").ok_or("platform home directory is unavailable")?)?;
    let app = EuthetoApp::open_with_solver_registry(
        AppDependencies {
            paths: AppPaths {
                database: directory.path().join("eutheto.sqlite"),
                safety_backups: directory.path().join("backups"),
            },
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(SystemMonotonicClock::new()),
            ids: Arc::new(SystemIdGenerator),
            cancellation: CancellationToken::new(),
        },
        registry,
    )
    .await
    .map_err(boxed)?;

    // The same supported one-person universe as Core's headless_worker fixture.
    // Install it through public Core commands; native tests need no store/domain dependencies.
    let mut source = serde_json::to_value(workforce_fixture::fixture()?)?;
    source["settings"]["timeZone"] = json!("UTC");
    source["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    let shift = &mut source["domain"]["entities"][workforce_fixture::id(8)];
    shift["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z","local":"2026-11-01T08:00:00","offsetSeconds":0
    });
    shift["endsAt"] = json!({
        "instant":"2026-11-01T10:00:00Z","local":"2026-11-01T10:00:00","offsetSeconds":0
    });
    let AppCommandResult::Project(project) = app
        .execute(AppCommand::CreateProject {
            request_id: request_id()?,
            title: "Native recipient regression".to_owned(),
            description: String::new(),
            domain_pack: serde_json::from_value(source["domainPack"].clone())?,
            settings: serde_json::from_value(source["settings"].clone())?,
        })
        .await
        .map_err(boxed)?
    else {
        return Err("missing project receipt".into());
    };
    // Workforce validates every prefix, so referenced records precede their dependants.
    let mut commands = Vec::new();
    for index in [2, 3, 11, 4, 5, 1, 8, 9] {
        commands.push(ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: "official.workforce.add_entity".to_owned(),
            payload: json!({"entity": source["domain"]["entities"][workforce_fixture::id(index)]}),
        }));
    }
    for (index, kind) in [
        (30, "eligibility"),
        (31, "availability"),
        (32, "coverage"),
        (33, "noOverlap"),
    ] {
        let mut rule = json!({
            "id":workforce_fixture::id(index),"kind":kind,"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}}
        });
        if kind == "noOverlap" {
            rule["compatibleCategoryPairs"] = json!([]);
        }
        commands.push(ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: "official.workforce.add_rule".to_owned(),
            payload: json!({"rule":rule}),
        }));
    }
    let AppCommandResult::ScenarioCommand(edit) = app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id: project.scenario_id,
                expected_revision: project.revision,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Recipient regression".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyBatch(CommandBatch {
                    label: None,
                    commands,
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?
    else {
        return Err("missing fixture mutation receipt".into());
    };
    let outcome = Box::pin(app.solve_stored(
        StoredSolveRequest {
            request_id: request_id()?,
            scenario_id: project.scenario_id,
            expected_revision: Some(edit.new_revision),
            options: SolveOptions {
                backend: BackendSelection::Specific(runtime.backend_id().clone()),
                mode: SolveMode::Balanced,
                time_limit_milliseconds: DurationMillis::new(60_000)?,
                memory_limit_bytes: None,
                worker_threads: WorkerThreadPolicy::Exact(1),
                random_seed: 1,
                solution_limit: None,
                stop_after_first_feasible: false,
                collect_intermediate_solutions: false,
                explanation_mode: ExplanationMode::None,
                preserve_existing: PreservationPolicy::None,
                reproducibility: ReproducibilityMode::Deterministic,
                resource_limits: ResourceLimits {
                    max_entities: 100,
                    max_rules: 100,
                    max_variables: 100,
                    max_constraints: 100,
                },
            },
            objective_selection: None,
        },
        &mut Progress,
    ))
    .await
    .map_err(boxed)?;
    let execution = &outcome
        .report
        .as_ref()
        .ok_or("missing real worker solve report")?
        .execution;
    assert_eq!(execution.invocation_count, 1);
    let [attempt] = execution.attempts.as_slice() else {
        return Err("real worker solve did not retain exactly one attempt".into());
    };
    assert!(!attempt.fallback_taken);
    let backend = attempt
        .outcome
        .as_ref()
        .ok_or("missing real worker outcome")?;
    let evidence = &backend
        .evidence
        .execution
        .as_ref()
        .ok_or("missing actual worker runtime evidence")?
        .reproducibility;
    assert_eq!(&backend.backend_id, runtime.backend_id());
    assert_eq!(&outcome.state().input.backend_id, runtime.backend_id());
    assert_eq!(evidence.worker_version, runtime.worker_version());
    assert_eq!(evidence.engine_version, runtime.solver_version());
    assert_eq!(evidence.applied_parameters.worker_threads, 1);
    let solution_id = outcome
        .accepted_result_id()
        .ok_or("real worker returned no accepted result")?;
    let clock = Arc::new(FixedMonotonicClock::default());
    let custody = Arc::new(ReportCustody::new(
        clock.clone(),
        Arc::new(SystemIdGenerator),
        ["main".to_owned(), "other".to_owned(), "third".to_owned()],
    ));
    Ok(Fixture {
        app,
        custody,
        clock,
        scope: Scope {
            scenario_id: project.scenario_id,
            expected_revision: edit.new_revision,
            kind: CaptureKind::Recipient,
        },
        solution_id,
        directory,
    })
}

fn reserve(fixture: &Fixture, owner: &str) -> TestResult<OperationId> {
    reserve_scope(fixture, owner, fixture.scope)
}

fn reserve_scope(fixture: &Fixture, owner: &str, scope: Scope) -> TestResult<OperationId> {
    let operation_id = OperationId::new(&SystemIdGenerator)?;
    fixture
        .custody
        .reserve(
            owner,
            operation_id,
            scope,
            fixture.custody.reservation_instant().map_err(boxed)?,
        )
        .map_err(boxed)?;
    Ok(operation_id)
}

fn assert_full(fixture: &Fixture, owner: &str) -> TestResult {
    assert_code(
        fixture.custody.reserve(
            owner,
            OperationId::new(&SystemIdGenerator)?,
            fixture.scope,
            fixture.custody.reservation_instant().map_err(boxed)?,
        ),
        "recipient.preview_capacity",
    );
    Ok(())
}

async fn capture(fixture: &Fixture) -> TestResult<(CapturedRecipientReport, RecipientReportV1)> {
    fixture
        .app
        .prepare_recipient_report(
            RecipientReportRequestV1 {
                schema_version: 1,
                scenario_id: fixture.scope.scenario_id,
                solution_id: fixture.solution_id,
                expected_revision: fixture.scope.expected_revision,
                options: RecipientReportOptionsV1::default(),
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)
}

struct EncodedPreview {
    response: RecipientPreviewResponse,
    operation_id: OperationId,
    preview_id: RequestId,
    html: Vec<u8>,
    report: Value,
    charge: usize,
}

async fn encode_for_operation(
    fixture: &Fixture,
    owner: &str,
    operation_id: OperationId,
) -> TestResult<EncodedPreview> {
    let mut preparing = fixture
        .custody
        .begin(owner, operation_id, fixture.scope)
        .map_err(boxed)?;
    let (capture, report) = capture(fixture).await?;
    let control = OperationControl::Cancellation(CancellationToken::new());
    let html = capture.html_bytes().to_vec();
    let expected_report = serde_json::to_value(&report)?;
    let library_revision = capture.metadata().library_revision;
    let model_sha256 = capture.model_sha256().to_owned();
    let html_sha256 = capture.html_sha256().to_owned();
    let capture = CapturedReport::Recipient(capture);
    let charge = preview_charge(&capture, &report, &control).map_err(boxed)?;
    let preview_id = preparing.stage(capture, charge).map_err(boxed)?;
    let response = encode_preview(
        request_id()?,
        fixture.scope.expected_revision,
        RecipientPreviewDtoV1 {
            schema_version: 1,
            preview_id,
            format: RecipientFileFormatV1::Html,
            library_revision,
            model_sha256,
            html_sha256,
            pdf_sha256: None,
            pdf_renderer: None,
            expires_after_milliseconds: 600_000,
            report,
        },
        preparing,
        control,
        PREVIEW_COMPACT_BYTES,
    )
    .await
    .map_err(boxed)?;
    Ok(EncodedPreview {
        response,
        operation_id,
        preview_id,
        html,
        report: expected_report,
        charge,
    })
}

async fn preview(fixture: &Fixture, owner: &str) -> TestResult<EncodedPreview> {
    encode_for_operation(fixture, owner, reserve(fixture, owner)?).await
}

fn body_json(response: impl IpcResponse) -> TestResult<Value> {
    match response.body()? {
        InvokeResponseBody::Json(json) => Ok(serde_json::from_str(&json)?),
        InvokeResponseBody::Raw(_) => Err("recipient response was not JSON".into()),
    }
}

fn consume(fixture: &Fixture, preview_id: RequestId) -> Result<ConsumingReport, ApiError> {
    fixture
        .custody
        .consume("main", preview_id, fixture.scope, CaptureFormat::Html)
}

fn schedule_scope(fixture: &Fixture) -> Scope {
    Scope {
        kind: CaptureKind::Schedule,
        ..fixture.scope
    }
}

async fn capture_schedule(
    fixture: &Fixture,
    format: ScheduleExportFormatV1,
) -> TestResult<(CapturedScheduleExport, ScheduleExportPreviewV1)> {
    fixture
        .app
        .prepare_schedule_export(
            ScheduleExportRequestV1 {
                schema_version: 1,
                scenario_id: fixture.scope.scenario_id,
                solution_id: fixture.solution_id,
                expected_revision: fixture.scope.expected_revision,
                format,
                person_id: None,
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)
}

async fn native_schedule_preview<R: tauri::Runtime>(
    fixture: &Fixture,
    window: &tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    format: ScheduleExportFormatV1,
) -> TestResult<(RecipientPreviewResponse, OperationId)> {
    let prepared = crate::setup_boundary::operation_prepare(
        window.clone(),
        state.clone(),
        Some(json!({
            "requestId":request_id()?,"schemaVersion":1,"purpose":{"kind":"exportPreview"},
            "context":{"kind":"scenario","scenarioId":fixture.scope.scenario_id,
                "expectedRevision":fixture.scope.expected_revision}
        })),
    )
    .map_err(boxed)?;
    let response = crate::schedule_export::solution_export_preview(
        window.clone(),
        state,
        NativeOperationRequest(Some(json!({
            "requestId":request_id()?,"schemaVersion":1,"operationId":prepared.result.operation_id,
            "scenarioId":fixture.scope.scenario_id,"expectedRevision":fixture.scope.expected_revision,
            "solutionId":fixture.solution_id,"format":format,"personId":null
        }))),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .map_err(boxed)?;
    Ok((response, prepared.result.operation_id))
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn pending_encoded_reply_release_and_drop_keep_count_and_bytes_until_retirement() -> TestResult
{
    let fixture = fixture().await?;
    let pending = preview(&fixture, "main").await?;
    assert_code(
        consume(&fixture, pending.preview_id),
        "recipient.preview_not_active",
    );
    let idle = reserve(&fixture, "main")?;
    assert_full(&fixture, "main")?;
    let probe_id = reserve(&fixture, "other")?;
    let mut probe = fixture
        .custody
        .begin("other", probe_id, fixture.scope)
        .map_err(boxed)?;
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Recipient,
            Target::Preview {
                preview_id: pending.preview_id
            }
        ),
        ReleaseAcknowledgement::Closing
    );
    assert_full(&fixture, "main")?;
    // A conservative extra response-capacity allowance fits alone but cannot fit beside
    // the real encoded reply's production charge. No private ledger inspection is needed.
    let probe_charge = BYTE_LIMIT
        .checked_sub(pending.charge / 2)
        .ok_or("invalid preview charge")?;
    let (source, _) = capture(&fixture).await?;
    assert_code(
        probe.stage(CapturedReport::Recipient(source), probe_charge),
        "recipient.preview_capacity",
    );
    let retired_id = pending.preview_id;
    drop(pending);
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Recipient,
            Target::Preview {
                preview_id: retired_id
            }
        ),
        ReleaseAcknowledgement::NotActive
    );
    let (source, _) = capture(&fixture).await?;
    probe
        .stage(CapturedReport::Recipient(source), probe_charge)
        .map_err(boxed)?;
    drop(probe);
    fixture.custody.release_reserved("main", idle);
    reserve(&fixture, "main")?;
    reserve(&fixture, "main")?;
    reserve(&fixture, "other")?;
    assert_full(&fixture, "third")?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn closing_encoded_handoff_never_resurrects_released_or_closed_authority() -> TestResult {
    let fixture = fixture().await?;
    for close_window in [false, true] {
        let pending = preview(&fixture, "main").await?;
        if close_window {
            fixture.custody.close_window("main");
        }
        assert_eq!(
            fixture.custody.release(
                "main",
                fixture.scope.scenario_id,
                CaptureKind::Recipient,
                Target::Operation {
                    operation_id: pending.operation_id
                }
            ),
            ReleaseAcknowledgement::Closing
        );
        let first = reserve(&fixture, "other")?;
        let second = reserve(&fixture, "other")?;
        assert_full(&fixture, "third")?;
        let value = body_json(pending.response)?;
        assert_eq!(value["result"]["report"], pending.report);
        assert_eq!(value["result"]["previewId"], json!(pending.preview_id));
        assert_code(
            consume(&fixture, pending.preview_id),
            "recipient.preview_not_active",
        );
        assert_eq!(
            fixture.custody.release(
                "main",
                fixture.scope.scenario_id,
                CaptureKind::Recipient,
                Target::Preview {
                    preview_id: pending.preview_id
                }
            ),
            ReleaseAcknowledgement::NotActive
        );
        let replacement = reserve(&fixture, "third")?;
        fixture.custody.release_reserved("other", first);
        fixture.custody.release_reserved("other", second);
        fixture.custody.release_reserved("third", replacement);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn ready_single_use_publication_and_encoded_receipt_preserve_native_custody() -> TestResult {
    let fixture = fixture().await?;
    let mut state = DesktopState::new(
        fixture.app.clone(),
        fixture.directory.path().join("cache"),
        fixture.directory.path().join("backups"),
    );
    state.recipient_reports = fixture.custody.clone();
    let desktop = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let window =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    let prepared = crate::setup_boundary::operation_prepare(
        window.clone(),
        desktop.state(),
        Some(json!({
            "requestId":request_id()?,"schemaVersion":1,"purpose":{"kind":"sharePreview"},
            "context":{"kind":"scenario","scenarioId":fixture.scope.scenario_id,
                "expectedRevision":fixture.scope.expected_revision}
        })),
    )
    .map_err(boxed)?;
    let pending = encode_for_operation(&fixture, "main", prepared.result.operation_id).await?;
    assert_code(
        consume(&fixture, pending.preview_id),
        "recipient.preview_not_active",
    );
    let value = body_json(pending.response)?;
    assert_eq!(value["result"]["report"], pending.report);
    assert_eq!(
        value["result"]["htmlSha256"],
        json!(eutheto_export::sha256_hex(&pending.html))
    );
    // Invoke the actual generic cleanup handler, including its Released -> release_reserved
    // branch. The source/encoder regression deliberately does not claim a solver operation.
    let released = crate::setup_boundary::operation_release(
        window,
        desktop.state(),
        Some(json!({
            "requestId":request_id()?,"schemaVersion":1,"operationId":pending.operation_id
        })),
    )
    .map_err(boxed)?;
    assert_eq!(
        released.result.acknowledgement,
        crate::operations::ReleaseAcknowledgementV1::Released
    );
    let consuming = Arc::new(consume(&fixture, pending.preview_id).map_err(boxed)?);
    assert_code(
        consume(&fixture, pending.preview_id),
        "recipient.preview_not_active",
    );
    let idle = reserve(&fixture, "main")?;
    assert_full(&fixture, "main")?;
    let captured = consuming
        .take_capture()
        .map_err(boxed)?
        .into_recipient()
        .map_err(boxed)?;
    assert_code(consuming.take_capture(), "recipient.preview_not_active");
    consuming.admit_if_retained().map_err(boxed)?;
    assert_code(consuming.admit(), "recipient.preview_not_active");
    let model_sha256 = captured.model_sha256().to_owned();
    // Only this regression bypasses the native picker: publication still uses the retained
    // actual Core capture. A displayed picker and browser rendering require visual coverage.
    let destination = fixture.directory.path().join("recipient.html");
    let publication = captured
        .publish_html(
            destination.clone(),
            false,
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(std::fs::read(&destination)?, pending.html);
    assert_eq!(publication.solution_id, fixture.solution_id);
    assert_eq!(publication.source_revision, fixture.scope.expected_revision);
    assert_eq!(
        publication.current_revision,
        fixture.scope.expected_revision
    );
    assert_eq!(publication.model_sha256, model_sha256);
    assert_eq!(
        publication.html_sha256,
        eutheto_export::sha256_hex(&pending.html)
    );
    let expected_publication = serde_json::to_value(&publication)?;
    let receipt = encode_created(
        request_id()?,
        publication.current_revision,
        RecipientCreatedDtoV1 {
            schema_version: 1,
            preview_id: pending.preview_id,
            format: RecipientFileFormatV1::Html,
            filename: "recipient.html".to_owned(),
            publication: RecipientPublicationV1::Html(publication),
        },
        consuming,
    )
    .await
    .map_err(boxed)?;
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Recipient,
            Target::Preview {
                preview_id: pending.preview_id
            }
        ),
        ReleaseAcknowledgement::Closing
    );
    assert_full(&fixture, "main")?;
    let receipt = body_json(receipt)?;
    assert_eq!(receipt["result"]["publication"], expected_publication);
    assert_eq!(receipt["result"]["filename"], "recipient.html");
    assert_code(
        consume(&fixture, pending.preview_id),
        "recipient.preview_not_active",
    );
    let replacement = reserve(&fixture, "main")?;
    fixture.custody.release_reserved("main", idle);
    fixture.custody.release_reserved("main", replacement);

    let collision = preview(&fixture, "main").await?;
    body_json(collision.response)?;
    let consuming = consume(&fixture, collision.preview_id).map_err(boxed)?;
    assert!(
        consuming
            .take_capture()
            .map_err(boxed)?
            .into_recipient()
            .map_err(boxed)?
            .publish_html(
                destination.clone(),
                false,
                OperationControl::Cancellation(CancellationToken::new())
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&destination)?, pending.html);
    assert_code(
        consume(&fixture, collision.preview_id),
        "recipient.preview_not_active",
    );
    drop(consuming);

    let expiring = preview(&fixture, "main").await?;
    body_json(expiring.response)?;
    fixture.clock.advance(custody::READY_TTL)?;
    assert_code(
        consume(&fixture, expiring.preview_id),
        "recipient.preview_not_active",
    );
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Recipient,
            Target::Preview {
                preview_id: expiring.preview_id
            }
        ),
        ReleaseAcknowledgement::NotActive
    );
    reserve(&fixture, "main")?;
    reserve(&fixture, "main")?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn queued_create_cancellation_retains_real_capture_until_outer_guard_settles() -> TestResult {
    use crate::operations::{
        OperationContextV1, OperationControlRequestV1, OperationPrepareRequestV1, OperationRegistry,
    };
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use tokio::sync::{Notify, oneshot};

    let fixture = fixture().await?;
    let pending = preview(&fixture, "main").await?;
    body_json(pending.response)?;
    let retained = Arc::new(consume(&fixture, pending.preview_id).map_err(boxed)?);
    let idle = reserve(&fixture, "main")?;
    let registry = Arc::new(OperationRegistry::new(
        fixture.app.clone(),
        Arc::new(SystemClock),
        fixture.clock.clone(),
        Arc::new(SystemIdGenerator),
        ["main".to_owned()],
    ));
    let mut holders = Vec::new();
    // Both existing heavy permits are held by real OperationRegistry jobs. These
    // explicitly test-scoped synchronization gates do not replace any source authority.
    for _ in 0..2 {
        let request_id = request_id()?;
        let prepared = registry
            .prepare(
                "main",
                &OperationPrepareRequestV1 {
                    request_id,
                    schema_version: 1,
                    purpose: OperationPurposeV1::ScenarioSummary,
                    context: OperationContextV1::Scenario {
                        scenario_id: fixture.scope.scenario_id,
                        expected_revision: Some(fixture.scope.expected_revision),
                    },
                },
            )
            .map_err(boxed)?;
        let operation = claim(
            prepared.operation_id,
            request_id,
            OperationPurposeV1::ScenarioSummary,
            fixture.scope.scenario_id,
            Some(fixture.scope.expected_revision),
        );
        let (entered, started) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let registry = registry.clone();
        let caller = tokio::spawn(async move {
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
                        Ok::<(), ApiError>(())
                    },
                )
                .await
        });
        holders.push((caller, release));
        tokio::time::timeout(Duration::from_secs(10), started).await??;
    }
    let create_request_id = request_id()?;
    let prepared = registry
        .prepare(
            "main",
            &OperationPrepareRequestV1 {
                request_id: create_request_id,
                schema_version: 1,
                purpose: OperationPurposeV1::ShareCreate,
                context: OperationContextV1::Scenario {
                    scenario_id: fixture.scope.scenario_id,
                    expected_revision: Some(fixture.scope.expected_revision),
                },
            },
        )
        .map_err(boxed)?;
    let operation = claim(
        prepared.operation_id,
        create_request_id,
        OperationPurposeV1::ShareCreate,
        fixture.scope.scenario_id,
        Some(fixture.scope.expected_revision),
    );
    let waiting = Arc::new(Notify::new());
    let work_started = Arc::new(AtomicBool::new(false));
    let time_limit = DurationMillis::new(30_000)?;
    let caller = tokio::spawn({
        let registry = registry.clone();
        let retained = retained.clone();
        let waiting = waiting.clone();
        let work_started = work_started.clone();
        async move {
            registry
                .run_with_mutation_budget(
                    "main",
                    operation,
                    Some(Arc::new(move |event| {
                        if event.phase == OperationPhaseV1::WaitingForAdmission {
                            waiting.notify_one();
                        }
                    })),
                    OperationPhaseV1::SelectingFile,
                    (retained, None),
                    time_limit,
                    move |_retained, _| async move {
                        work_started.store(true, Ordering::SeqCst);
                        Ok::<(), ApiError>(())
                    },
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), waiting.notified()).await?;
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Recipient,
            Target::Preview {
                preview_id: pending.preview_id
            }
        ),
        ReleaseAcknowledgement::Closing
    );
    assert_full(&fixture, "main")?;
    registry
        .cancel(
            "main",
            &OperationControlRequestV1 {
                request_id: request_id()?,
                schema_version: 1,
                operation_id: prepared.operation_id,
            },
        )
        .map_err(boxed)?;
    assert_code(
        tokio::time::timeout(Duration::from_secs(10), caller).await??,
        "operation.cancelled",
    );
    assert!(!work_started.load(Ordering::SeqCst));
    assert_full(&fixture, "main")?;
    // The queued job dropped its clone, but the caller still has the actual source
    // for post-Err readmission; closing is cleanup, not a source admission failure.
    retained.admit_if_retained().map_err(boxed)?;
    retained.admit().map_err(boxed)?;
    drop(retained);
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Recipient,
            Target::Preview {
                preview_id: pending.preview_id
            }
        ),
        ReleaseAcknowledgement::NotActive
    );
    let replacement = reserve(&fixture, "main")?;
    fixture.custody.release_reserved("main", idle);
    fixture.custody.release_reserved("main", replacement);
    for (caller, release) in holders {
        release.send(()).map_err(boxed)?;
        tokio::time::timeout(Duration::from_secs(10), caller)
            .await??
            .map_err(boxed)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn html_and_schedule_share_count_window_and_retained_byte_limits() -> TestResult {
    let fixture = fixture().await?;
    let html = preview(&fixture, "main").await?;
    let schedule_id = reserve_scope(&fixture, "other", schedule_scope(&fixture))?;
    assert_code(
        fixture.custody.begin("other", schedule_id, fixture.scope),
        "recipient.preview_binding_mismatch",
    );
    let mut schedule = fixture
        .custody
        .begin("other", schedule_id, schedule_scope(&fixture))
        .map_err(boxed)?;
    let idle = reserve_scope(&fixture, "main", schedule_scope(&fixture))?;
    assert_full(&fixture, "third")?;
    fixture.custody.release_reserved("main", idle);
    assert_code(
        schedule.stage(
            CapturedReport::Recipient(capture(&fixture).await?.0),
            html.charge,
        ),
        "recipient.preview_binding_mismatch",
    );
    let charge = BYTE_LIMIT
        .checked_sub(html.charge / 2)
        .ok_or("invalid preview charge")?;
    assert_code(
        schedule.stage(
            CapturedReport::Schedule(
                capture_schedule(&fixture, ScheduleExportFormatV1::AssignmentsCsv)
                    .await?
                    .0,
            ),
            charge,
        ),
        "recipient.preview_capacity",
    );
    // A schedule capture that cannot fit beside HTML fits after the still-owned reply drops.
    drop(html);
    schedule
        .stage(
            CapturedReport::Schedule(
                capture_schedule(&fixture, ScheduleExportFormatV1::AssignmentsCsv)
                    .await?
                    .0,
            ),
            charge,
        )
        .map_err(boxed)?;
    drop(schedule);
    let html = preview(&fixture, "main").await?;
    body_json(html.response)?;
    assert_code(
        fixture.custody.consume(
            "main",
            html.preview_id,
            schedule_scope(&fixture),
            CaptureFormat::Schedule(ScheduleExportFormatV1::AssignmentsCsv),
        ),
        "recipient.preview_binding_mismatch",
    );
    assert_eq!(
        fixture.custody.release(
            "main",
            fixture.scope.scenario_id,
            CaptureKind::Schedule,
            Target::Preview {
                preview_id: html.preview_id
            },
        ),
        ReleaseAcknowledgement::NotActive
    );
    assert_code(
        fixture
            .custody
            .consume("main", html.preview_id, fixture.scope, CaptureFormat::Pdf),
        "recipient.preview_binding_mismatch",
    );
    // Cross-kind refusals did not spend the HTML review's single use either.
    drop(consume(&fixture, html.preview_id).map_err(boxed)?);
    let html_id = reserve(&fixture, "main")?;
    let schedule_id = reserve_scope(&fixture, "main", schedule_scope(&fixture))?;
    assert_full(&fixture, "main")?;
    let other = reserve_scope(&fixture, "other", schedule_scope(&fixture))?;
    assert_full(&fixture, "third")?;
    fixture.custody.release_reserved("main", html_id);
    fixture.custody.release_reserved("main", schedule_id);
    fixture.custody.release_reserved("other", other);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn schedule_response_drop_and_closing_handoff_retire_shared_custody() -> TestResult {
    let fixture = fixture().await?;
    let mut state = DesktopState::new(
        fixture.app.clone(),
        fixture.directory.path().join("cache"),
        fixture.directory.path().join("backups"),
    );
    state.recipient_reports = fixture.custody.clone();
    let desktop = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let window =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    // Abandonment, explicit review release, and window cleanup all own the pending reply.
    for mode in 0..3 {
        let (pending, operation_id) = native_schedule_preview(
            &fixture,
            &window,
            desktop.state(),
            ScheduleExportFormatV1::SummaryCsv,
        )
        .await?;
        let idle = reserve(&fixture, "main")?;
        let other = reserve(&fixture, "other")?;
        assert_full(&fixture, "third")?;
        if mode == 0 {
            drop(pending);
        } else {
            assert_eq!(
                fixture.custody.release(
                    "main",
                    fixture.scope.scenario_id,
                    CaptureKind::Recipient,
                    Target::Operation { operation_id },
                ),
                ReleaseAcknowledgement::NotActive
            );
            if mode == 2 {
                fixture.custody.close_window("main");
            }
            let released = crate::schedule_export::solution_export_release(
                window.clone(),
                desktop.state(),
                NativeOperationRequest(Some(json!({
                    "requestId":request_id()?,"schemaVersion":1,
                    "scenarioId":fixture.scope.scenario_id,
                    "target":{"kind":"operation","operationId":operation_id}
                }))),
            )
            .map_err(boxed)?;
            assert_eq!(
                released.result.acknowledgement,
                ReleaseAcknowledgement::Closing
            );
            if mode == 1 {
                assert_full(&fixture, "third")?;
            } else {
                // Closing the window already retired the idle HTML reservation.
                let replacement = reserve(&fixture, "other")?;
                assert_full(&fixture, "third")?;
                fixture.custody.release_reserved("other", replacement);
            }
            let value = body_json(pending)?;
            assert_eq!(value["result"]["preview"]["format"], "summaryCsv");
            let preview_id = serde_json::from_value(value["result"]["previewId"].clone())?;
            assert_code(
                fixture.custody.consume(
                    "main",
                    preview_id,
                    schedule_scope(&fixture),
                    CaptureFormat::Schedule(ScheduleExportFormatV1::SummaryCsv),
                ),
                "recipient.preview_not_active",
            );
        }
        assert_eq!(
            fixture.custody.release(
                "main",
                fixture.scope.scenario_id,
                CaptureKind::Schedule,
                Target::Operation { operation_id },
            ),
            ReleaseAcknowledgement::NotActive
        );
        let replacement = reserve(&fixture, "third")?;
        fixture.custody.release_reserved("main", idle);
        fixture.custody.release_reserved("other", other);
        fixture.custody.release_reserved("third", replacement);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn schedule_refusals_preserve_review_and_exact_publication_receipt() -> TestResult {
    let fixture = fixture().await?;
    let mut state = DesktopState::new(
        fixture.app.clone(),
        fixture.directory.path().join("cache"),
        fixture.directory.path().join("backups"),
    );
    state.recipient_reports = fixture.custody.clone();
    let desktop = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?;
    let window =
        tauri::WebviewWindowBuilder::new(&desktop, "main", tauri::WebviewUrl::default()).build()?;
    for (format, filename) in [
        (ScheduleExportFormatV1::AssignmentsCsv, "assignments.csv"),
        (ScheduleExportFormatV1::SummaryCsv, "summary.csv"),
        (ScheduleExportFormatV1::Ics, "schedule.ics"),
    ] {
        let (pending, operation_id) =
            native_schedule_preview(&fixture, &window, desktop.state(), format).await?;
        let value = body_json(pending)?;
        let preview = &value["result"]["preview"];
        let preview_id = serde_json::from_value(value["result"]["previewId"].clone())?;
        let exact_format = CaptureFormat::Schedule(format);
        assert_code(
            consume(&fixture, preview_id),
            "recipient.preview_binding_mismatch",
        );
        assert_code(
            fixture
                .custody
                .consume("other", preview_id, schedule_scope(&fixture), exact_format),
            "recipient.preview_not_active",
        );
        let wrong_format = if format == ScheduleExportFormatV1::Ics {
            ScheduleExportFormatV1::SummaryCsv
        } else {
            ScheduleExportFormatV1::Ics
        };
        for rejected in [CaptureFormat::Html, CaptureFormat::Schedule(wrong_format)] {
            assert_code(
                fixture
                    .custody
                    .consume("main", preview_id, schedule_scope(&fixture), rejected),
                "recipient.preview_binding_mismatch",
            );
        }
        assert_code(
            fixture.custody.consume(
                "main",
                preview_id,
                Scope {
                    expected_revision: Revision::new(fixture.scope.expected_revision.value() + 1),
                    ..schedule_scope(&fixture)
                },
                exact_format,
            ),
            "recipient.preview_binding_mismatch",
        );
        for target in [
            Target::Preview { preview_id },
            Target::Operation { operation_id },
        ] {
            assert_eq!(
                fixture.custody.release(
                    "main",
                    fixture.scope.scenario_id,
                    CaptureKind::Recipient,
                    target,
                ),
                ReleaseAcknowledgement::NotActive
            );
        }
        let consuming = Arc::new(
            fixture
                .custody
                .consume("main", preview_id, schedule_scope(&fixture), exact_format)
                .map_err(boxed)?,
        );
        assert_code(
            fixture
                .custody
                .consume("main", preview_id, schedule_scope(&fixture), exact_format),
            "recipient.preview_not_active",
        );
        let captured = consuming
            .take_capture()
            .map_err(boxed)?
            .into_schedule()
            .map_err(boxed)?;
        let bytes = captured.bytes().to_vec();
        assert_eq!(preview["format"], json!(format));
        assert_eq!(preview["personId"], Value::Null);
        assert_eq!(preview["sha256"], eutheto_export::sha256_hex(&bytes));
        assert_eq!(preview["byteCount"], json!(bytes.len()));
        assert_eq!(preview["source"]["solutionId"], json!(fixture.solution_id));
        assert_eq!(
            preview["source"]["scenarioRevision"],
            json!(fixture.scope.expected_revision)
        );
        assert_eq!(preview["sampleTruncated"], false);
        assert_eq!(preview["sample"], std::str::from_utf8(&bytes)?);
        consuming.admit_if_retained().map_err(boxed)?;
        // Bypass only the picker; the real captured Core source still owns publication.
        let destination = fixture.directory.path().join(filename);
        let publication = captured
            .publish(
                destination.clone(),
                false,
                OperationControl::Cancellation(CancellationToken::new()),
            )
            .await
            .map_err(boxed)?;
        assert_eq!(std::fs::read(&destination)?, bytes);
        assert_eq!(publication.solution_id, fixture.solution_id);
        assert_eq!(publication.source_revision, fixture.scope.expected_revision);
        assert_eq!(
            publication.current_revision,
            fixture.scope.expected_revision
        );
        assert_eq!(publication.format, format);
        assert_eq!(publication.person_id, None);
        assert_eq!(publication.sha256, eutheto_export::sha256_hex(&bytes));
        assert_eq!(usize::try_from(publication.byte_count)?, bytes.len());
        let expected_publication = serde_json::to_value(&publication)?;
        let receipt = encode_created(
            request_id()?,
            publication.current_revision,
            json!({"schemaVersion":1,"previewId":preview_id,"filename":filename,
                "publication":publication}),
            consuming,
        )
        .await
        .map_err(boxed)?;
        let idle = reserve(&fixture, "main")?;
        assert_eq!(
            fixture.custody.release(
                "main",
                fixture.scope.scenario_id,
                CaptureKind::Schedule,
                Target::Preview { preview_id },
            ),
            ReleaseAcknowledgement::Closing
        );
        assert_full(&fixture, "main")?;
        let receipt = body_json(receipt)?;
        assert_eq!(receipt["result"]["previewId"], json!(preview_id));
        assert_eq!(receipt["result"]["filename"], filename);
        assert_eq!(receipt["result"]["publication"], expected_publication);
        assert_code(
            fixture
                .custody
                .consume("main", preview_id, schedule_scope(&fixture), exact_format),
            "recipient.preview_not_active",
        );
        let replacement = reserve_scope(&fixture, "main", schedule_scope(&fixture))?;
        fixture.custody.release_reserved("main", idle);
        fixture.custody.release_reserved("main", replacement);
    }
    Ok(())
}
