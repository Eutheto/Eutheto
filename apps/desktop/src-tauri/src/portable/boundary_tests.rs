use super::*;
use crate::operations::OperationPrepareRequestV1;
use crate::setup_boundary::operation_prepare;
use crate::tests::{invoke_ipc_args, invoke_ok, native_summary_and_core_snapshot};
use crate::{PortablePreviewDto, scenario_get_summary};
use eutheto_core::{AppDependencies, AppPaths, EuthetoApp};
use eutheto_import::{CollisionAction, RestoreMode};
use eutheto_types::{
    ActorRef, AddEntity, ApiResponseDto, CommandEnvelope, CommandId, CommandSource, DomainPackRef,
    EntityId, FixedClock, FixedMonotonicClock, ProjectListItemV1, ScenarioCommand,
    ScenarioSettings, SystemIdGenerator,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, io};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
type Window = tauri::WebviewWindow<tauri::test::MockRuntime>;

fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    io::Error::other(format!("{error:?}")).into()
}

fn next() -> TestResult<RequestId> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}

fn dependencies(directory: &std::path::Path, name: &str) -> TestResult<AppDependencies> {
    Ok(AppDependencies {
        paths: AppPaths {
            database: directory.join(format!("{name}.sqlite3")),
            safety_backups: directory.join(format!("{name}-backups")),
        },
        clock: Arc::new(FixedClock::new("2026-09-10T12:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    })
}

fn desktop(state: DesktopState) -> TestResult<tauri::App<tauri::test::MockRuntime>> {
    Ok(tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            operation_prepare,
            crate::project_create,
            crate::project_list,
            scenario_get_summary,
            project_import_apply,
            project_restore_apply,
            project_operation_cancel,
        ])
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))?)
}

async fn create_project(app: &EuthetoApp, title: &str) -> TestResult<ScenarioId> {
    let settings: ScenarioSettings = serde_json::from_value(json!({
        "timeZone":"UTC", "locale":"en-US", "units":"metric",
        "horizon":{"start":"2026-09-01T00:00:00Z", "end":"2026-10-01T00:00:00Z"},
        "gapPolicy":"reject", "overlapPolicy":"earlier"
    }))?;
    let AppCommandResult::Project(project) = app
        .execute(AppCommand::CreateProject {
            request_id: next()?,
            title: title.to_owned(),
            description: String::new(),
            domain_pack: DomainPackRef {
                id: "official.test".parse()?,
                schema_version: 1,
            },
            settings,
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong project receipt".into());
    };
    Ok(project.scenario_id)
}

async fn export(app: &EuthetoApp, scenario_id: ScenarioId) -> TestResult<Vec<u8>> {
    let AppQueryResult::Bundle { bytes, .. } = app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong export receipt".into());
    };
    Ok(bytes)
}

