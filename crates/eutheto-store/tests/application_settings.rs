use eutheto_store::{AppSetting, SqliteScenarioStore, StoreError};
#[cfg(debug_assertions)]
use eutheto_store::{CommandCommitTestHook, CommandCommitTestPhase, Failpoint, OpenOptions};
use eutheto_types::{CancellationToken, REVISION_MAX_V1, Revision, Rfc3339Timestamp};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use tempfile::tempdir;

const KEYS: &[&str] = &["appearance", "locale", "units"];
const CREATED: &str = "2026-08-28T23:00:00Z";
const UPDATED: &str = "2026-08-28T23:01:00Z";

fn setting(value: Value, timestamp: &str) -> Result<AppSetting<Value>, jiff::Error> {
    Ok(AppSetting {
        value,
        updated_at: Rfc3339Timestamp::parse(timestamp)?,
    })
}

async fn seed_settings(store: &SqliteScenarioStore) -> Result<(), Box<dyn Error>> {
    for (key, value) in [
        ("appearance", json!({"theme": "system"})),
        ("locale", json!("en-US")),
    ] {
        store
            .set_setting(key.to_owned(), value, Rfc3339Timestamp::parse(CREATED)?)
            .await?;
    }
    Ok(())
}

fn excluded_rows(
    connection: &Connection,
) -> Result<Vec<Vec<rusqlite::types::Value>>, rusqlite::Error> {
    let mut rows = Vec::new();
    for query in [
        "SELECT * FROM scenarios ORDER BY id",
        "SELECT * FROM portable_sections ORDER BY section, key",
        "SELECT * FROM portable_import_provenance ORDER BY id",
        "SELECT * FROM portable_library_metadata ORDER BY singleton",
        "SELECT * FROM app_metadata WHERE key <> 'portable_library_revision' ORDER BY key",
        "SELECT * FROM app_settings WHERE key = 'excluded'",
    ] {
        let mut statement = connection.prepare(query)?;
        let columns = statement.column_count();
        let selected =
            statement.query_map([], |row| (0..columns).map(|index| row.get(index)).collect())?;
        rows.extend(selected.collect::<Result<Vec<_>, _>>()?);
    }
    Ok(rows)
}

#[tokio::test]
async fn scoped_replacement_adds_updates_removes_and_preserves_excluded_rows()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_settings(&store).await?;
    let connection = Connection::open(&path)?;
    // Unreadable excluded payloads prove that settings operations do not load the library.
    connection.execute_batch(
        "INSERT INTO app_settings VALUES ('excluded', 'not-json', 'not-a-timestamp');
         INSERT INTO scenarios (id, domain_pack_id, domain_schema_version, title, revision,
             document_json, created_at, updated_at)
             VALUES ('excluded-scenario', 'official.test', 1, 'Excluded', 0,
                 'not-json', 'not-a-timestamp', 'not-a-timestamp');
         INSERT INTO portable_sections (section, key, value)
             VALUES ('results', 'excluded-result', X'010203');
         INSERT INTO portable_import_provenance VALUES (1, 'excluded-bundle', 'not-json',
             1, 1, 'excluded-digest', 'not-json', 'not-json', 'not-json',
             'not-a-timestamp', 'not-a-timestamp');
         UPDATE portable_library_metadata SET manifest_extensions_json = 'not-json';
         INSERT INTO app_metadata VALUES ('excluded-metadata', 'preserved');",
    )?;
    let excluded = excluded_rows(&connection)?;
    let before = store.settings_snapshot(KEYS).await?;
    assert_eq!(before.settings.len(), 2);
    let after = BTreeMap::from([
        (
            "appearance".to_owned(),
            setting(json!({"theme": "dark"}), UPDATED)?,
        ),
        ("units".to_owned(), setting(json!("metric"), UPDATED)?),
    ]);
    let committed = store
        .replace_settings(
            KEYS,
            before.clone(),
            after.clone(),
            CancellationToken::new(),
        )
        .await?;
    assert!(committed.changed);
    assert_eq!(
        committed.library_revision,
        before.library_revision.checked_next()?
    );
    let snapshot = store.settings_snapshot(KEYS).await?;
    assert_eq!(snapshot.settings, after);
    assert_eq!(snapshot.library_revision, committed.library_revision);
    assert!(
        store
            .get_setting::<Value>("locale".to_owned())
            .await?
            .is_none()
    );
    assert_eq!(excluded_rows(&connection)?, excluded);

    let cleared = store
        .replace_settings(KEYS, snapshot, BTreeMap::new(), CancellationToken::new())
        .await?;
    assert!(cleared.changed);
    assert_eq!(
        cleared.library_revision,
        committed.library_revision.checked_next()?
    );
    assert_eq!(
        store.settings_snapshot(KEYS).await?.settings,
        BTreeMap::new()
    );
    assert_eq!(excluded_rows(&connection)?, excluded);
    Ok(())
}

