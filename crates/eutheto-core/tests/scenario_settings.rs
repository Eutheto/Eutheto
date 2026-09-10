#![forbid(unsafe_code)]

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_command::{AppliedCommand, CommandError, apply_command_with_registry};
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult, EuthetoApp,
};
use eutheto_domain_api::DomainPackRegistry;
use eutheto_store::{NewProject, SqliteScenarioStore};
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandBatch, CommandEnvelope, CommandId, CommandResult,
    CommandSource, DomainCommandEnvelope, FixedClock, FixedMonotonicClock, GapPolicy, Horizon,
    OverlapPolicy, RequestId, Revision, Rfc3339Timestamp, ScenarioCommand, ScenarioDocument,
    ScenarioId, ScenarioSettings, ScenarioViewDto, SetScenarioSettings, SystemIdGenerator,
};
use eutheto_workforce::{WorkforcePack, commands, temporal::resolve_shifts};
use serde_json::{Value, json};
use std::{error::Error, fmt::Debug, sync::Arc};
use tempfile::TempDir;
use workforce_fixture::id;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: impl Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

fn entity(document: &ScenarioDocument, index: u32) -> TestResult<&Value> {
    document
        .domain
        .entities
        .get(&id(index).parse()?)
        .ok_or_else(|| "missing fixture entity".into())
}

fn entity_mut(document: &mut ScenarioDocument, index: u32) -> TestResult<&mut Value> {
    document
        .domain
        .entities
        .get_mut(&id(index).parse()?)
        .ok_or_else(|| "missing fixture entity".into())
}

fn settings(settings: ScenarioSettings, restoration: Option<Value>) -> ScenarioCommand {
    ScenarioCommand::SetScenarioSettings(Box::new(SetScenarioSettings {
        settings,
        restoration,
    }))
}

fn envelope(
    document: &ScenarioDocument,
    revision: Revision,
    command: ScenarioCommand,
) -> TestResult<CommandEnvelope> {
    Ok(CommandEnvelope {
        command_id: CommandId::new(&SystemIdGenerator)?,
        scenario_id: document.scenario_id,
        expected_revision: revision,
        actor: ActorRef {
            actor_id: None,
            display_name: "Settings authority regression".to_owned(),
        },
        source: CommandSource::System,
        command,
    })
}

fn pure(
    document: &ScenarioDocument,
    command: ScenarioCommand,
) -> TestResult<Result<AppliedCommand, CommandError>> {
    let registry = DomainPackRegistry::builder()
        .register(WorkforcePack)
        .build()?;
    Ok(apply_command_with_registry(
        document,
        Revision::INITIAL,
        &envelope(document, Revision::INITIAL, command)?,
        &registry,
        &CancellationToken::new(),
    ))
}

fn domain(command_type: &str, payload: Value) -> ScenarioCommand {
    ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
        command_type: command_type.to_owned(),
        payload,
    })
}

fn batch(commands: Vec<ScenarioCommand>) -> ScenarioCommand {
    ScenarioCommand::ApplyBatch(CommandBatch {
        label: Some("Settings and workforce".to_owned()),
        commands,
    })
}

async fn stored(document: &ScenarioDocument) -> TestResult<(TempDir, AppDependencies, EuthetoApp)> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-scenario-settings-")
        .tempdir_in(dirs::home_dir().ok_or("missing home directory")?)?;
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("eutheto.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new("2026-09-01T00:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    let (store, _) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    store
        .create_project(NewProject {
            document: document.clone(),
        })
        .await?;
    drop(store);
    let app = EuthetoApp::open(dependencies.clone())
        .await
        .map_err(boxed)?;
    Ok((directory, dependencies, app))
}

async fn view(app: &EuthetoApp, scenario_id: ScenarioId) -> TestResult<ScenarioViewDto> {
    match app
        .query(AppQuery::ScenarioView(scenario_id))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::Scenario(view) => Ok(*view),
        other => Err(boxed(other)),
    }
}

async fn history(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
) -> TestResult<Vec<eutheto_store::HistoryEntry>> {
    match app
        .query(AppQuery::History(scenario_id))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::History(history) => Ok(history),
        other => Err(boxed(other)),
    }
}

async fn execute(
    app: &EuthetoApp,
    envelope: CommandEnvelope,
) -> TestResult<Result<CommandResult, AppError>> {
    match app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope,
            truncate_redo: false,
        })
        .await
    {
        Ok(AppCommandResult::ScenarioCommand(result)) => Ok(Ok(result)),
        Ok(other) => Err(boxed(other)),
        Err(error) => Ok(Err(error)),
    }
}

