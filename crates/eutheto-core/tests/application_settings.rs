use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult, EuthetoApp,
};
use eutheto_store::{OpenOptions, SqliteScenarioStore};
use eutheto_types::{
    AppError, CancellationToken, EventPayload, EventTopic, FixedClock, FixedMonotonicClock,
    RequestId, SettingsImportApplyRequestV1, SettingsImportPreviewDtoV1, SystemIdGenerator,
};
use serde_json::{Value, json};
use std::{error::Error, sync::Arc};

const UPDATED: &str = "2026-09-01T00:00:00Z";
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

async fn fixture(
    options: OpenOptions,
) -> TestResult<(tempfile::TempDir, EuthetoApp, Arc<SqliteScenarioStore>)> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-settings-")
        .tempdir_in(dirs::home_dir().ok_or("home missing")?)?;
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("state.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new(UPDATED.parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let (store, initialization) =
        SqliteScenarioStore::open_with_options(&dependencies.paths.database, options).await?;
    let store = Arc::new(store);
    let app = EuthetoApp::from_initialized_store(Arc::clone(&store), initialization, dependencies)
        .map_err(boxed)?;
    Ok((directory, app, store))
}

fn document(settings: &Value) -> TestResult<Vec<u8>> {
    Ok(serde_json::to_vec(&json!({
        "format": "eutheto/application-settings", "schemaVersion": 1, "settings": settings
    }))?)
}

fn approval(preview: &SettingsImportPreviewDtoV1) -> TestResult<SettingsImportApplyRequestV1> {
    Ok(SettingsImportApplyRequestV1 {
        schema_version: 1,
        request_id: RequestId::new(&SystemIdGenerator)?,
        preview_id: preview.preview_id,
        approval_sha256: preview.approval_sha256.clone(),
        expected_library_revision: preview.library_revision,
    })
}

async fn preview(app: &EuthetoApp, bytes: Vec<u8>) -> TestResult<SettingsImportPreviewDtoV1> {
    app.preview_nonsecret_settings(bytes, app.setup_cancellation())
        .await
        .map_err(boxed)
}