#[tokio::test]
async fn empty_scope_is_noop_without_removing_any_settings() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    let empty = store.settings_snapshot(&[]).await?;
    assert_eq!(empty.settings, BTreeMap::new());
    assert_eq!(empty.library_revision, before.library_revision);
    let committed = store
        .replace_settings(&[], empty, BTreeMap::new(), CancellationToken::new())
        .await?;
    assert!(!committed.changed);
    assert_eq!(committed.library_revision, before.library_revision);
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(reopened.settings_snapshot(KEYS).await?, before);
    Ok(())
}

#[tokio::test]
async fn timestamp_change_commits_once_and_identical_entries_do_not_write()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    let mut after = before.settings.clone();
    after
        .get_mut("appearance")
        .ok_or("missing appearance")?
        .updated_at = Rfc3339Timestamp::parse(UPDATED)?;
    let connection = Connection::open(&path)?;
    // An unchanged entry must not be rewritten even when another entry changes.
    connection.execute_batch(
        "CREATE TRIGGER protect_unchanged_locale BEFORE UPDATE ON app_settings
         WHEN OLD.key = 'locale' BEGIN SELECT RAISE(ABORT, 'unchanged row rewritten'); END;",
    )?;
    let committed = store
        .replace_settings(
            KEYS,
            before.clone(),
            after.clone(),
            CancellationToken::new(),
        )
        .await?;
    assert!(committed.changed);
    assert_eq!(
        committed.library_revision,
        before.library_revision.checked_next()?
    );
    let before_noop = store.settings_snapshot(KEYS).await?;
    assert_eq!(before_noop.settings, after);
    connection.execute_batch(
        "CREATE TRIGGER prevent_setting_insert BEFORE INSERT ON app_settings
             BEGIN SELECT RAISE(ABORT, 'unexpected insert'); END;
         CREATE TRIGGER prevent_setting_update BEFORE UPDATE ON app_settings
             BEGIN SELECT RAISE(ABORT, 'unexpected update'); END;
         CREATE TRIGGER prevent_setting_delete BEFORE DELETE ON app_settings
             BEGIN SELECT RAISE(ABORT, 'unexpected delete'); END;
         CREATE TRIGGER prevent_revision_update BEFORE UPDATE ON app_metadata
             BEGIN SELECT RAISE(ABORT, 'unexpected revision update'); END;",
    )?;
    let noop = store
        .replace_settings(KEYS, before_noop.clone(), after, CancellationToken::new())
        .await?;
    assert!(!noop.changed);
    assert_eq!(noop.library_revision, committed.library_revision);
    assert_eq!(store.settings_snapshot(KEYS).await?, before_noop);
    Ok(())
}