async fn assert_state(
    app: &EuthetoApp,
    document: &ScenarioDocument,
    revision: Revision,
    journal: &[eutheto_store::HistoryEntry],
) -> TestResult {
    let current = view(app, document.scenario_id).await?;
    assert_eq!(current.revision, revision);
    assert_eq!(&current.document, document);
    assert_eq!(
        history(app, document.scenario_id).await?.as_slice(),
        journal
    );
    Ok(())
}

async fn restart_round_trip(
    app: EuthetoApp,
    dependencies: AppDependencies,
    original: &ScenarioDocument,
    applied: &ScenarioDocument,
) -> TestResult {
    let scenario_id = original.scenario_id;
    let journal = history(&app, scenario_id).await?;
    assert_eq!(journal.len(), 1);
    drop(app);
    let app = EuthetoApp::open(dependencies.clone())
        .await
        .map_err(boxed)?;
    assert_state(&app, applied, Revision::new(1), &journal).await?;
    app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::new(1),
    })
    .await
    .map_err(boxed)?;
    let undone = view(&app, scenario_id).await?;
    assert_eq!(undone.revision, Revision::new(2));
    assert_eq!(&undone.document, original);
    drop(app);
    let app = EuthetoApp::open(dependencies).await.map_err(boxed)?;
    assert_eq!(view(&app, scenario_id).await?.document, *original);
    app.execute(AppCommand::Redo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::new(2),
    })
    .await
    .map_err(boxed)?;
    let redone = view(&app, scenario_id).await?;
    assert_eq!(redone.revision, Revision::new(3));
    assert_eq!(&redone.document, applied);
    let replayed_journal = history(&app, scenario_id).await?;
    assert_eq!(replayed_journal.len(), 1);
    assert_eq!(replayed_journal[0].command, journal[0].command);
    assert_eq!(replayed_journal[0].inverse, journal[0].inverse);
    Ok(())
}

fn fold_fixture() -> TestResult<ScenarioDocument> {
    let mut document = workforce_fixture::fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    entity_mut(&mut document, 8)?["startsAt"]["instant"] = json!("2026-11-01T05:30:00+00:00");
    let mut late_fold = entity(&document, 8)?.clone();
    late_fold["id"] = json!(id(20));
    late_fold["startsAt"] = json!({"instant":"2026-11-01T06:30:00Z","local":"2026-11-01T01:30:00","offsetSeconds":-18000});
    document.domain.entities.insert(id(20).parse()?, late_fold);
    let mut detached = entity(&document, 8)?.clone();
    detached["id"] = json!(id(21));
    detached["origin"] =
        json!({"kind":"detached","templateId":id(6),"occurrenceDate":"2026-11-08"});
    detached["startsAt"] = json!({"instant":"2026-11-08T14:00:00Z","local":"2026-11-08T09:00:00","offsetSeconds":-18000});
    detached["endsAt"] = json!({"instant":"2026-11-08T16:00:00Z","local":"2026-11-08T11:00:00","offsetSeconds":-18000});
    document.domain.entities.insert(id(21).parse()?, detached);
    let mut dormant = entity(&document, 8)?.clone();
    dormant["id"] = json!(id(22));
    dormant["startsAt"] = json!({"instant":"2026-06-01T13:00:00Z","local":"2026-06-01T09:00:00","offsetSeconds":-14400});
    dormant["endsAt"] = json!({"instant":"2026-06-01T15:00:00Z","local":"2026-06-01T11:00:00","offsetSeconds":-14400});
    document.domain.entities.insert(id(22).parse()?, dormant);
    Ok(document)
}

fn chicago(document: &ScenarioDocument) -> TestResult<ScenarioSettings> {
    let mut desired = document.settings.clone();
    desired.time_zone = "America/Chicago".parse()?;
    desired.horizon = Horizon::new(
        "2026-11-01T05:00:00Z".parse()?,
        "2026-11-02T06:00:00Z".parse()?,
    )?;
    Ok(desired)
}

fn instant(record: &Value, endpoint: &str) -> TestResult<Rfc3339Timestamp> {
    Ok(record[endpoint]["instant"]
        .as_str()
        .ok_or("missing instant")?
        .parse()?)
}

fn elapsed_seconds(record: &Value) -> TestResult<i64> {
    Ok(instant(record, "endsAt")?.as_timestamp().as_second()
        - instant(record, "startsAt")?.as_timestamp().as_second())
}