#[tokio::test]
async fn ordinary_settings_conflicts_preserve_writes_and_absent_reset_is_silent() -> TestResult {
    let (_directory, app, _store) = fixture(OpenOptions::default()).await?;
    let before = app.application_settings_snapshot().await.map_err(boxed)?;
    let mut events = app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;
    let AppCommandResult::SettingsWritten(written) = app
        .execute(AppCommand::SetSetting {
            request_id: RequestId::new(&SystemIdGenerator)?,
            expected_library_revision: before.library_revision,
            key: "locale".to_owned(),
            value: json!("en-GB"),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("unexpected settings result".into());
    };
    assert!(written.changed);
    assert!(events.try_recv().map_err(boxed)?.is_some());
    for command in [
        AppCommand::SetSetting {
            request_id: RequestId::new(&SystemIdGenerator)?,
            expected_library_revision: before.library_revision,
            key: "locale".to_owned(),
            value: json!("fr"),
        },
        AppCommand::DeleteSetting {
            request_id: RequestId::new(&SystemIdGenerator)?,
            expected_library_revision: before.library_revision,
            key: "locale".to_owned(),
        },
    ] {
        assert!(matches!(
            app.execute(command).await,
            Err(AppError::Conflict { expected_revision, actual_revision })
                if expected_revision == before.library_revision
                    && actual_revision == written.library_revision
        ));
    }
    let AppCommandResult::SettingsWritten(reset) = app
        .execute(AppCommand::DeleteSetting {
            request_id: RequestId::new(&SystemIdGenerator)?,
            expected_library_revision: written.library_revision,
            key: "appearance".to_owned(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("unexpected settings result".into());
    };
    assert!(!reset.changed);
    assert_eq!(reset.library_revision, written.library_revision);
    assert_eq!(
        reset.settings.locale.ok_or("locale was removed")?.value,
        json!("en-GB")
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());
    let AppCommandResult::SettingsWritten(deleted) = app
        .execute(AppCommand::DeleteSetting {
            request_id: RequestId::new(&SystemIdGenerator)?,
            expected_library_revision: reset.library_revision,
            key: "locale".to_owned(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("unexpected settings result".into());
    };
    assert!(deleted.changed);
    assert!(deleted.settings.locale.is_none());
    let reopened = app.application_settings_snapshot().await.map_err(boxed)?;
    assert_eq!(reopened.library_revision, deleted.library_revision);
    assert!(reopened.settings.locale.is_none());
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn approval_is_one_use_and_notifications_follow_complete_entry_changes() -> TestResult {
    let (_directory, app, store) = fixture(OpenOptions::default()).await?;
    let bytes = document(&json!({"locale": {"value": "en-GB", "updatedAt": UPDATED}}))?;
    let reviewed = preview(&app, bytes.clone()).await?;
    assert_eq!(reviewed.changes[0].key, "locale");
    assert!(reviewed.changes[0].before.is_none());
    assert_eq!(reviewed.source_sha256, eutheto_export::sha256_hex(&bytes));
    let mut events = app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;
    let request = approval(&reviewed)?;
    let mut wrong_digest = request.clone();
    wrong_digest.approval_sha256 = "0".repeat(64);
    assert!(
        app.apply_nonsecret_settings(wrong_digest, app.setup_cancellation())
            .await
            .is_err()
    );
    let mut wrong_revision = request.clone();
    wrong_revision.expected_library_revision = eutheto_types::Revision::new(1);
    assert!(matches!(
        app.apply_nonsecret_settings(wrong_revision, app.setup_cancellation())
            .await,
        Err(AppError::Conflict { .. })
    ));
    assert!(
        app.execute(AppCommand::CancelPortablePreview {
            preview_id: reviewed.preview_id
        })
        .await
        .is_err()
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());
    let applied = app
        .apply_nonsecret_settings(request.clone(), app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert!(applied.changed);
    assert_eq!(applied.library_revision.value(), 1);
    assert!(
        matches!(events.try_recv().map_err(boxed)?.map(|event| event.payload),
        Some(EventPayload::AppNotification { context, code, .. })
        if context.request_id == Some(request.request_id) && code == "settings.updated")
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());
    assert!(
        app.apply_nonsecret_settings(request, app.setup_cancellation())
            .await
            .is_err()
    );

    let export = app
        .export_nonsecret_settings(app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert_eq!(export.library_revision, applied.library_revision);
    let unchanged = preview(&app, serde_json::to_vec(&export.document)?).await?;
    assert!(unchanged.changes.is_empty());
    let unchanged = app
        .apply_nonsecret_settings(approval(&unchanged)?, app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert!(!unchanged.changed);
    assert_eq!(unchanged.library_revision, applied.library_revision);
    assert!(events.try_recv().map_err(boxed)?.is_none());

    let timestamp_only = preview(
        &app,
        document(&json!({"locale": {
            "value": "en-GB", "updatedAt": "2026-09-02T00:00:00Z"
        }}))?,
    )
    .await?;
    assert_eq!(
        timestamp_only.changes[0]
            .before
            .as_ref()
            .ok_or("before missing")?
            .value,
        timestamp_only.changes[0]
            .after
            .as_ref()
            .ok_or("after missing")?
            .value
    );
    let timestamp_only = app
        .apply_nonsecret_settings(approval(&timestamp_only)?, app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert!(timestamp_only.changed);
    assert_eq!(timestamp_only.library_revision.value(), 2);
    assert!(events.try_recv().map_err(boxed)?.is_some());
    assert!(events.try_recv().map_err(boxed)?.is_none());

    let clear = preview(&app, document(&json!({}))?).await?;
    assert!(clear.changes[0].after.is_none());
    let clear = app
        .apply_nonsecret_settings(approval(&clear)?, app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert!(clear.changed);
    assert_eq!(clear.library_revision.value(), 3);
    assert!(
        store
            .get_setting::<Value>("locale".to_owned())
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn library_change_rejects_and_consumes_owned_approval_without_notification() -> TestResult {
    let (_directory, app, store) = fixture(OpenOptions::default()).await?;
    let reviewed = preview(
        &app,
        document(&json!({"units": {"value": "metric", "updatedAt": UPDATED}}))?,
    )
    .await?;
    store
        .replace_settings(
            &["excluded-device-state"],
            store.settings_snapshot(&["excluded-device-state"]).await?,
            std::collections::BTreeMap::from([(
                "excluded-device-state".to_owned(),
                eutheto_store::AppSetting {
                    value: json!(true),
                    updated_at: UPDATED.parse()?,
                },
            )]),
            CancellationToken::new(),
        )
        .await?;
    let mut events = app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;
    let request = approval(&reviewed)?;
    assert!(matches!(
        app.apply_nonsecret_settings(request.clone(), app.setup_cancellation())
            .await,
        Err(AppError::Conflict { .. })
    ));
    assert!(
        app.apply_nonsecret_settings(request, app.setup_cancellation())
            .await
            .is_err()
    );
    assert!(
        store
            .get_setting::<Value>("units".to_owned())
            .await?
            .is_none()
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());
    Ok(())
}

#[tokio::test]
async fn strict_source_rejection_never_changes_settings_or_publishes_events() -> TestResult {
    let (_directory, app, store) = fixture(OpenOptions::default()).await?;
    let initial = store.library_metadata_snapshot().await?.revision;
    let mut events = app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;
    let good = document(&json!({"locale": {"value": "en", "updatedAt": UPDATED}}))?;
    let mut newer: Value = serde_json::from_slice(&good)?;
    newer["schemaVersion"] = json!(2);
    let mut unknown: Value = serde_json::from_slice(&good)?;
    unknown["secrets"] = json!({});
    let mut invalid_time: Value = serde_json::from_slice(&good)?;
    invalid_time["settings"]["locale"]["updatedAt"] = json!("not a timestamp");
    let duplicate = format!(
        "{{\"format\":\"eutheto/application-settings\",\"schemaVersion\":1,\"settings\":{{\"locale\":{{\"value\":\"en\",\"value\":\"fr\",\"updatedAt\":\"{UPDATED}\"}}}}}}"
    );
    for bytes in [
        serde_json::to_vec(&newer)?,
        serde_json::to_vec(&unknown)?,
        serde_json::to_vec(&invalid_time)?,
        duplicate.into_bytes(),
        document(&json!({"unrecognized": {"value": true, "updatedAt": UPDATED}}))?,
        document(&json!({"appearance": {"value": {"theme": "invisible"}, "updatedAt": UPDATED}}))?,
        vec![b' '; eutheto_types::MAX_APPLICATION_SETTINGS_BYTES + 1],
    ] {
        assert!(
            app.preview_nonsecret_settings(bytes, app.setup_cancellation())
                .await
                .is_err()
        );
        assert_eq!(store.library_metadata_snapshot().await?.revision, initial);
        assert!(events.try_recv().map_err(boxed)?.is_none());
    }
    assert!(
        store
            .get_setting::<Value>("locale".to_owned())
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn unportable_local_setting_blocks_export_but_can_be_replaced_by_review() -> TestResult {
    let (_directory, app, _store) = fixture(OpenOptions::default()).await?;
    app.execute(AppCommand::SetSetting {
        request_id: RequestId::new(&SystemIdGenerator)?,
        expected_library_revision: app
            .application_settings_snapshot()
            .await
            .map_err(boxed)?
            .library_revision,
        key: "locale".to_owned(),
        value: json!("con"),
    })
    .await
    .map_err(boxed)?;
    assert!(
        app.export_nonsecret_settings(app.setup_cancellation())
            .await
            .is_err()
    );
    assert!(
        matches!(app.query(AppQuery::Setting("locale".to_owned())).await.map_err(boxed)?,
        AppQueryResult::Setting(Some(setting)) if setting.value == json!("con"))
    );
    let reviewed = preview(
        &app,
        document(&json!({"locale": {"value": "en", "updatedAt": UPDATED}}))?,
    )
    .await?;
    assert_eq!(
        reviewed.changes[0]
            .before
            .as_ref()
            .ok_or("before missing")?
            .value,
        json!("con")
    );
    app.apply_nonsecret_settings(approval(&reviewed)?, app.setup_cancellation())
        .await
        .map_err(boxed)?;
    let exported = app
        .export_nonsecret_settings(app.setup_cancellation())
        .await
        .map_err(boxed)?;
    let exported = serde_json::to_value(exported.document)?;
    assert_eq!(exported["settings"]["locale"]["value"], json!("en"));
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn abandoned_caller_cannot_lose_committed_settings_notification() -> TestResult {
    use eutheto_store::{CommandCommitTestHook, CommandCommitTestPhase};
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::AfterFinalCancellationCheck);
    let (_directory, app, store) =
        fixture(OpenOptions::default().with_command_commit_test_hook(hook.clone())).await?;
    let reviewed = preview(
        &app,
        document(&json!({"units": {"value": "metric", "updatedAt": UPDATED}}))?,
    )
    .await?;
    let request = approval(&reviewed)?;
    let request_id = request.request_id;
    let mut events = app
        .subscribe(EventTopic::AppNotification)
        .await
        .map_err(boxed)?;
    let caller_app = app.clone();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_nonsecret_settings(request, caller_app.setup_cancellation())
            .await
    });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    caller.abort();
    assert!(caller.await.is_err_and(|error| error.is_cancelled()));
    hook.release();
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await?
        .map_err(boxed)?;
    assert!(
        matches!(event.payload, EventPayload::AppNotification { context, code, .. }
        if context.request_id == Some(request_id) && code == "settings.updated")
    );
    assert!(events.try_recv().map_err(boxed)?.is_none());
    assert_eq!(
        store
            .get_setting::<Value>("units".to_owned())
            .await?
            .ok_or("setting missing")?
            .value,
        json!("metric")
    );
    assert_eq!(store.library_metadata_snapshot().await?.revision.value(), 1);
    Ok(())
}
