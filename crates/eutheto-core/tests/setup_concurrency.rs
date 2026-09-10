//! Pauses real Workforce settings authority to exercise races through the public core service.
#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod fixture;
#[path = "support/setup_control.rs"]
mod setup_control;

use setup_control::{ControlledPack, PauseAt, PreparationGate, ReleaseOnDrop};

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, EuthetoApp, SetupSourceV2,
    WorkforceGenerationApplyRequestV1,
};
use eutheto_domain_api::{DomainPack, DomainPackRegistry, DomainValidationReport};
use eutheto_store::{NewProject, OpenOptions, SqliteScenarioStore};
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandEnvelope, CommandId, CommandSource, EventTopic,
    FixedClock, FixedMonotonicClock, OperationControl, OperationId, OverlapPolicy, RequestId,
    Revision, ScenarioCommand, ScenarioDocument, ScenarioSettings, SetScenarioSettings,
    SystemIdGenerator,
};
use eutheto_workforce::{WorkforcePack, temporal::preview_generation};
use std::{
    error::Error,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
fn boxed(value: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{value:?}")).into()
}
fn settings(value: ScenarioSettings) -> ScenarioCommand {
    ScenarioCommand::SetScenarioSettings(Box::new(SetScenarioSettings {
        settings: value,
        restoration: None,
    }))
}

async fn commit_settings(
    app: &EuthetoApp,
    scenario_id: eutheto_types::ScenarioId,
    expected_revision: Revision,
    value: ScenarioSettings,
    request_id: RequestId,
) -> TestResult<AppCommandResult> {
    app.execute(AppCommand::ApplyScenario {
        request_id,
        envelope: CommandEnvelope {
            command_id: CommandId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision,
            actor: ActorRef {
                actor_id: None,
                display_name: "Competing settings change".to_owned(),
            },
            source: CommandSource::System,
            command: settings(value),
        },
        truncate_redo: false,
    })
    .await
    .map_err(boxed)
}

async fn read_summary(
    app: &EuthetoApp,
    scenario_id: eutheto_types::ScenarioId,
) -> TestResult<eutheto_core::ScenarioSummaryV2> {
    app.setup_summary(scenario_id, None, app.setup_cancellation())
        .await
        .map_err(boxed)
}

async fn fixture_app(
    options: OpenOptions,
) -> TestResult<(
    tempfile::TempDir,
    EuthetoApp,
    Arc<SqliteScenarioStore>,
    ScenarioDocument,
    Arc<PreparationGate>,
)> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-setup-race-")
        .tempdir_in(dirs::home_dir().ok_or("home missing")?)?;
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("state.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new("2026-09-01T00:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let (store, initialization) =
        SqliteScenarioStore::open_with_options(&dependencies.paths.database, options).await?;
    let store = Arc::new(store);
    let mut original = fixture::fixture()?;
    original.settings.overlap_policy = OverlapPolicy::Earlier;
    store
        .create_project(NewProject {
            document: original.clone(),
        })
        .await?;
    let gate = Arc::new(PreparationGate::default());
    let registry = DomainPackRegistry::builder()
        .register(ControlledPack {
            pack: WorkforcePack,
            gate: Arc::clone(&gate),
        })
        .build()?;
    let app = EuthetoApp::from_initialized_store_with_pack_registry(
        Arc::clone(&store),
        initialization,
        dependencies,
        registry,
    )
    .map_err(boxed)?;
    Ok((directory, app, store, original, gate))
}

fn request(original: &ScenarioDocument) -> TestResult<WorkforceGenerationApplyRequestV1> {
    let mut target = original.settings.clone();
    target.horizon.end = "2026-11-09T05:00:00Z".parse()?;
    let mut prospective = original.clone();
    prospective.domain = WorkforcePack
        .reconcile_settings(
            original,
            &target,
            None,
            &OperationControl::Cancellation(CancellationToken::new()),
        )?
        .domain;
    prospective.settings = target.clone();
    let hash =
        preview_generation(original, &prospective, &CancellationToken::new())?.prospective_hash;
    Ok(WorkforceGenerationApplyRequestV1 {
        request_id: RequestId::new(&SystemIdGenerator)?,
        schema_version: 1,
        operation_id: OperationId::new(&SystemIdGenerator)?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        scenario_id: original.scenario_id,
        expected_revision: Revision::INITIAL,
        actor: ActorRef {
            actor_id: None,
            display_name: "Reviewed generation".to_owned(),
        },
        source: SetupSourceV2::CommandPreview {
            command: settings(target),
        },
        prospective_hash: hash,
        truncate_redo: false,
    })
}

#[tokio::test]
async fn competing_command_commits_while_generation_preparation_is_paused() -> TestResult {
    let (_directory, app, store, original, gate) = fixture_app(OpenOptions::default()).await?;
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    let mut events = app
        .subscribe(EventTopic::ScenarioChanged)
        .await
        .map_err(boxed)?;
    let request = request(&original)?;
    let preparation_id = request.request_id;
    gate.armed.store(true, Ordering::SeqCst);
    let owner = app.clone();
    let pending = tokio::spawn(async move {
        owner
            .apply_reviewed_generation(request, owner.setup_cancellation())
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await?;
    let mut competing = original.settings.clone();
    competing.locale = "fr-CA".parse()?;
    let competing_id = RequestId::new(&SystemIdGenerator)?;
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        commit_settings(
            &app,
            original.scenario_id,
            Revision::INITIAL,
            competing.clone(),
            competing_id,
        ),
    )
    .await??;
    assert!(
        matches!(outcome, AppCommandResult::ScenarioCommand(result) if result.new_revision == Revision::new(1))
    );
    gate.release();
    assert_eq!(
        pending.await?,
        Err(AppError::Conflict {
            expected_revision: Revision::INITIAL,
            actual_revision: Revision::new(1)
        })
    );
    let persisted = store.get_project(original.scenario_id).await?;
    assert_eq!(persisted.document.settings, competing);
    assert_eq!(persisted.document.domain, original.domain);
    let event = events
        .try_recv()
        .map_err(boxed)?
        .ok_or("missing committed event")?;
    let eutheto_types::EventPayload::ScenarioChanged { context, .. } = event.payload else {
        return Err("unexpected committed event kind".into());
    };
    assert_eq!(context.request_id, Some(competing_id));
    assert_ne!(context.request_id, Some(preparation_id));
    assert!(events.try_recv().map_err(boxed)?.is_none());
    Ok(())
}

#[tokio::test]
async fn cancelled_generation_preparation_has_no_state_or_success_event() -> TestResult {
    let (_directory, app, store, original, gate) = fixture_app(OpenOptions::default()).await?;
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    let mut events = app
        .subscribe(EventTopic::ScenarioChanged)
        .await
        .map_err(boxed)?;
    let request = request(&original)?;
    let cancellation = app.setup_cancellation();
    let signal = cancellation.clone();
    gate.armed.store(true, Ordering::SeqCst);
    let owner = app.clone();
    let pending =
        tokio::spawn(async move { owner.apply_reviewed_generation(request, cancellation).await });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await?;
    signal.cancel();
    gate.release();
    assert!(
        matches!(pending.await?, Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled")
    );
    let persisted = store.get_project(original.scenario_id).await?;
    assert_eq!(persisted.document, original);
    assert_eq!(persisted.summary.revision, Revision::INITIAL);
    assert!(events.try_recv().map_err(boxed)?.is_none());
    Ok(())
}

#[tokio::test]
async fn same_revision_source_replacement_cannot_publish_a_prepared_command() -> TestResult {
    let (directory, app, store, original, gate) = fixture_app(OpenOptions::default()).await?;
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    let mut events = app
        .subscribe(EventTopic::ScenarioChanged)
        .await
        .map_err(boxed)?;
    let request = request(&original)?;
    gate.armed.store(true, Ordering::SeqCst);
    let owner = app.clone();
    let pending = tokio::spawn(async move {
        owner
            .apply_reviewed_generation(request, owner.setup_cancellation())
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await?;
    let mut replacement = original.clone();
    replacement.extensions.insert(
        "nonsemantic.replacement".to_owned(),
        serde_json::json!({"note":"replacement source"}),
    );
    // Simulate an out-of-band same-revision source replacement. Normal typed commands advance
    // revisions; the prepared publication must additionally defend its exact captured authority.
    let connection = rusqlite::Connection::open(directory.path().join("state.sqlite"))?;
    assert_eq!(
        connection.execute(
            "UPDATE scenarios SET document_json = ?1 WHERE id = ?2",
            rusqlite::params![
                serde_json::to_string(&replacement)?,
                original.scenario_id.to_string()
            ],
        )?,
        1
    );
    drop(connection);
    gate.release();
    assert!(
        matches!(pending.await?, Err(AppError::Validation(report)) if report.issues.iter().any(
        |issue| issue.code == "scenario.setup_source_changed"))
    );
    let current = store.get_project(original.scenario_id).await?;
    assert_eq!(current.summary.revision, Revision::INITIAL);
    assert_eq!(current.document, replacement);
    let eutheto_core::AppQueryResult::History(history) = app
        .query(eutheto_core::AppQuery::History(original.scenario_id))
        .await
        .map_err(boxed)?
    else {
        return Err("wrong history response".into());
    };
    assert!(history.is_empty());
    assert!(events.try_recv().map_err(boxed)?.is_none());
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn generation_cancellation_after_final_check_preserves_receipt_and_event() -> TestResult {
    use eutheto_store::{CommandCommitTestHook, CommandCommitTestPhase};
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::AfterFinalCancellationCheck);
    let (_directory, app, store, original, _gate) =
        fixture_app(OpenOptions::default().with_command_commit_test_hook(hook.clone())).await?;
    let mut events = app
        .subscribe(EventTopic::ScenarioChanged)
        .await
        .map_err(boxed)?;
    let request = request(&original)?;
    let request_id = request.request_id;
    let cancellation = app.setup_cancellation();
    let signal = cancellation.clone();
    let owner = app.clone();
    let pending =
        tokio::spawn(async move { owner.apply_reviewed_generation(request, cancellation).await });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    signal.cancel();
    tokio::task::spawn_blocking(move || hook.release()).await?;
    let receipt = tokio::time::timeout(Duration::from_secs(10), pending)
        .await??
        .map_err(boxed)?;
    assert_eq!(receipt.new_revision, Revision::new(1));
    let persisted = store.get_project(original.scenario_id).await?;
    assert_eq!(
        eutheto_workforce::temporal::resolve_shifts(
            &persisted.document,
            &CancellationToken::new()
        )?
        .len(),
        3
    );
    assert_eq!(persisted.summary.revision, receipt.new_revision);
    let event = events
        .try_recv()
        .map_err(boxed)?
        .ok_or("missing committed event")?;
    let eutheto_types::EventPayload::ScenarioChanged { context, .. } = event.payload else {
        return Err("unexpected event kind".into());
    };
    assert_eq!(context.request_id, Some(request_id));
    assert!(events.try_recv().map_err(boxed)?.is_none());
    Ok(())
}

#[tokio::test]
async fn full_validation_releases_capture_lock_and_newer_attempt_owns_readiness() -> TestResult {
    use eutheto_core::FullValidationStateV1;
    let (_directory, app, store, original, gate) = fixture_app(OpenOptions::default()).await?;
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    let initial = read_summary(&app, original.scenario_id).await?;
    assert_eq!(initial.fast.counts.errors, 0);
    assert_eq!(initial.full, FullValidationStateV1::NotRun);
    gate.pause_at
        .store(PauseAt::FullValidation as u8, Ordering::SeqCst);
    gate.armed.store(true, Ordering::SeqCst);
    let operation_id = OperationId::new(&SystemIdGenerator)?;
    let scenario_id = original.scenario_id;
    let owner = app.clone();
    let pending = tokio::spawn(async move {
        owner
            .full_validate_setup(
                scenario_id,
                Revision::INITIAL,
                operation_id,
                owner.setup_cancellation(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await?;
    let running = read_summary(&app, scenario_id).await?;
    assert_eq!(
        running.full,
        FullValidationStateV1::Running {
            operation_id,
            input_revision: Revision::INITIAL,
            stale: false
        }
    );
    let mut target = original.settings.clone();
    target.locale = "fr-CA".parse()?;
    let changed = tokio::time::timeout(
        Duration::from_secs(10),
        commit_settings(
            &app,
            scenario_id,
            Revision::INITIAL,
            target,
            RequestId::new(&SystemIdGenerator)?,
        ),
    )
    .await??;
    assert!(
        matches!(changed, AppCommandResult::ScenarioCommand(result) if result.new_revision == Revision::new(1))
    );
    let stale = read_summary(&app, scenario_id).await?;
    assert_eq!(
        stale.full,
        FullValidationStateV1::Running {
            operation_id,
            input_revision: Revision::INITIAL,
            stale: true
        }
    );
    // Reusing a public operation ID cannot defeat the internal attempt-order guard.
    let newer = app
        .full_validate_setup(
            scenario_id,
            Revision::new(1),
            operation_id,
            app.setup_cancellation(),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(newer.revision, Revision::new(1));
    gate.release();
    let older = pending.await?.map_err(boxed)?;
    assert_eq!(older.revision, Revision::INITIAL);
    let current = read_summary(&app, scenario_id).await?;
    assert!(matches!(current.full, FullValidationStateV1::Completed {
        operation_id: completed, input_revision, stale:false, ..
    } if completed == operation_id && input_revision == Revision::new(1)));
    assert_eq!(
        store.get_project(scenario_id).await?.summary.revision,
        Revision::new(1)
    );
    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::new(1),
    })
    .await
    .map_err(boxed)?;
    let after_undo = read_summary(&app, scenario_id).await?;
    assert_eq!(after_undo.revision, Revision::new(2));
    assert!(
        matches!(after_undo.full, FullValidationStateV1::Completed {input_revision,stale:true,..}
        if input_revision == Revision::new(1))
    );
    assert_eq!(store.get_project(scenario_id).await?.document, original);
    Ok(())
}

#[tokio::test]
async fn full_validation_cancellation_is_terminal_only_after_work_observes_it() -> TestResult {
    use eutheto_core::FullValidationStateV1;
    let (_directory, app, store, original, gate) = fixture_app(OpenOptions::default()).await?;
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    gate.pause_at
        .store(PauseAt::FullValidation as u8, Ordering::SeqCst);
    gate.armed.store(true, Ordering::SeqCst);
    let operation_id = OperationId::new(&SystemIdGenerator)?;
    let scenario_id = original.scenario_id;
    let cancellation = app.setup_cancellation();
    let signal = cancellation.clone();
    let sibling = app.setup_cancellation();
    let owner = app.clone();
    let pending = tokio::spawn(async move {
        owner
            .full_validate_setup(scenario_id, Revision::INITIAL, operation_id, cancellation)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await?;
    signal.cancel();
    let requested = read_summary(&app, scenario_id).await?;
    assert!(matches!(
        requested.full,
        FullValidationStateV1::Running { .. }
    ));
    gate.release();
    assert!(
        matches!(pending.await?, Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled")
    );
    let cancelled = read_summary(&app, scenario_id).await?;
    assert_eq!(
        cancelled.full,
        FullValidationStateV1::Cancelled {
            operation_id,
            input_revision: Revision::INITIAL,
            stale: false
        }
    );
    app.full_validate_setup(
        scenario_id,
        Revision::INITIAL,
        OperationId::new(&SystemIdGenerator)?,
        sibling,
    )
    .await
    .map_err(boxed)?;
    assert_eq!(store.get_project(scenario_id).await?.document, original);
    assert!(store.history(scenario_id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn abandoned_full_validation_cannot_leave_running_readiness() -> TestResult {
    use eutheto_core::FullValidationStateV1;
    let (_directory, app, _store, original, gate) = fixture_app(OpenOptions::default()).await?;
    let _release = ReleaseOnDrop(Arc::clone(&gate));
    gate.pause_at
        .store(PauseAt::FullValidation as u8, Ordering::SeqCst);
    gate.armed.store(true, Ordering::SeqCst);
    let operation_id = OperationId::new(&SystemIdGenerator)?;
    let scenario_id = original.scenario_id;
    let owner = app.clone();
    let pending = tokio::spawn(async move {
        owner
            .full_validate_setup(
                scenario_id,
                Revision::INITIAL,
                operation_id,
                owner.setup_cancellation(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await?;
    pending.abort();
    assert!(pending.await.is_err_and(|error| error.is_cancelled()));
    let cancelled = read_summary(&app, scenario_id).await?;
    assert_eq!(
        cancelled.full,
        FullValidationStateV1::Cancelled {
            operation_id,
            input_revision: Revision::INITIAL,
            stale: false
        }
    );
    gate.release();
    Ok(())
}

#[tokio::test]
async fn full_validation_resource_failure_never_becomes_partial_completed_readiness() -> TestResult
{
    use eutheto_core::FullValidationStateV1;
    // Exercise both independent ceilings: too many small findings, and a byte-heavy report
    // whose issue count is otherwise well within the allowed range.
    for (count, message) in [
        (100_001, "Finding".to_owned()),
        (17, "x".repeat(1024 * 1024)),
    ] {
        let (_directory, app, store, original, gate) = fixture_app(OpenOptions::default()).await?;
        *gate.full_report.lock().map_err(boxed)? = Some(DomainValidationReport {
            issues: vec![
                eutheto_types::ValidationIssue {
                    code: "fixture.amplified_report".to_owned(),
                    severity: eutheto_types::ValidationSeverity::Info,
                    message,
                    field_path: None,
                    resource: None,
                };
                count
            ],
        });
        let operation_id = OperationId::new(&SystemIdGenerator)?;
        let result = app
            .full_validate_setup(
                original.scenario_id,
                Revision::INITIAL,
                operation_id,
                app.setup_cancellation(),
            )
            .await;
        assert!(
            matches!(result, Err(AppError::Protocol(failure)) if failure.code == "operation.resource_limit")
        );
        let summary = read_summary(&app, original.scenario_id).await?;
        assert!(matches!(summary.full, FullValidationStateV1::Failed {
            operation_id:failed, input_revision:Revision::INITIAL, stale:false, ..
        } if failed == operation_id));
        assert_eq!(
            store.get_project(original.scenario_id).await?.document,
            original
        );
        assert!(store.history(original.scenario_id).await?.is_empty());
    }
    Ok(())
}