#[tokio::test]
async fn timezone_preserves_every_stored_instant_and_recurring_intent_across_restart() -> TestResult
{
    let original = fold_fixture()?;
    // A new overlap default cannot reinterpret either explicit fold choice.
    let mut later = original.settings.clone();
    later.overlap_policy = OverlapPolicy::Later;
    let unchanged_times = pure(&original, settings(later.clone(), None))??;
    let mut expected_policy_only = original.clone();
    expected_policy_only.settings = later;
    assert_eq!(unchanged_times.document, expected_policy_only);

    let desired = chicago(&original)?;
    let (_directory, dependencies, app) = stored(&original).await?;
    let command = settings(desired.clone(), None);
    let in_memory = pure(&original, command.clone())??;
    let result = execute(&app, envelope(&original, Revision::INITIAL, command)?)
        .await?
        .map_err(boxed)?;
    assert_eq!(result.new_revision, Revision::new(1));
    let applied = view(&app, original.scenario_id).await?.document;
    assert_eq!(applied, in_memory.document);
    let mut expected = original.clone();
    expected.settings = desired;
    for (index, start, start_offset, end, end_offset) in [
        (
            8,
            "2026-11-01T00:30:00",
            -18000,
            "2026-11-01T01:30:00",
            -21600,
        ),
        (
            20,
            "2026-11-01T01:30:00",
            -18000,
            "2026-11-01T01:30:00",
            -21600,
        ),
        (
            21,
            "2026-11-08T08:00:00",
            -21600,
            "2026-11-08T10:00:00",
            -21600,
        ),
        (
            22,
            "2026-06-01T08:00:00",
            -18000,
            "2026-06-01T10:00:00",
            -18000,
        ),
    ] {
        let record = entity_mut(&mut expected, index)?;
        record["startsAt"]["local"] = json!(start);
        record["startsAt"]["offsetSeconds"] = json!(start_offset);
        record["endsAt"]["local"] = json!(end);
        record["endsAt"]["offsetSeconds"] = json!(end_offset);
        let before = entity(&original, index)?;
        let after = entity(&applied, index)?;
        for endpoint in ["startsAt", "endsAt"] {
            assert_eq!(instant(after, endpoint)?, instant(before, endpoint)?);
            assert_eq!(after[endpoint]["instant"], before[endpoint]["instant"]);
        }
        assert_eq!(elapsed_seconds(after)?, elapsed_seconds(before)?);
    }
    assert_eq!(applied, expected);
    let generated_before = resolve_shifts(&original, &CancellationToken::new())?;
    let generated_after = resolve_shifts(&applied, &CancellationToken::new())?;
    let occurrence_id = id(7).parse()?;
    let before = generated_before
        .iter()
        .find(|shift| shift.id == occurrence_id)
        .ok_or("missing original occurrence")?;
    let after = generated_after
        .iter()
        .find(|shift| shift.id == occurrence_id)
        .ok_or("missing target occurrence")?;
    assert_eq!(
        before.interval.starts_at.local,
        after.interval.starts_at.local
    );
    assert_eq!(
        before.interval.starts_at.instant,
        "2026-11-01T05:30:00Z".parse()?
    );
    assert_eq!(
        after.interval.starts_at.instant,
        "2026-11-01T06:30:00Z".parse()?
    );
    restart_round_trip(app, dependencies, &original, &applied).await
}

#[tokio::test]
async fn gap_policy_normalizes_without_moving_instants_and_undo_restores_raw_gap_intent()
-> TestResult {
    let mut original = workforce_fixture::fixture()?;
    original.settings.gap_policy = GapPolicy::MoveForward;
    entity_mut(&mut original, 8)?["startsAt"] = json!({
        "instant":"2026-03-08T07:30:00+00:00", "local":"2026-03-08T02:30:00", "offsetSeconds":-14400
    });
    entity_mut(&mut original, 8)?["endsAt"] = json!({
        "instant":"2026-03-08T08:30:00+00:00", "local":"2026-03-08T04:30:00", "offsetSeconds":-14400
    });
    let mut desired = original.settings.clone();
    desired.gap_policy = GapPolicy::Reject;
    let (_directory, dependencies, app) = stored(&original).await?;
    execute(
        &app,
        envelope(
            &original,
            Revision::INITIAL,
            settings(desired.clone(), None),
        )?,
    )
    .await?
    .map_err(boxed)?;
    let applied = view(&app, original.scenario_id).await?.document;
    let mut expected = original.clone();
    expected.settings = desired;
    entity_mut(&mut expected, 8)?["startsAt"]["local"] = json!("2026-03-08T03:30:00");
    assert_eq!(applied, expected);
    assert_eq!(elapsed_seconds(entity(&applied, 8)?)?, 3600);
    restart_round_trip(app, dependencies, &original, &applied).await
}