async fn backup(app: &EuthetoApp) -> TestResult<Vec<u8>> {
    let AppQueryResult::BackupBundle { bytes, .. } = app
        .query(AppQuery::ExportBackup {
            title: "Exact portable fixture".to_owned(),
            selection: BackupSelection::default(),
            cancellation: app.setup_cancellation(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong backup receipt".into());
    };
    Ok(bytes)
}

// The file picker is intentionally outside this headless test. The selected bytes still pass
// through real operation admission, core inspection and concrete owner/revision custody.
async fn preview(
    state: &DesktopState,
    bytes: Vec<u8>,
    mode: RestoreMode,
) -> TestResult<PortablePreviewDto> {
    let request_id = next()?;
    let (kind, purpose) = if mode == RestoreMode::ImportScenario {
        (ReviewKind::Import, OperationPurposeV1::ProjectImportPreview)
    } else {
        (
            ReviewKind::Restore,
            OperationPurposeV1::ProjectRestorePreview,
        )
    };
    let prepared = state
        .operations
        .prepare(
            "main",
            &OperationPrepareRequestV1 {
                request_id,
                schema_version: 1,
                purpose: purpose.clone(),
                context: OperationContextV1::Library {
                    expected_library_revision: None,
                },
            },
        )
        .map_err(boxed)?;
    let operation_id = prepared.operation_id;
    let app = state.app.clone();
    let custody = Arc::clone(&state.portable);
    state
        .operations
        .run(
            "main",
            library_claim(operation_id, request_id, purpose, None),
            None,
            OperationPhaseV1::Validating,
            (bytes, None),
            move |bytes, execution| async move {
                let cancellation = execution.cancellation();
                let reservation = custody.reserve(
                    "main",
                    Creator {
                        operation_id,
                        request_id,
                    },
                    kind,
                    cancellation.clone(),
                )?;
                let options = ImportOptions {
                    restore_mode: mode,
                    include_results: true,
                    include_assets: true,
                };
                let query = if mode == RestoreMode::ImportScenario {
                    AppQuery::PreviewImport {
                        bytes,
                        options,
                        cancellation,
                    }
                } else {
                    AppQuery::PreviewRestore {
                        bytes,
                        options,
                        cancellation,
                    }
                };
                let AppQueryResult::PortablePreview {
                    preview_id,
                    preview,
                } = app.query(query).await.map_err(map_app_error)?
                else {
                    return Err(prepared_output_error("protocol.result_mismatch").into());
                };
                let library_revision = preview.binding.local_library_revision;
                let binding = if kind == ReviewKind::Import {
                    ReviewBinding::Import { library_revision }
                } else {
                    ReviewBinding::Restore { library_revision }
                };
                reservation.publish_core(preview_id, binding)?;
                Ok(portable_preview(preview_id, *preview))
            },
        )
        .await
        .map_err(boxed)
}

fn prepare(window: &Window, purpose: &str, revision: Revision) -> TestResult<Value> {
    let response: ApiResponseDto<Value> = invoke_ok(
        window,
        "operation_prepare",
        &json!({
            "schemaVersion":1, "requestId":next()?, "purpose":{"kind":purpose},
            "context":{"kind":"library", "expectedLibraryRevision":revision}
        }),
    )?;
    Ok(response.result["operationId"].clone())
}

fn heavy(
    window: &Window,
    command: &str,
    request: Value,
) -> TestResult<Result<ApiResponseDto<Value>, Value>> {
    let mut arguments = json!({"onProgress":"__CHANNEL__:42"});
    arguments["request"] = request;
    match invoke_ipc_args(window, command, arguments)? {
        Ok(body) => Ok(Ok(body.deserialize()?)),
        Err(error) => Ok(Err(error)),
    }
}

fn import_request(
    window: &Window,
    preview: &PortablePreviewDto,
    collision_plan: &CollisionPlan,
) -> TestResult<Value> {
    Ok(json!({
        "schemaVersion":1, "requestId":next()?,
        "operationId":prepare(window, "projectImportApply", preview.library_revision)?,
        "previewId":preview.preview_id, "expectedLibraryRevision":preview.library_revision,
        "collisionPlan":collision_plan
    }))
}

#[test]
fn portable_v1_requests_reject_renderer_paths_missing_bindings_and_forged_restore_evidence()
-> TestResult {
    let id = next()?;
    let controls = json!({"schemaVersion":1, "requestId":id, "operationId":id});
    let mut input = controls.clone();
    input["options"] =
        json!({"restoreMode":"import-scenario", "includeResults":true, "includeAssets":true});
    for key in ["path", "sourceArtifact"] {
        let mut supplied_path = input.clone();
        supplied_path[key] = json!("/private/renderer-selected.eutheto");
        assert!(serde_json::from_value::<ImportPreviewRequest>(supplied_path).is_err());
    }
    let mut missing_control = input.clone();
    missing_control
        .as_object_mut()
        .ok_or("request was not an object")?
        .remove("operationId");
    assert!(serde_json::from_value::<ImportPreviewRequest>(missing_control).is_err());
    let mut restore = input;
    restore["options"]["restoreMode"] = json!("replace-library");
    assert!(serde_json::from_value::<RestorePreviewRequest>(restore.clone()).is_err());
    restore["origin"] = json!("/private/safety-backups");
    assert!(serde_json::from_value::<RestorePreviewRequest>(restore).is_err());

    let mut export = controls.clone();
    export["scenarioId"] = json!(id);
    export["previewId"] = json!(id);
    export["expectedRevision"] = json!(0);
    assert!(serde_json::from_value::<ExportCreateRequest>(export.clone()).is_err());
    export["expectedLibraryRevision"] = json!(0);
    export["fileName"] = json!("renderer-chosen.eutheto");
    assert!(serde_json::from_value::<ExportCreateRequest>(export).is_err());
    let mut backup = controls;
    backup["previewId"] = json!(id);
    backup["expectedLibraryRevision"] = json!(0);
    backup["title"] = json!("Changed after review");
    assert!(serde_json::from_value::<BackupCreateRequest>(backup).is_err());
    assert!(
        serde_json::from_value::<RestoreAuthorizationDto>(json!({
            "destructiveActionConfirmed":true, "safetyBackupBypassPhrase":null,
            "prospectiveFailureReceiptToken":id
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<DiscardRequest>(json!({
            "schemaVersion":1, "requestId":id, "previewId":id
        }))
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn versioned_apply_requires_admission_before_missing_preview_authority() -> TestResult {
    let directory = tempfile::tempdir()?;
    let deps = dependencies(directory.path(), "admission")?;
    let app = EuthetoApp::open(deps.clone()).await.map_err(boxed)?;
    let revision = app
        .application_settings_snapshot()
        .await
        .map_err(boxed)?
        .library_revision;
    let native = desktop(DesktopState::new(
        app,
        directory.path().join("cache"),
        deps.paths.safety_backups,
    ))?;
    let window =
        tauri::WebviewWindowBuilder::new(&native, "main", tauri::WebviewUrl::default()).build()?;
    for (command, purpose) in [
        ("project_import_apply", "projectImportApply"),
        ("project_restore_apply", "projectRestoreApply"),
    ] {
        let operation = prepare(&window, purpose, revision)?;
        let mut request = json!({
            "schemaVersion":1, "requestId":next()?, "operationId":operation,
            "previewId":next()?, "expectedLibraryRevision":revision,
            "collisionPlan":{"scenarios":{},"supplementalChoices":[]}
        });
        if command == "project_restore_apply" {
            request["authorization"] =
                json!({"destructiveActionConfirmed":true,"safetyBackupBypassPhrase":null});
        }
        let mut wrong_version = request.clone();
        wrong_version["schemaVersion"] = json!(2);
        assert_eq!(
            heavy(&window, command, wrong_version)?
                .err()
                .ok_or("unsupported version admitted")?["code"],
            "operation.version_unsupported"
        );
        let missing_channel =
            invoke_ipc_args(&window, command, json!({"request":request.clone()}))?
                .err()
                .ok_or("missing progress channel admitted")?;
        assert_eq!(
            missing_channel["code"],
            "operation.progress_channel_invalid"
        );
        assert_eq!(
            heavy(&window, command, request)?
                .err()
                .ok_or("missing review acquired")?["code"],
            "portable.preview_not_found"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn native_import_replaces_identity_not_title_and_persists_exact_content() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = EuthetoApp::open(dependencies(directory.path(), "source")?)
        .await
        .map_err(boxed)?;
    let title = "Portable identity collision";
    let scenario_id = create_project(&source, title).await?;
    let target_deps = dependencies(directory.path(), "target")?;
    let target = EuthetoApp::open(target_deps.clone()).await.map_err(boxed)?;
    let unrelated_id = create_project(&target, title).await?;
    let state = DesktopState::new(
        target,
        directory.path().join("cache"),
        target_deps.paths.safety_backups.clone(),
    );
    let native = desktop(state.clone())?;
    let window =
        tauri::WebviewWindowBuilder::new(&native, "main", tauri::WebviewUrl::default()).build()?;
    let selected_path = directory.path().join("selected.eutheto");
    std::fs::write(&selected_path, export(&source, scenario_id).await?)?;
    let initial = preview(
        &state,
        read_bounded_portable(&selected_path, &state.app.setup_cancellation()).map_err(boxed)?,
        RestoreMode::ImportScenario,
    )
    .await?;
    assert_eq!(
        initial
            .scenarios
            .iter()
            .map(|scenario| (scenario.scenario_id, scenario.collides))
            .collect::<Vec<_>>(),
        vec![(scenario_id, false)]
    );
    let request = import_request(&window, &initial, &CollisionPlan::default())?;
    let mut wrong_revision = request.clone();
    wrong_revision["expectedLibraryRevision"] = json!(initial.library_revision.value() + 1);
    assert_eq!(
        heavy(&window, "project_import_apply", wrong_revision)?
            .err()
            .ok_or("changed admission binding accepted")?["code"],
        "operation.claim_mismatch"
    );
    let applied = heavy(&window, "project_import_apply", request)?.map_err(boxed)?;
    assert_eq!(applied.result["schemaVersion"], 1);
    assert_eq!(applied.result["scenarioIds"], json!([scenario_id]));
    assert_eq!(applied.result["safetyBackup"]["kind"], "notRequired");

    let entity_id = EntityId::new(&SystemIdGenerator)?;
    let entity = json!({"id":entity_id.to_string(), "name":"Authoritative portable entity"});
    source
        .execute(AppCommand::ApplyScenario {
            request_id: next()?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: Some("desktop.portable.test".to_owned()),
                    display_name: "Portable test".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::AddEntity(AddEntity {
                    entity_id,
                    value: entity.clone(),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    std::fs::write(&selected_path, export(&source, scenario_id).await?)?;
    let replacement = preview(
        &state,
        read_bounded_portable(&selected_path, &state.app.setup_cancellation()).map_err(boxed)?,
        RestoreMode::ImportScenario,
    )
    .await?;
    let [collision] = replacement.scenarios.as_slice() else {
        return Err("wrong collision preview".into());
    };
    assert_eq!(collision.scenario_id, scenario_id);
    assert!(collision.collides);
    assert_eq!(collision.source_revision, Revision::new(1));
    assert_eq!(collision.same_identity_revision, Revision::new(1));
    let replaced = heavy(
        &window,
        "project_import_apply",
        import_request(
            &window,
            &replacement,
            &CollisionPlan {
                scenarios: BTreeMap::from([(scenario_id, CollisionAction::Replace)]),
                supplemental: BTreeMap::new(),
            },
        )?,
    )?
    .map_err(boxed)?;
    assert_eq!(replaced.result["scenarioIds"], json!([scenario_id]));
    assert_eq!(
        heavy(
            &window,
            "project_import_apply",
            import_request(&window, &replacement, &CollisionPlan::default())?
        )?
        .err()
        .ok_or("consumed review applied twice")?["code"],
        "portable.preview_not_found"
    );
    drop((window, native, state));

    let reopened = EuthetoApp::open(target_deps.clone()).await.map_err(boxed)?;
    let native = desktop(DesktopState::new(
        reopened,
        directory.path().join("reopened-cache"),
        target_deps.paths.safety_backups,
    ))?;
    let window =
        tauri::WebviewWindowBuilder::new(&native, "main", tauri::WebviewUrl::default()).build()?;
    let listed: ApiResponseDto<Vec<ProjectListItemV1>> = invoke_ok(
        &window,
        "project_list",
        &json!({"schemaVersion":1,"requestId":next()?,"scope":"active"}),
    )?;
    let mut actual_ids = listed
        .result
        .iter()
        .map(|project| project.scenario_id)
        .collect::<Vec<_>>();
    let mut expected_ids = vec![scenario_id, unrelated_id];
    actual_ids.sort();
    expected_ids.sort();
    assert_eq!(actual_ids, expected_ids);
    let (imported, snapshot) =
        native_summary_and_core_snapshot(&window, next()?, scenario_id).await?;
    assert_eq!(imported.result.title, title);
    assert_eq!(
        snapshot.document.domain.entities.get(&entity_id),
        Some(&entity)
    );
    let (unrelated, snapshot) =
        native_summary_and_core_snapshot(&window, next()?, unrelated_id).await?;
    assert_eq!(unrelated.result.title, title);
    assert_eq!(unrelated.result.revision, Revision::INITIAL);
    assert!(snapshot.document.domain.entities.is_empty());
    Ok(())
}

#[tokio::test]
async fn native_restore_reports_actual_retention_and_persists_confirmed_bypass() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = EuthetoApp::open(dependencies(directory.path(), "restore-source")?)
        .await
        .map_err(boxed)?;
    let restored_id = create_project(&source, "Restored project").await?;
    let deps = dependencies(directory.path(), "restore-target")?;
    let app = EuthetoApp::open(deps.clone()).await.map_err(boxed)?;
    let removed_id = create_project(&app, "Prior project").await?;
    let state = DesktopState::new(
        app,
        directory.path().join("cache"),
        deps.paths.safety_backups.clone(),
    );
    let native = desktop(state.clone())?;
    let window =
        tauri::WebviewWindowBuilder::new(&native, "main", tauri::WebviewUrl::default()).build()?;
    let review = preview(&state, backup(&source).await?, RestoreMode::ReplaceLibrary).await?;
    if deps.paths.safety_backups.is_dir() {
        std::fs::remove_dir_all(&deps.paths.safety_backups)?;
    }
    std::fs::write(&deps.paths.safety_backups, b"not a directory")?;
    let mut request = json!({
        "schemaVersion":1,"requestId":next()?,"operationId":prepare(&window,"projectRestoreApply",review.library_revision)?,
        "previewId":review.preview_id,"expectedLibraryRevision":review.library_revision,
        "collisionPlan":CollisionPlan::default(),
        "authorization":{"destructiveActionConfirmed":true,"safetyBackupBypassPhrase":null}
    });
    let failure = heavy(&window, "project_restore_apply", request.clone())?
        .err()
        .ok_or("blocked backup unexpectedly committed")?;
    assert_eq!(failure["code"], "restore.safety_backup_failed");
    assert_eq!(
        failure["details"]["portablePreviewRetained"],
        json!({"type":"boolean","value":true})
    );
    assert_eq!(
        state
            .app
            .application_settings_snapshot()
            .await
            .map_err(boxed)?
            .library_revision,
        review.library_revision
    );
    assert!(matches!(
        state
            .app
            .query(AppQuery::ProjectMetadata(removed_id))
            .await
            .map_err(boxed)?,
        AppQueryResult::Project(_)
    ));
    request["requestId"] = json!(next()?);
    request["operationId"] = prepare(&window, "projectRestoreApply", review.library_revision)?;
    request["authorization"]["safetyBackupBypassPhrase"] = json!("REPLACE WITHOUT BACKUP");
    let applied = heavy(&window, "project_restore_apply", request)?.map_err(boxed)?;
    assert_eq!(applied.result["safetyBackup"]["kind"], "confirmedBypass");
    assert_eq!(applied.result["scenarioIds"], json!([restored_id]));
    let committed = applied
        .current_revision
        .ok_or("restore omitted committed revision")?;
    assert!(committed > review.library_revision);
    drop((window, native, state));
    let reopened = EuthetoApp::open(deps).await.map_err(boxed)?;
    assert_eq!(
        reopened
            .application_settings_snapshot()
            .await
            .map_err(boxed)?
            .library_revision,
        committed
    );
    assert!(matches!(
        reopened.query(AppQuery::ProjectMetadata(removed_id)).await,
        Err(eutheto_types::AppError::NotFound(_))
    ));
    let AppQueryResult::Project(project) = reopened
        .query(AppQuery::ProjectMetadata(restored_id))
        .await
        .map_err(boxed)?
    else {
        return Err("wrong restored project receipt".into());
    };
    assert_eq!(project.title, "Restored project");
    Ok(())
}

async fn inspect_unopened(state: &DesktopState, bytes: Vec<u8>) -> TestResult<RequestId> {
    let request_id = next()?;
    let prepared = state
        .operations
        .prepare(
            "main",
            &OperationPrepareRequestV1 {
                request_id,
                schema_version: 1,
                purpose: OperationPurposeV1::ProjectUnopenedBundleInspect,
                context: OperationContextV1::Library {
                    expected_library_revision: None,
                },
            },
        )
        .map_err(boxed)?;
    let operation_id = prepared.operation_id;
    let app = state.app.clone();
    let custody = Arc::clone(&state.portable);
    state
        .operations
        .run(
            "main",
            library_claim(
                operation_id,
                request_id,
                OperationPurposeV1::ProjectUnopenedBundleInspect,
                None,
            ),
            None,
            OperationPhaseV1::Validating,
            (bytes, None),
            move |bytes, execution| async move {
                let cancellation = execution.cancellation();
                let reservation = custody.reserve(
                    "main",
                    Creator {
                        operation_id,
                        request_id,
                    },
                    ReviewKind::Unopened,
                    cancellation.clone(),
                )?;
                let AppQueryResult::UnopenedBundlePreview { preview_id, .. } = app
                    .query(AppQuery::InspectUnopenedBundle {
                        bytes,
                        cancellation,
                    })
                    .await
                    .map_err(map_app_error)?
                else {
                    return Err(prepared_output_error("protocol.result_mismatch").into());
                };
                reservation.publish_core(preview_id, ReviewBinding::Unopened)?;
                Ok(preview_id)
            },
        )
        .await
        .map_err(boxed)
}

async fn reexport_unopened(
    state: &DesktopState,
    preview_id: RequestId,
    destination: PathBuf,
) -> Result<(), ApiError> {
    let lease = state
        .portable
        .acquire("main", preview_id, ReviewBinding::Unopened)?;
    let result = state
        .app
        .execute(AppCommand::ExactReexportUnopenedBundle {
            preview_id,
            destination,
            cancellation: state.app.setup_cancellation(),
        })
        .await
        .map_err(map_app_error)?;
    if !matches!(result, AppCommandResult::UnopenedBundleReexported) {
        return Err(prepared_output_error("protocol.result_mismatch").into());
    }
    drop(lease);
    Ok(())
}

#[tokio::test]
async fn unopened_core_and_custody_preserve_exact_bytes_no_clobber_and_consumption() -> TestResult {
    let directory = tempfile::tempdir()?;
    let deps = dependencies(directory.path(), "unopened")?;
    let app = EuthetoApp::open(deps.clone()).await.map_err(boxed)?;
    let original = backup(&app).await?;
    let state = DesktopState::new(
        app,
        directory.path().join("cache"),
        deps.paths.safety_backups,
    );
    let native = desktop(state.clone())?;
    let window =
        tauri::WebviewWindowBuilder::new(&native, "main", tauri::WebviewUrl::default()).build()?;
    let preview = inspect_unopened(&state, original.clone()).await?;
    let destination = directory.path().join("exact.eutheto");
    reexport_unopened(&state, preview, destination.clone())
        .await
        .map_err(boxed)?;
    assert_eq!(std::fs::read(destination)?, original);
    let second = directory.path().join("second.eutheto");
    assert_eq!(
        reexport_unopened(&state, preview, second.clone())
            .await
            .err()
            .ok_or("review reused")?
            .code,
        "portable.preview_not_found"
    );
    assert!(!second.exists());

    let preview = inspect_unopened(&state, original.clone()).await?;
    let occupied = directory.path().join("occupied.eutheto");
    std::fs::write(&occupied, b"sentinel")?;
    assert_eq!(
        reexport_unopened(&state, preview, occupied.clone())
            .await
            .err()
            .ok_or("existing artifact clobbered")?
            .category,
        eutheto_types::ApiErrorCategoryDto::Storage
    );
    assert_eq!(std::fs::read(occupied)?, b"sentinel");
    assert_eq!(
        reexport_unopened(&state, preview, second)
            .await
            .err()
            .ok_or("failed publication retained review")?
            .code,
        "portable.preview_not_found"
    );

    let preview_id = inspect_unopened(&state, original).await?;
    let cancelled: ApiResponseDto<Value> = invoke_ok(
        &window,
        "project_operation_cancel",
        &json!({
            "schemaVersion":1,"requestId":next()?,"target":{"kind":"preview","previewId":preview_id}
        }),
    )?;
    assert_eq!(cancelled.current_revision, None);
    assert!(
        state
            .portable
            .acquire("main", preview_id, ReviewBinding::Unopened)
            .is_err()
    );
    let malformed = state
        .app
        .query(AppQuery::InspectUnopenedBundle {
            bytes: b"not a zip".to_vec(),
            cancellation: state.app.setup_cancellation(),
        })
        .await
        .err()
        .ok_or("malformed archive accepted")?;
    let error = map_app_error(malformed);
    assert_eq!(error.code, "portable.content_invalid");
    assert_eq!(
        error.category,
        eutheto_types::ApiErrorCategoryDto::Validation
    );
    Ok(())
}
