#![forbid(unsafe_code)]

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult, EuthetoApp,
    HISTORY_API_SCHEMA_VERSION, HISTORY_PAGE_MAX_ENTRIES, HISTORY_SUMMARY_MAX_BYTES,
    HistoryPageContinuationV1, HistoryPageDtoV1, HistoryPageRequestV1,
};
use eutheto_store::{NewProject, SqliteScenarioStore};
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandBatch, CommandEnvelope, CommandId, CommandSource,
    DomainCommandEnvelope, FixedClock, FixedMonotonicClock, RequestId, ResourceRef, Revision,
    ScenarioCommand, ScenarioDocument, ScenarioId, SystemIdGenerator,
};
use eutheto_workforce::commands;
use serde_json::json;
use std::{error::Error, fmt::Debug, sync::Arc};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: impl Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

async fn stored(document: &ScenarioDocument) -> TestResult<(TempDir, AppDependencies, EuthetoApp)> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-history-page-")
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

fn request(scenario_id: ScenarioId, revision: u64, limit: u32) -> HistoryPageRequestV1 {
    HistoryPageRequestV1 {
        schema_version: HISTORY_API_SCHEMA_VERSION,
        scenario_id,
        expected_revision: Revision::new(revision),
        limit,
        continuation: None,
    }
}

async fn page(app: &EuthetoApp, request: HistoryPageRequestV1) -> TestResult<HistoryPageDtoV1> {
    match app
        .query(AppQuery::HistoryPage(request))
        .await
        .map_err(boxed)?
    {
        AppQueryResult::HistoryPage(page) => Ok(page),
        other => Err(boxed(other)),
    }
}

fn update_qualification(index: u32, name: &str, description: &str) -> ScenarioCommand {
    ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
        command_type: commands::UPDATE_ENTITY.to_owned(),
        payload: json!({"entity": {
            "kind": "qualification", "id": workforce_fixture::id(index),
            "name": name, "description": description,
        }}),
    })
}

async fn apply(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: u64,
    command: ScenarioCommand,
    truncate_redo: bool,
) -> TestResult<CommandId> {
    let command_id = CommandId::new(&SystemIdGenerator)?;
    let result = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id,
                scenario_id,
                expected_revision: Revision::new(revision),
                actor: ActorRef {
                    actor_id: None,
                    display_name: "History regression".to_owned(),
                },
                source: CommandSource::System,
                command,
            },
            truncate_redo,
        })
        .await
        .map_err(boxed)?;
    assert!(matches!(result, AppCommandResult::ScenarioCommand(result)
        if result.new_revision == Revision::new(revision + 1)));
    Ok(command_id)
}

async fn move_history(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: u64,
    undo: bool,
) -> TestResult {
    let request_id = RequestId::new(&SystemIdGenerator)?;
    let expected_revision = Revision::new(revision);
    let command = if undo {
        AppCommand::Undo {
            request_id,
            scenario_id,
            expected_revision,
        }
    } else {
        AppCommand::Redo {
            request_id,
            scenario_id,
            expected_revision,
        }
    };
    app.execute(command).await.map_err(boxed)?;
    Ok(())
}

fn sequences(page: &HistoryPageDtoV1) -> Vec<u64> {
    page.entries
        .iter()
        .map(|entry| entry.history_sequence)
        .collect()
}

async fn assert_stale(app: &EuthetoApp, previous: &HistoryPageDtoV1, actual: u64) -> TestResult {
    let mut stale = request(previous.scenario_id, previous.revision.value(), 1);
    stale.continuation.clone_from(&previous.continuation);
    assert!(matches!(app.query(AppQuery::HistoryPage(stale)).await,
        Err(AppError::Conflict { expected_revision, actual_revision })
            if expected_revision == previous.revision && actual_revision == Revision::new(actual)));
    Ok(())
}