#[tokio::test]
async fn mixed_batch_uses_new_settings_prefix_and_late_failure_is_atomic() -> TestResult {
    let original = fold_fixture()?;
    let desired = chicago(&original)?;
    let transitioned = pure(&original, settings(desired.clone(), None))??.document;
    let mut updated = entity(&transitioned, 8)?.clone();
    updated["tags"] = json!(["changed-after-settings"]);
    let update = domain(commands::UPDATE_ENTITY, json!({"entity":updated}));
    // The same endpoint update is invalid in the old zone, proving prefix authority matters.
    assert!(matches!(
        pure(&original, update.clone())?,
        Err(CommandError::InvalidDomainPayload { .. })
    ));
    let commands = vec![settings(desired, None), update];
    let mut failing_commands = commands.clone();
    failing_commands.push(domain(commands::REMOVE_ENTITY, json!({"entityId":id(5)})));
    let (_directory, dependencies, app) = stored(&original).await?;
    let before_history = history(&app, original.scenario_id).await?;
    assert!(matches!(
        execute(
            &app,
            envelope(&original, Revision::INITIAL, batch(failing_commands))?
        )
        .await?,
        Err(AppError::Validation(_))
    ));
    assert_state(&app, &original, Revision::INITIAL, &before_history).await?;
    let result = execute(
        &app,
        envelope(&original, Revision::INITIAL, batch(commands))?,
    )
    .await?
    .map_err(boxed)?;
    assert_eq!(result.new_revision, Revision::new(1));
    let mut expected = transitioned;
    *entity_mut(&mut expected, 8)? = updated;
    let applied = view(&app, original.scenario_id).await?.document;
    assert_eq!(applied, expected);
    restart_round_trip(app, dependencies, &original, &applied).await
}

fn restoration_entry(document: &ScenarioDocument, index: u32) -> TestResult<Value> {
    let record = entity(document, index)?;
    Ok(json!({"shiftId":id(index),"startsAt":record["startsAt"],"endsAt":record["endsAt"]}))
}

fn invalid_restorations(current: &ScenarioDocument) -> TestResult<Vec<(&'static str, Value)>> {
    let entry = restoration_entry(current, 8)?;
    let valid = json!({"schemaVersion":1,"times":[entry.clone()]});
    let mut wrong_version = valid.clone();
    wrong_version["schemaVersion"] = json!(2);
    let mut malformed = valid.clone();
    malformed["times"][0]["startsAt"]["instant"] = json!("not-an-instant");
    let mut missing_endpoint = valid.clone();
    missing_endpoint["times"][0]
        .as_object_mut()
        .ok_or("entry is not an object")?
        .remove("endsAt");
    let mut forged_record = valid.clone();
    forged_record["times"][0]["origin"] = json!({"kind":"manual"});
    let mut forged_endpoint = valid.clone();
    forged_endpoint["times"][0]["startsAt"]["extra"] = json!(true);
    let mut forged_root = valid.clone();
    forged_root["entities"] = json!({});
    let mut duplicate_alias = entry.clone();
    duplicate_alias["shiftId"] = json!(id(8).to_uppercase());
    let mut moving = valid.clone();
    moving["times"][0]["startsAt"] = json!({
        "instant":"2026-11-01T06:30:00Z","local":"2026-11-01T01:30:00","offsetSeconds":-18000
    });
    let mut inconsistent_offset = valid.clone();
    inconsistent_offset["times"][0]["startsAt"]["offsetSeconds"] = json!(-18000);
    let mut rejected = vec![
        ("wrong version", wrong_version),
        ("malformed endpoint", malformed),
        ("missing endpoint", missing_endpoint),
        ("forged record field", forged_record),
        ("forged endpoint field", forged_endpoint),
        ("forged root field", forged_root),
        (
            "duplicate identity",
            json!({"schemaVersion":1,"times":[entry.clone(),entry.clone()]}),
        ),
        (
            "duplicate UUID alias",
            json!({"schemaVersion":1,"times":[entry.clone(),duplicate_alias]}),
        ),
        (
            "unsorted identities",
            json!({"schemaVersion":1,"times":[restoration_entry(current,20)?,entry.clone()]}),
        ),
        ("moved instant", moving),
        ("inconsistent offset", inconsistent_offset),
    ];
    for (name, index) in [
        ("non-shift entity", 1),
        ("generated occurrence", 7),
        ("missing shift", 99),
    ] {
        let mut payload = valid.clone();
        payload["times"][0]["shiftId"] = json!(id(index));
        rejected.push((name, payload));
    }
    Ok(rejected)
}