#[tokio::test]
async fn stale_revision_and_exact_before_state_are_rejected() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    store
        .set_setting(
            "excluded".to_owned(),
            json!(true),
            Rfc3339Timestamp::parse(UPDATED)?,
        )
        .await?;
    let current = store.settings_snapshot(KEYS).await?;
    assert!(matches!(
        store.replace_settings(KEYS, before.clone(), BTreeMap::new(), CancellationToken::new()).await,
        Err(StoreError::LibraryConflict { expected, actual })
            if expected == before.library_revision && actual == current.library_revision
    ));
    assert_eq!(store.settings_snapshot(KEYS).await?, current);
    // Model an external write that did not advance the revision. Timestamp drift alone is stale.
    Connection::open(&path)?.execute(
        "UPDATE app_settings SET updated_at = ?1 WHERE key = 'appearance'",
        [UPDATED],
    )?;
    let drifted = store.settings_snapshot(KEYS).await?;
    assert!(matches!(
        store
            .replace_settings(KEYS, current, BTreeMap::new(), CancellationToken::new())
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    assert_eq!(store.settings_snapshot(KEYS).await?, drifted);
    Ok(())
}

#[tokio::test]
async fn out_of_scope_before_and_after_entries_cannot_modify_settings() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let (store, _) = SqliteScenarioStore::open(directory.path().join("library.sqlite3")).await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    let excluded = setting(json!(true), CREATED)?;
    let mut invalid_before = before.clone();
    invalid_before
        .settings
        .insert("excluded".to_owned(), excluded.clone());
    assert!(matches!(
        store
            .replace_settings(
                KEYS,
                invalid_before,
                BTreeMap::new(),
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    assert!(matches!(
        store
            .replace_settings(
                KEYS,
                before.clone(),
                BTreeMap::from([("excluded".to_owned(), excluded)]),
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InvalidStagedApply(_))
    ));
    assert_eq!(store.settings_snapshot(KEYS).await?, before);
    assert!(
        store
            .get_setting::<Value>("excluded".to_owned())
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn revision_overflow_rolls_back_removal_and_upsert_but_allows_noop()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_settings(&store).await?;
    Connection::open(&path)?.execute(
        "UPDATE app_metadata SET value = ?1 WHERE key = 'portable_library_revision'",
        params![REVISION_MAX_V1.to_string()],
    )?;
    let before = store.settings_snapshot(KEYS).await?;
    let replacement = BTreeMap::from([("units".to_owned(), setting(json!("metric"), UPDATED)?)]);
    assert!(matches!(
        store
            .replace_settings(KEYS, before.clone(), replacement, CancellationToken::new())
            .await,
        Err(StoreError::NumericRange)
    ));
    assert_eq!(store.settings_snapshot(KEYS).await?, before);
    let noop = store
        .replace_settings(
            KEYS,
            before.clone(),
            before.settings.clone(),
            CancellationToken::new(),
        )
        .await?;
    assert!(!noop.changed);
    assert_eq!(noop.library_revision, Revision::new(REVISION_MAX_V1));
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    assert_eq!(reopened.settings_snapshot(KEYS).await?, before);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn after_write_failure_rolls_back_and_is_not_consumed_by_noop() -> Result<(), Box<dyn Error>>
{
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let (store, _) = SqliteScenarioStore::open(&path).await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    store.set_failpoint(Failpoint::AfterSettingsWrite)?;
    let noop = store
        .replace_settings(
            KEYS,
            before.clone(),
            before.settings.clone(),
            CancellationToken::new(),
        )
        .await?;
    assert!(!noop.changed);
    let replacement = BTreeMap::from([("units".to_owned(), setting(json!("metric"), UPDATED)?)]);
    assert!(matches!(
        store
            .replace_settings(
                KEYS,
                before.clone(),
                replacement.clone(),
                CancellationToken::new()
            )
            .await,
        Err(StoreError::InjectedFailure)
    ));
    assert_eq!(store.settings_snapshot(KEYS).await?, before);
    let committed = store
        .replace_settings(
            KEYS,
            before.clone(),
            replacement.clone(),
            CancellationToken::new(),
        )
        .await?;
    assert!(committed.changed);
    assert_eq!(
        committed.library_revision,
        before.library_revision.checked_next()?
    );
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let durable = reopened.settings_snapshot(KEYS).await?;
    assert_eq!(durable.settings, replacement);
    assert_eq!(durable.library_revision, committed.library_revision);
    Ok(())
}

#[tokio::test]
async fn cancellation_before_work_preserves_settings() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let (store, _) = SqliteScenarioStore::open(directory.path().join("library.sqlite3")).await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        store
            .replace_settings(KEYS, before.clone(), BTreeMap::new(), cancellation)
            .await,
        Err(StoreError::OperationCancelled)
    ));
    assert_eq!(store.settings_snapshot(KEYS).await?, before);
    Ok(())
}

#[cfg(debug_assertions)]
async fn cancellation_at_final_boundary(
    phase: CommandCommitTestPhase,
    change: bool,
) -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("library.sqlite3");
    let hook = CommandCommitTestHook::new(phase);
    let (store, _) = SqliteScenarioStore::open_with_options(
        &path,
        OpenOptions::default().with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    seed_settings(&store).await?;
    let before = store.settings_snapshot(KEYS).await?;
    let after = if change {
        BTreeMap::new()
    } else {
        before.settings.clone()
    };
    let cancellation = CancellationToken::new();
    let actor_store = store.clone();
    let actor_cancellation = cancellation.clone();
    let actor_before = before.clone();
    let actor_after = after.clone();
    let command = tokio::spawn(async move {
        actor_store
            .replace_settings(KEYS, actor_before, actor_after, actor_cancellation)
            .await
    });
    let wait_hook = hook.clone();
    tokio::task::spawn_blocking(move || wait_hook.wait_until_reached()).await?;
    cancellation.cancel();
    hook.release();
    let result = command.await?;
    let expected_revision = if phase == CommandCommitTestPhase::BeforeFinalCancellationCheck {
        assert!(matches!(result, Err(StoreError::OperationCancelled)));
        assert_eq!(store.settings_snapshot(KEYS).await?, before);
        before.library_revision
    } else {
        let committed = result?;
        assert_eq!(committed.changed, change);
        let expected = if change {
            before.library_revision.checked_next()?
        } else {
            before.library_revision
        };
        assert_eq!(committed.library_revision, expected);
        assert_eq!(store.settings_snapshot(KEYS).await?.settings, after);
        expected
    };
    drop(store);
    let (reopened, _) = SqliteScenarioStore::open(&path).await?;
    let durable = reopened.settings_snapshot(KEYS).await?;
    assert_eq!(durable.library_revision, expected_revision);
    let expected_settings = if phase == CommandCommitTestPhase::BeforeFinalCancellationCheck {
        before.settings
    } else {
        after
    };
    assert_eq!(durable.settings, expected_settings);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn final_cancellation_rolls_back_changes_and_cancels_noop() -> Result<(), Box<dyn Error>> {
    cancellation_at_final_boundary(CommandCommitTestPhase::BeforeFinalCancellationCheck, true)
        .await?;
    cancellation_at_final_boundary(CommandCommitTestPhase::BeforeFinalCancellationCheck, false)
        .await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn completion_wins_cancellation_after_final_check_for_changes_and_noop()
-> Result<(), Box<dyn Error>> {
    cancellation_at_final_boundary(CommandCommitTestPhase::AfterFinalCancellationCheck, true)
        .await?;
    cancellation_at_final_boundary(CommandCommitTestPhase::AfterFinalCancellationCheck, false).await
}