#[tokio::test]
async fn pages_are_bounded_disjoint_and_exhaust_the_retained_journal() -> TestResult {
    let document = workforce_fixture::fixture()?;
    let (_directory, _dependencies, app) = stored(&document).await?;
    let scenario_id = document.scenario_id;
    let empty = page(&app, request(scenario_id, 0, 2)).await?;
    assert!(empty.entries.is_empty());
    assert_eq!((empty.undo_available, empty.redo_available), (false, false));
    assert!(empty.continuation.is_none());
    let mut ids = Vec::new();
    for revision in 0..5 {
        ids.push(
            apply(
                &app,
                scenario_id,
                revision,
                update_qualification(11, &format!("Clinician {revision}"), ""),
                false,
            )
            .await?,
        );
    }
    let first = page(&app, request(scenario_id, 5, 2)).await?;
    assert_eq!(sequences(&first), [5, 4]);
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        [ids[4], ids[3]]
    );
    assert!(first.entries.iter().all(|entry| entry.applied));
    assert_eq!((first.undo_available, first.redo_available), (true, false));
    let mut next = request(scenario_id, 5, 2);
    next.continuation = first.continuation.clone();
    let second = page(&app, next.clone()).await?;
    assert_eq!(sequences(&second), [3, 2]);
    next.continuation = second.continuation;
    let last = page(&app, next).await?;
    assert_eq!(sequences(&last), [1]);
    assert!(last.continuation.is_none());
    assert_eq!((last.undo_available, last.redo_available), (true, false));
    let all = page(&app, request(scenario_id, 5, HISTORY_PAGE_MAX_ENTRIES)).await?;
    assert_eq!(sequences(&all), [5, 4, 3, 2, 1]);
    assert!(all.continuation.is_none());
    Ok(())
}

#[tokio::test]
async fn cursor_availability_and_continuations_follow_undo_redo_and_branch_changes() -> TestResult {
    let document = workforce_fixture::fixture()?;
    let (_directory, _dependencies, app) = stored(&document).await?;
    let scenario_id = document.scenario_id;
    apply(
        &app,
        scenario_id,
        0,
        update_qualification(11, "First", ""),
        false,
    )
    .await?;
    let abandoned = apply(
        &app,
        scenario_id,
        1,
        update_qualification(11, "Second", ""),
        false,
    )
    .await?;
    let original = page(&app, request(scenario_id, 2, 1)).await?;
    move_history(&app, scenario_id, 2, true).await?;
    assert_stale(&app, &original, 3).await?;
    let undone = page(&app, request(scenario_id, 3, 1)).await?;
    assert_eq!((undone.undo_available, undone.redo_available), (true, true));
    assert!(!undone.entries[0].applied);
    let mut rebound = request(scenario_id, 3, 1);
    rebound.continuation = original.continuation;
    assert!(matches!(
        app.query(AppQuery::HistoryPage(rebound)).await,
        Err(AppError::Validation(_))
    ));
    let mut past_end = request(scenario_id, 3, 1);
    past_end.continuation = Some(HistoryPageContinuationV1 {
        scenario_id,
        revision: Revision::new(3),
        before_sequence: 1,
    });
    let past_end = page(&app, past_end).await?;
    assert!(past_end.entries.is_empty());
    assert_eq!(
        (past_end.undo_available, past_end.redo_available),
        (true, true)
    );
    move_history(&app, scenario_id, 3, false).await?;
    assert_stale(&app, &undone, 4).await?;
    let redone = page(&app, request(scenario_id, 4, 1)).await?;
    assert_eq!(
        (redone.undo_available, redone.redo_available),
        (true, false)
    );
    assert!(redone.entries[0].applied);
    move_history(&app, scenario_id, 4, true).await?;
    let before_branch = page(&app, request(scenario_id, 5, 1)).await?;
    let replacement = apply(
        &app,
        scenario_id,
        5,
        update_qualification(11, "New branch", ""),
        true,
    )
    .await?;
    assert_stale(&app, &before_branch, 6).await?;
    let branched = page(&app, request(scenario_id, 6, 100)).await?;
    assert_eq!(sequences(&branched), [2, 1]);
    assert_eq!(branched.entries[0].id, replacement);
    assert!(branched.entries.iter().all(|entry| entry.id != abandoned));
    assert_eq!(branched.entries[0].branch_generation, 1);
    assert_eq!(branched.entries[1].branch_generation, 0);
    assert_eq!(
        (branched.undo_available, branched.redo_available),
        (true, false)
    );
    move_history(&app, scenario_id, 6, true).await?;
    move_history(&app, scenario_id, 7, true).await?;
    let beginning = page(&app, request(scenario_id, 8, 100)).await?;
    assert_eq!(
        (beginning.undo_available, beginning.redo_available),
        (false, true)
    );
    assert!(beginning.entries.iter().all(|entry| !entry.applied));
    Ok(())
}