#[tokio::test]
async fn invalid_restoration_stale_revision_and_cancellation_preserve_document_and_history()
-> TestResult {
    let original = fold_fixture()?;
    let (_directory, mut dependencies, app) = stored(&original).await?;
    let mut desired = original.settings.clone();
    desired.locale = "en-GB".parse()?;
    execute(
        &app,
        envelope(&original, Revision::INITIAL, settings(desired, None))?,
    )
    .await?
    .map_err(boxed)?;
    let current = view(&app, original.scenario_id).await?.document;
    let journal = history(&app, original.scenario_id).await?;
    for (name, payload) in invalid_restorations(&current)? {
        let command = settings(current.settings.clone(), Some(payload));
        assert!(
            matches!(
                pure(&current, command.clone())?,
                Err(CommandError::Validation { .. })
            ),
            "{name}"
        );
        assert!(
            matches!(
                execute(&app, envelope(&current, Revision::new(1), command)?).await?,
                Err(AppError::Validation(_))
            ),
            "{name}"
        );
        assert_state(&app, &current, Revision::new(1), &journal).await?;
    }
    assert_eq!(
        execute(
            &app,
            envelope(
                &current,
                Revision::INITIAL,
                settings(original.settings.clone(), None)
            )?
        )
        .await?,
        Err(AppError::Conflict {
            expected_revision: Revision::INITIAL,
            actual_revision: Revision::new(1)
        })
    );
    assert_state(&app, &current, Revision::new(1), &journal).await?;
    dependencies.cancellation.cancel();
    assert!(matches!(
        execute(&app, envelope(&current, Revision::new(1), settings(chicago(&current)?, None))?).await?,
        Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled"
    ));
    drop(app);
    dependencies.cancellation = CancellationToken::new();
    let app = EuthetoApp::open(dependencies).await.map_err(boxed)?;
    assert_state(&app, &current, Revision::new(1), &journal).await
}

#[test]
fn invalid_original_settings_authority_precedes_restoration_decoding() -> TestResult {
    let mut original = workforce_fixture::fixture()?;
    entity_mut(&mut original, 8)?["startsAt"]["offsetSeconds"] = json!(0);
    let control = eutheto_types::OperationControl::Cancellation(CancellationToken::new());
    let source_error = eutheto_domain_api::DomainPack::reconcile_settings(
        &WorkforcePack,
        &original,
        &original.settings,
        None,
        &control,
    )
    .err()
    .ok_or("invalid source endpoint accepted")?;
    let restoration = json!({"schemaVersion": 2, "times": []});
    let restoration_error = eutheto_domain_api::DomainPack::reconcile_settings(
        &WorkforcePack,
        &original,
        &original.settings,
        Some(&restoration),
        &control,
    )
    .err()
    .ok_or("invalid source and restoration accepted")?;
    assert_eq!(restoration_error, source_error);
    Ok(())
}

#[tokio::test]
async fn setup_preview_cursor_binds_the_exact_draft_without_persisting_it() -> TestResult {
    use eutheto_core::SetupSourceV2;
    use eutheto_domain_api::DomainSetupQueryV1;
    use eutheto_workforce::setup::contracts::{WorkforceSetupResultV1, WorkforceSetupViewDataV1};

    let original = workforce_fixture::fixture()?;
    let (_directory, dependencies, app) = stored(&original).await?;
    let mut changed = original.settings.clone();
    changed.locale = "fr-CA".parse()?;
    let source = SetupSourceV2::CommandPreview {
        command: batch(vec![
            settings(changed.clone(), None),
            settings(original.settings.clone(), None),
        ]),
    };
    let mut query = DomainSetupQueryV1 {
        schema_version: 1,
        view_id: "eutheto.setup.command_changes".to_owned(),
        parameters: json!({"limit":1}),
        continuation: None,
    };
    let first = app
        .setup_view(
            original.scenario_id,
            Revision::INITIAL,
            source.clone(),
            query.clone(),
            app.setup_cancellation(),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(first.revision, Revision::INITIAL);
    let WorkforceSetupViewDataV1::CommandChanges(page) =
        serde_json::from_value::<WorkforceSetupResultV1>(first.view.data)?.result
    else {
        return Err("wrong preview family".into());
    };
    assert_eq!(page.total_items, 2);
    assert_eq!(
        page.items[0].change.before,
        Some(serde_json::to_value(&original.settings)?)
    );
    assert_eq!(
        page.items[0].change.after,
        Some(serde_json::to_value(&changed)?)
    );
    query.continuation = Some(page.continuation.ok_or("missing draft continuation")?);
    let last = app
        .setup_view(
            original.scenario_id,
            Revision::INITIAL,
            source,
            query.clone(),
            app.setup_cancellation(),
        )
        .await
        .map_err(boxed)?;
    let WorkforceSetupViewDataV1::CommandChanges(page) =
        serde_json::from_value::<WorkforceSetupResultV1>(last.view.data)?.result
    else {
        return Err("wrong preview family".into());
    };
    assert_eq!(
        page.items[0].change.before,
        Some(serde_json::to_value(&changed)?)
    );
    assert_eq!(
        page.items[0].change.after,
        Some(serde_json::to_value(&original.settings)?)
    );
    assert!(page.continuation.is_none());
    changed.locale = "de-DE".parse()?;
    let different = SetupSourceV2::CommandPreview {
        command: batch(vec![
            settings(changed, None),
            settings(original.settings.clone(), None),
        ]),
    };
    assert!(matches!(
        app.setup_view(
            original.scenario_id,
            Revision::INITIAL,
            different,
            query,
            app.setup_cancellation()
        )
        .await,
        Err(AppError::Validation(_))
    ));
    assert_state(&app, &original, Revision::INITIAL, &[]).await?;
    drop(app);
    let app = EuthetoApp::open(dependencies).await.map_err(boxed)?;
    assert_state(&app, &original, Revision::INITIAL, &[]).await
}

#[tokio::test]
async fn generation_preview_requires_executable_reconciliation_and_isolates_cancellation()
-> TestResult {
    use eutheto_core::SetupSourceV2;
    use eutheto_domain_api::DomainSetupQueryV1;
    use eutheto_workforce::setup::contracts::{WorkforceSetupResultV1, WorkforceSetupViewDataV1};

    let mut original = workforce_fixture::fixture()?;
    original.settings.overlap_policy = OverlapPolicy::Earlier;
    let (_directory, _dependencies, app) = stored(&original).await?;
    let mut target = original.settings.clone();
    target.horizon.end = "2026-11-09T05:00:00Z".parse()?;
    let draft = settings(target, None);
    let query = DomainSetupQueryV1 {
        schema_version: 1,
        view_id: "official.workforce.setup.generation_review".to_owned(),
        parameters: json!({"changesOnly":true, "limit":1}),
        continuation: None,
    };
    let cancelled = app.setup_cancellation();
    cancelled.cancel();
    assert!(
        matches!(app.setup_view(original.scenario_id, Revision::INITIAL,
        SetupSourceV2::CommandPreview { command:draft.clone() }, query.clone(), cancelled).await,
        Err(AppError::Protocol(failure)) if failure.code == "operation.cancelled")
    );
    let result = app
        .setup_view(
            original.scenario_id,
            Revision::INITIAL,
            SetupSourceV2::CommandPreview {
                command: draft.clone(),
            },
            query.clone(),
            app.setup_cancellation(),
        )
        .await
        .map_err(boxed)?;
    let WorkforceSetupViewDataV1::GenerationReview(review) =
        serde_json::from_value::<WorkforceSetupResultV1>(result.view.data)?.result
    else {
        return Err("wrong generation family".into());
    };
    assert_eq!(
        (
            review.total_added,
            review.total_changed,
            review.total_removed
        ),
        (1, 0, 0)
    );
    assert!(review.reconciliation_required);
    let mut wrapped = draft;
    for _ in 0..eutheto_command::MAX_BATCH_DEPTH {
        wrapped = batch(vec![wrapped]);
    }
    assert!(
        matches!(app.setup_view(original.scenario_id, Revision::INITIAL,
        SetupSourceV2::CommandPreview { command:wrapped }, query.clone(), app.setup_cancellation()).await,
        Err(AppError::Validation(report)) if report.issues.iter().any(|issue|
            issue.code == eutheto_command::CODE_BATCH_DEPTH_EXCEEDED))
    );
    assert_eq!(
        app.setup_view(
            original.scenario_id,
            Revision::new(1),
            SetupSourceV2::Stored,
            query,
            app.setup_cancellation()
        )
        .await,
        Err(AppError::Conflict {
            expected_revision: Revision::new(1),
            actual_revision: Revision::INITIAL,
        })
    );
    assert_state(&app, &original, Revision::INITIAL, &[]).await
}

async fn generation_request(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    source: eutheto_core::SetupSourceV2,
) -> TestResult<eutheto_core::WorkforceGenerationApplyRequestV1> {
    use eutheto_workforce::setup::contracts::{WorkforceSetupResultV1, WorkforceSetupViewDataV1};
    let result = app
        .setup_view(
            scenario_id,
            Revision::INITIAL,
            source.clone(),
            eutheto_domain_api::DomainSetupQueryV1 {
                schema_version: 1,
                view_id: "official.workforce.setup.generation_review".to_owned(),
                parameters: json!({"changesOnly":true,"limit":1}),
                continuation: None,
            },
            app.setup_cancellation(),
        )
        .await
        .map_err(boxed)?;
    let WorkforceSetupViewDataV1::GenerationReview(review) =
        serde_json::from_value::<WorkforceSetupResultV1>(result.view.data)?.result
    else {
        return Err("wrong generation view".into());
    };
    Ok(eutheto_core::WorkforceGenerationApplyRequestV1 {
        request_id: RequestId::new(&SystemIdGenerator)?,
        schema_version: 1,
        operation_id: eutheto_types::OperationId::new(&SystemIdGenerator)?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::INITIAL,
        actor: ActorRef {
            actor_id: None,
            display_name: "Generation review".to_owned(),
        },
        source,
        prospective_hash: review.prospective_hash,
        truncate_redo: false,
    })
}

#[tokio::test]
async fn reviewed_generation_rechecks_hash_and_commits_one_reversible_command() -> TestResult {
    let mut original = workforce_fixture::fixture()?;
    original.settings.overlap_policy = OverlapPolicy::Earlier;
    let (_directory, dependencies, app) = stored(&original).await?;
    let mut target = original.settings.clone();
    target.horizon.end = "2026-11-09T05:00:00Z".parse()?;
    let request = generation_request(
        &app,
        original.scenario_id,
        eutheto_core::SetupSourceV2::CommandPreview {
            command: settings(target.clone(), None),
        },
    )
    .await?;
    let mut tampered = request.clone();
    tampered.prospective_hash[0] ^= 1;
    assert!(
        matches!(app.apply_reviewed_generation(tampered, app.setup_cancellation()).await,
        Err(AppError::Validation(report)) if report.issues.iter().any(|issue|
            issue.code == "workforce.generation_review_changed"))
    );
    let mut changed_draft = request.clone();
    target.locale = "fr-CA".parse()?;
    changed_draft.source = eutheto_core::SetupSourceV2::CommandPreview {
        command: settings(target, None),
    };
    assert!(
        matches!(app.apply_reviewed_generation(changed_draft, app.setup_cancellation()).await,
        Err(AppError::Validation(report)) if report.issues.iter().any(|issue|
            issue.code == "workforce.generation_review_changed"))
    );
    assert_state(&app, &original, Revision::INITIAL, &[]).await?;
    let result = app
        .apply_reviewed_generation(request.clone(), app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert_eq!(result.new_revision, Revision::new(1));
    let applied = view(&app, original.scenario_id).await?.document;
    let resolved = resolve_shifts(&applied, &CancellationToken::new())?;
    assert_eq!(resolved.len(), 3);
    let journal = history(&app, original.scenario_id).await?;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].id, request.command_id);
    assert_eq!(
        app.apply_reviewed_generation(request, app.setup_cancellation())
            .await,
        Err(AppError::Conflict {
            expected_revision: Revision::INITIAL,
            actual_revision: Revision::new(1)
        })
    );
    restart_round_trip(app, dependencies, &original, &applied).await
}