#[tokio::test]
async fn invalid_requests_fail_before_scenario_lookup_and_missing_scenarios_are_not_empty_pages()
-> TestResult {
    let document = workforce_fixture::fixture()?;
    let (_directory, _dependencies, app) = stored(&document).await?;
    let missing = ScenarioId::new(&SystemIdGenerator)?;
    let mut unsupported = request(missing, 0, 1);
    unsupported.schema_version = 2;
    assert!(
        matches!(app.query(AppQuery::HistoryPage(unsupported)).await,
        Err(AppError::Validation(report)) if report.issues[0].code == "history.schema_version_unsupported")
    );
    for limit in [0, 101, u32::MAX] {
        assert!(
            matches!(app.query(AppQuery::HistoryPage(request(missing, 0, limit))).await,
            Err(AppError::Validation(report)) if report.issues[0].field_path.as_deref() == Some("/limit"))
        );
    }
    for continuation in [
        HistoryPageContinuationV1 {
            scenario_id: document.scenario_id,
            revision: Revision::INITIAL,
            before_sequence: 1,
        },
        HistoryPageContinuationV1 {
            scenario_id: missing,
            revision: Revision::new(1),
            before_sequence: 1,
        },
        HistoryPageContinuationV1 {
            scenario_id: missing,
            revision: Revision::INITIAL,
            before_sequence: 0,
        },
        HistoryPageContinuationV1 {
            scenario_id: missing,
            revision: Revision::INITIAL,
            before_sequence: u64::MAX,
        },
    ] {
        let mut invalid = request(missing, 0, 1);
        invalid.continuation = Some(continuation);
        assert!(matches!(
            app.query(AppQuery::HistoryPage(invalid)).await,
            Err(AppError::Validation(_))
        ));
    }
    assert!(
        matches!(app.query(AppQuery::HistoryPage(request(missing, 0, 1))).await,
        Err(AppError::NotFound(ResourceRef::Scenario(id))) if id == missing)
    );
    assert!(
        serde_json::from_value::<HistoryPageRequestV1>(json!({
            "schemaVersion": 1, "scenarioId": missing, "expectedRevision": 0,
        }))
        .is_err()
    );
    let omitted: HistoryPageRequestV1 = serde_json::from_value(json!({
        "schemaVersion": 1, "scenarioId": document.scenario_id, "expectedRevision": 0, "limit": 1,
    }))?;
    assert!(page(&app, omitted).await?.entries.is_empty());
    Ok(())
}

#[tokio::test]
async fn summary_limit_is_utf8_bytes_and_omits_without_truncation() -> TestResult {
    let document = workforce_fixture::fixture()?;
    let (_directory, _dependencies, app) = stored(&document).await?;
    let label = format!("{}x", "é".repeat((HISTORY_SUMMARY_MAX_BYTES - 14) / 2));
    let recorded = format!("{label} (1 commands)");
    assert_eq!(recorded.len(), HISTORY_SUMMARY_MAX_BYTES);
    apply(
        &app,
        document.scenario_id,
        0,
        ScenarioCommand::ApplyBatch(CommandBatch {
            label: Some(label.clone()),
            commands: vec![update_qualification(11, "Boundary", "")],
        }),
        false,
    )
    .await?;
    apply(
        &app,
        document.scenario_id,
        1,
        ScenarioCommand::ApplyBatch(CommandBatch {
            label: Some(format!("{label}é")),
            commands: vec![update_qualification(11, "Omitted", "")],
        }),
        false,
    )
    .await?;
    let page = page(&app, request(document.scenario_id, 2, 100)).await?;
    assert_eq!(page.entries[0].summary, None);
    assert_eq!(page.entries[1].summary.as_deref(), Some(recorded.as_str()));
    let wire = serde_json::to_value(&page)?;
    assert!(wire["entries"][0]["summary"].is_null());
    Ok(())
}