#[tokio::test]
async fn stored_generation_reconciles_without_draft_and_rejects_empty_application() -> TestResult {
    let mut original = workforce_fixture::fixture()?;
    original.settings.overlap_policy = OverlapPolicy::Earlier;
    original.settings.horizon.end = "2026-11-09T05:00:00Z".parse()?;
    let spelling = id(7).to_uppercase();
    let identities = entity_mut(&mut original, 6)?["occurrenceIdentities"]
        .as_object_mut()
        .ok_or("missing occurrence ledger")?;
    let mut retained = identities
        .remove(&id(7))
        .ok_or("missing retained occurrence")?;
    retained["id"] = json!(spelling);
    identities.insert(spelling.clone(), retained.clone());
    let (_directory, dependencies, app) = stored(&original).await?;
    let request = generation_request(
        &app,
        original.scenario_id,
        eutheto_core::SetupSourceV2::Stored,
    )
    .await?;
    let result = app
        .apply_reviewed_generation(request, app.setup_cancellation())
        .await
        .map_err(boxed)?;
    assert_eq!(result.new_revision, Revision::new(1));
    let applied = view(&app, original.scenario_id).await?.document;
    assert_eq!(applied.settings, original.settings);
    assert_eq!(
        resolve_shifts(&applied, &CancellationToken::new())?.len(),
        3
    );
    assert_eq!(
        entity(&applied, 6)?["occurrenceIdentities"][&spelling],
        retained
    );
    restart_round_trip(app, dependencies, &original, &applied).await?;

    let mut complete = workforce_fixture::fixture()?;
    complete.settings.overlap_policy = OverlapPolicy::Earlier;
    let (_directory, _dependencies, app) = stored(&complete).await?;
    let request = generation_request(
        &app,
        complete.scenario_id,
        eutheto_core::SetupSourceV2::Stored,
    )
    .await?;
    assert!(
        matches!(app.apply_reviewed_generation(request, app.setup_cancellation()).await,
        Err(AppError::Validation(report)) if report.issues.iter().any(|issue|
            issue.code == "workforce.generation_no_changes"))
    );
    assert_state(&app, &complete, Revision::INITIAL, &[]).await
}

#[tokio::test]
async fn reviewed_generation_rejects_unbounded_actor_before_source_capture() -> TestResult {
    let mut original = workforce_fixture::fixture()?;
    original.settings.overlap_policy = OverlapPolicy::Earlier;
    let (_directory, _dependencies, app) = stored(&original).await?;
    let mut request = generation_request(
        &app,
        original.scenario_id,
        eutheto_core::SetupSourceV2::Stored,
    )
    .await?;
    request.scenario_id = ScenarioId::new(&SystemIdGenerator)?;
    for actor in [
        ActorRef {
            actor_id: None,
            display_name: "x".repeat(257),
        },
        ActorRef {
            actor_id: Some("x".repeat(257)),
            display_name: "Reviewer".to_owned(),
        },
        ActorRef {
            actor_id: None,
            display_name: "Reviewer\ninjected".to_owned(),
        },
    ] {
        request.actor = actor;
        assert!(
            matches!(app.apply_reviewed_generation(request.clone(), app.setup_cancellation()).await,
            Err(AppError::Validation(report)) if report.issues.iter().any(|issue|
                issue.field_path.as_deref() == Some("/actor")))
        );
    }
    request.actor = ActorRef {
        actor_id: Some("x".repeat(256)),
        display_name: "x".repeat(256),
    };
    assert!(matches!(
        app.apply_reviewed_generation(request, app.setup_cancellation())
            .await,
        Err(AppError::NotFound(_))
    ));
    assert_state(&app, &original, Revision::INITIAL, &[]).await
}

#[tokio::test]
async fn setup_source_and_continuation_policy_rejects_before_scenario_capture() -> TestResult {
    use eutheto_core::SetupSourceV2;
    use eutheto_domain_api::{DomainPack, DomainSetupQueryV1, SetupContinuationV1};
    let original = workforce_fixture::fixture()?;
    let (_directory, _dependencies, app) = stored(&original).await?;
    let missing = ScenarioId::new(&SystemIdGenerator)?;
    let catalog = WorkforcePack.catalog()?;
    let make_query = |id: &str| -> TestResult<DomainSetupQueryV1> {
        let descriptor = catalog
            .setup_queries
            .iter()
            .find(|query| query.id == id)
            .ok_or("missing registered query")?;
        Ok(DomainSetupQueryV1 {
            schema_version: 1,
            view_id: id.to_owned(),
            parameters: descriptor.valid_examples[0].clone(),
            continuation: None,
        })
    };
    let preview = SetupSourceV2::CommandPreview {
        command: settings(original.settings.clone(), None),
    };
    let command_changes = make_query("eutheto.setup.command_changes")?;
    let settings_query = make_query("official.workforce.setup.settings_preparation")?;
    let mut continued_settings = settings_query.clone();
    continued_settings.continuation = Some(SetupContinuationV1 {
        schema_version: 1,
        scenario_id: missing,
        revision: Revision::INITIAL,
        query_fingerprint: [0; 32],
        position: json!({}),
    });
    for (source, query) in [
        (SetupSourceV2::Stored, command_changes.clone()),
        (preview.clone(), settings_query.clone()),
        (SetupSourceV2::Stored, continued_settings),
    ] {
        assert!(matches!(
            app.setup_view(
                missing,
                Revision::INITIAL,
                source,
                query,
                app.setup_cancellation()
            )
            .await,
            Err(AppError::Validation(_))
        ));
    }
    for (source, query) in [
        (preview, command_changes),
        (SetupSourceV2::Stored, settings_query),
    ] {
        assert!(matches!(
            app.setup_view(
                missing,
                Revision::INITIAL,
                source,
                query,
                app.setup_cancellation()
            )
            .await,
            Err(AppError::NotFound(_))
        ));
    }
    assert_state(&app, &original, Revision::INITIAL, &[]).await
}