#[tokio::test]
async fn bulk_valid_command_and_inverse_remain_usable_as_small_metadata() -> TestResult {
    let mut document = workforce_fixture::fixture()?;
    let padding = "x".repeat(220);
    let tags = |prefix: &str| {
        (0..2048)
            .map(|index| format!("{prefix} tag {index}: {padding}"))
            .collect::<Vec<_>>()
    };
    let person = document
        .domain
        .entities
        .get_mut(&workforce_fixture::id(1).parse()?)
        .ok_or("missing fixture person")?;
    person["tags"] = json!(tags("Original"));
    let mut updated = person.clone();
    updated["tags"] = json!(tags("Revised"));
    // One large valid mutation exercises payload isolation without benchmarking
    // repeated full-document preparation for hundreds of unrelated mutations.
    let command = ScenarioCommand::ApplyBatch(CommandBatch {
        label: Some("Revise person tags".to_owned()),
        commands: vec![ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: commands::UPDATE_ENTITY.to_owned(),
            payload: json!({"entity": updated}),
        })],
    });
    assert!(serde_json::to_vec(&command)?.len() > 400 * 1024);
    let (_directory, _dependencies, app) = stored(&document).await?;
    let command_id = apply(&app, document.scenario_id, 0, command, false).await?;
    let metadata = page(&app, request(document.scenario_id, 1, 1)).await?;
    assert_eq!(metadata.entries[0].id, command_id);
    assert_eq!(
        (metadata.undo_available, metadata.redo_available),
        (true, false)
    );
    assert!(serde_json::to_vec(&metadata)?.len() < 1024);
    // Exercise the real inverse, not malformed JSON or a fabricated authority row.
    move_history(&app, document.scenario_id, 1, true).await?;
    let restored = app
        .query(AppQuery::ScenarioView(document.scenario_id))
        .await
        .map_err(boxed)?;
    assert!(matches!(restored, AppQueryResult::Scenario(view) if view.document == document));
    let undone = page(&app, request(document.scenario_id, 2, 1)).await?;
    assert_eq!(
        (undone.undo_available, undone.redo_available),
        (false, true)
    );
    assert!(!undone.entries[0].applied);
    move_history(&app, document.scenario_id, 2, false).await?;
    let legacy = app
        .query(AppQuery::History(document.scenario_id))
        .await
        .map_err(boxed)?;
    let AppQueryResult::History(legacy) = legacy else {
        return Err("expected legacy history".into());
    };
    assert_eq!(legacy[0].id, command_id);
    assert!(
        serde_json::to_vec(legacy[0].inverse.as_ref().ok_or("missing inverse")?)?.len()
            > 400 * 1024
    );
    assert!(legacy[0].applied);
    Ok(())
}

#[tokio::test]
async fn recorded_empty_summary_and_nonreversible_entry_are_not_omissions_or_undo_capabilities()
-> TestResult {
    let document = workforce_fixture::fixture()?;
    let (_directory, dependencies, app) = stored(&document).await?;
    drop(app);
    let scenario_id = document.scenario_id;
    let (store, _) = SqliteScenarioStore::open(&dependencies.paths.database).await?;
    let envelope = CommandEnvelope {
        command_id: CommandId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::INITIAL,
        actor: ActorRef {
            actor_id: None,
            display_name: "Journal producer".to_owned(),
        },
        source: CommandSource::System,
        command: update_qualification(11, "Recorded without inverse", ""),
    };
    let registry = eutheto_domain_api::DomainPackRegistry::builder()
        .register(eutheto_workforce::WorkforcePack)
        .build()?;
    let applied_at = "2026-09-01T00:00:00Z".parse()?;
    // The store's public journal producer contract permits an absent inverse and
    // an empty recorded summary. Commit a real pure mutation through that API.
    store
        .execute_command(
            scenario_id,
            Revision::INITIAL,
            eutheto_store::RedoBranchPolicy::Reject,
            CancellationToken::new(),
            move |current| {
                let applied = eutheto_command::apply_command_with_registry(
                    current,
                    Revision::INITIAL,
                    &envelope,
                    &registry,
                    &CancellationToken::new(),
                )
                .map_err(|error| {
                    eutheto_store::StoreError::CommandApplication {
                        code: "history.fixture_command_failed".to_owned(),
                        message: format!("{error:?}"),
                    }
                })?;
                Ok(eutheto_store::CommandWrite {
                    document: applied.document,
                    journal: eutheto_store::JournalWrite {
                        command_type: applied.command_type,
                        command: serde_json::to_value(&envelope.command)?,
                        command_id: envelope.command_id,
                        inverse: None,
                        actor: envelope.actor,
                        source: envelope.source,
                        summary: String::new(),
                        created_at: applied_at,
                    },
                    output: (),
                })
            },
        )
        .await?;
    drop(store);
    let app = EuthetoApp::open(dependencies).await.map_err(boxed)?;
    let metadata = page(&app, request(scenario_id, 1, 1)).await?;
    assert_eq!(metadata.entries[0].summary.as_deref(), Some(""));
    assert_eq!(
        (metadata.undo_available, metadata.redo_available),
        (false, false)
    );
    assert!(metadata.entries[0].applied);
    assert!(matches!(app.execute(AppCommand::Undo {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Revision::new(1),
    }).await, Err(AppError::Validation(report))
        if report.issues[0].code == "history.command_not_reversible"));
    Ok(())
}
