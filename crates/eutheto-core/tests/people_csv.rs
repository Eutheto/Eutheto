#[path = "support/private_storage.rs"]
mod private_storage;

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    CsvApplyOutcomeV1, CsvApplyRequestV1, EuthetoApp, PeopleCsvPreviewDtoV1,
    PeopleCsvPreviewRequestV1,
};
use eutheto_import::{CollisionPlan, ImportOptions, RestoreMode};
use eutheto_store::HistoryEntry;
#[cfg(debug_assertions)]
use eutheto_store::{
    CommandCommitTestHook, CommandCommitTestPhase, OpenOptions, SnapshotPolicy, SqliteScenarioStore,
};
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandId, CommandSource, DomainPackRef, FixedClock,
    FixedMonotonicClock, GapPolicy, Horizon, OverlapPolicy, PersonId, RequestId, Revision,
    ScenarioId, ScenarioSettings, ScenarioViewDto, SystemIdGenerator, UnitSystem,
};
#[cfg(debug_assertions)]
use eutheto_types::{EventPayload, EventTopic};
use eutheto_workforce::{
    model::{ActiveRange, WorkloadWeight},
    people_csv::{
        BlankPolicy, ColumnMapping, CsvDialect, IdentityDecision, MAX_CSV_SOURCE_BYTES,
        NewPersonDefaults, PeopleCsvMapping, PeopleImportDisposition, PeopleRowStatus, PersonField,
        RowDecision, RowRejectionCode,
    },
};
use std::{
    collections::BTreeMap,
    error::Error,
    io::{self, Cursor, Read},
    sync::{Arc, Barrier},
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;
trait Boxed<T> {
    fn boxed(self) -> Result<T, Box<dyn Error>>;
}
impl<T> Boxed<T> for Result<T, AppError> {
    fn boxed(self) -> Result<T, Box<dyn Error>> {
        self.map_err(|error| format!("{error:?}").into())
    }
}

fn directory() -> Result<TempDir, Box<dyn Error>> {
    Ok(private_storage::tempdir("eutheto-csv-test-")?)
}
fn dependencies(directory: &TempDir) -> Result<AppDependencies, Box<dyn Error>> {
    Ok(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("library.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new("2026-01-10T12:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    })
}
fn request_id() -> Result<RequestId, Box<dyn Error>> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}
async fn create(app: &EuthetoApp) -> Result<ScenarioId, Box<dyn Error>> {
    let result = app
        .execute(AppCommand::CreateProject {
            request_id: request_id()?,
            title: "CSV review".to_owned(),
            description: String::new(),
            domain_pack: DomainPackRef {
                id: "official.workforce".parse()?,
                schema_version: 1,
            },
            settings: ScenarioSettings {
                time_zone: "UTC".parse()?,
                locale: "en-US".parse()?,
                units: UnitSystem::Metric,
                horizon: Horizon::new(
                    "2026-01-01T00:00:00Z".parse()?,
                    "2026-01-08T00:00:00Z".parse()?,
                )?,
                gap_policy: GapPolicy::Reject,
                overlap_policy: OverlapPolicy::Earlier,
            },
        })
        .await
        .boxed()?;
    match result {
        AppCommandResult::Project(project) => Ok(project.scenario_id),
        _ => Err("wrong create receipt".into()),
    }
}
fn mapping() -> PeopleCsvMapping {
    PeopleCsvMapping {
        dialect: CsvDialect::Comma,
        has_header: true,
        expected_columns: 2,
        columns: vec![
            ColumnMapping {
                index: 0,
                field: PersonField::ExternalId,
                blank: BlankPolicy::Preserve,
            },
            ColumnMapping {
                index: 1,
                field: PersonField::Name,
                blank: BlankPolicy::Preserve,
            },
        ],
        new_person_defaults: NewPersonDefaults {
            active_range: ActiveRange::Always {},
            qualification_grants: Vec::new(),
            eligible_assignment_type_ids: Vec::new(),
            home_location_id: None,
            workload_weight: WorkloadWeight {
                numerator: 1,
                denominator: 1,
            },
            workload_target: None,
            tags: Vec::new(),
            team_ids: Vec::new(),
            display: None,
        },
        reference_mappings: BTreeMap::new(),
    }
}
fn add(record: u32) -> Result<RowDecision, Box<dyn Error>> {
    Ok(RowDecision {
        record,
        decision: IdentityDecision::Add {
            person_id: PersonId::new(&SystemIdGenerator)?,
        },
    })
}
fn preview_request(
    id: ScenarioId,
    revision: Revision,
    decisions: Vec<RowDecision>,
) -> PeopleCsvPreviewRequestV1 {
    PeopleCsvPreviewRequestV1 {
        schema_version: 1,
        scenario_id: id,
        expected_revision: revision,
        mapping: mapping(),
        decisions,
    }
}
async fn preview(
    app: &EuthetoApp,
    id: ScenarioId,
    revision: Revision,
    bytes: &[u8],
    decisions: Vec<RowDecision>,
) -> Result<PeopleCsvPreviewDtoV1, Box<dyn Error>> {
    app.preview_people_csv(
        preview_request(id, revision, decisions),
        Cursor::new(bytes.to_vec()),
        app.csv_operation(),
    )
    .await
    .boxed()
}
fn apply_request(
    id: ScenarioId,
    revision: Revision,
    preview: &PeopleCsvPreviewDtoV1,
) -> Result<CsvApplyRequestV1, Box<dyn Error>> {
    Ok(CsvApplyRequestV1 {
        schema_version: 1,
        scenario_id: id,
        expected_revision: revision,
        preview_id: preview.preview_id,
        approved_digest: preview
            .preview
            .approval_digest
            .clone()
            .unwrap_or_else(|| "0".repeat(64)),
        command_id: CommandId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
        actor: ActorRef {
            actor_id: None,
            display_name: "CSV integration".to_owned(),
        },
        source: CommandSource::System,
        truncate_redo: false,
    })
}
async fn view(app: &EuthetoApp, id: ScenarioId) -> Result<ScenarioViewDto, Box<dyn Error>> {
    match app.query(AppQuery::ScenarioView(id)).await.boxed()? {
        AppQueryResult::Scenario(view) => Ok(*view),
        _ => Err("wrong scenario receipt".into()),
    }
}
async fn history(app: &EuthetoApp, id: ScenarioId) -> Result<Vec<HistoryEntry>, Box<dyn Error>> {
    match app.query(AppQuery::History(id)).await.boxed()? {
        AppQueryResult::History(history) => Ok(history),
        _ => Err("wrong history receipt".into()),
    }
}
fn assert_code<T>(result: Result<T, AppError>, code: &str) {
    let actual = match result {
        Err(AppError::Protocol(error)) => Some(error.code),
        _ => None,
    };
    assert_eq!(actual.as_deref(), Some(code));
}
const SOURCE: &[u8] = b"external,name\nstaff-01,River\nbroken\nstaff-02,Alex\n";

fn assert_partial_preview(review: &PeopleCsvPreviewDtoV1) {
    let statuses: Vec<_> = review.preview.rows.iter().map(|row| row.status).collect();
    assert_eq!(
        statuses,
        [
            PeopleRowStatus::Added,
            PeopleRowStatus::Rejected,
            PeopleRowStatus::Added,
        ]
    );
    assert_eq!(
        review.preview.rejected_rows[0].code,
        RowRejectionCode::ColumnCount
    );
}

fn assert_reviewed_person(view: &ScenarioViewDto, person_id: PersonId, name: &str) -> TestResult {
    let entity_id = eutheto_types::EntityId::from_uuid(person_id.as_uuid());
    assert_eq!(view.document.domain.entities.len(), 1);
    let person = view
        .document
        .domain
        .entities
        .get(&entity_id)
        .ok_or("reviewed person")?;
    assert_eq!(person["id"], serde_json::json!(person_id));
    assert_eq!(person["name"], name);
    assert_eq!(person["tags"], serde_json::json!(["reviewed"]));
    Ok(())
}

#[tokio::test]
async fn preview_is_read_only_partial_import_is_one_durable_undoable_command() -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    let before = view(&app, id).await?;
    let review = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    assert_partial_preview(&review);
    assert_eq!(view(&app, id).await?.document, before.document);
    assert_eq!(view(&app, id).await?.revision, Revision::INITIAL);
    assert!(history(&app, id).await?.is_empty());
    let retained = app.csv_report(review.preview_id).await.boxed()?;
    assert!(!retained.consumed);
    let request = apply_request(id, Revision::INITIAL, &review)?;
    let command_id = request.command_id;
    let result = app
        .apply_csv_import(request.clone(), Cursor::new(SOURCE), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        result.outcome,
        CsvApplyOutcomeV1::Applied {
            command_id,
            revision: Revision::new(1)
        }
    );
    assert_eq!(result.report.rows, retained.rows);
    assert!(result.report.consumed);
    assert_eq!(
        app.csv_report(review.preview_id).await.boxed()?,
        result.report
    );
    assert_code(
        app.apply_csv_import(request, Cursor::new(SOURCE), app.csv_operation())
            .await,
        "people_csv.preview_consumed",
    );
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, command_id);
    assert_eq!(entries[0].command["type"], "applyBatch");
    assert_eq!(
        entries[0].command["payload"]["commands"]
            .as_array()
            .ok_or("batch commands")?
            .len(),
        2
    );
    let applied_document = view(&app, id).await?.document;
    drop(app);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_code(
        reopened.csv_report(review.preview_id).await,
        "people_csv.preview_unavailable",
    );
    assert_eq!(view(&reopened, id).await?.document, applied_document);
    assert_eq!(history(&reopened, id).await?, entries);
    reopened
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id: id,
            expected_revision: Revision::new(1),
        })
        .await
        .boxed()?;
    assert_eq!(view(&reopened, id).await?.document, before.document);
    let undone_history = history(&reopened, id).await?;
    assert_eq!(undone_history.len(), 1);
    assert_eq!(undone_history[0].id, entries[0].id);
    assert_eq!(undone_history[0].command, entries[0].command);
    assert_eq!(undone_history[0].inverse, entries[0].inverse);
    assert!(!undone_history[0].applied);
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id: id,
            expected_revision: Revision::new(2),
        })
        .await
        .boxed()?;
    assert_eq!(view(&reopened, id).await?.document, applied_document);
    assert_eq!(history(&reopened, id).await?, entries);
    Ok(())
}

#[tokio::test]
async fn blocked_reviews_have_no_authority_and_no_changes_has_no_history() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let source = b"external,name\none,New person\n";
    let blocked = preview(&app, id, Revision::INITIAL, source, vec![]).await?;
    assert_eq!(
        blocked.preview.disposition,
        PeopleImportDisposition::Blocked
    );
    assert!(blocked.preview.approval_digest.is_none());
    assert_code(
        app.apply_csv_import(
            apply_request(id, Revision::INITIAL, &blocked)?,
            Cursor::new(source),
            app.csv_operation(),
        )
        .await,
        "people_csv.preview_blocked",
    );
    let no_changes = preview(
        &app,
        id,
        Revision::INITIAL,
        source,
        vec![RowDecision {
            record: 2,
            decision: IdentityDecision::Skip,
        }],
    )
    .await?;
    let result = app
        .apply_csv_import(
            apply_request(id, Revision::INITIAL, &no_changes)?,
            Cursor::new(source),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        result.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::INITIAL
        }
    );
    assert!(result.report.consumed);
    assert!(history(&app, id).await?.is_empty());
    assert_eq!(view(&app, id).await?.revision, Revision::INITIAL);
    assert!(view(&app, id).await?.document.domain.entities.is_empty());
    Ok(())
}

#[tokio::test]
async fn approval_source_and_changed_mapping_or_decisions_require_repreview() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let source = b"external,name\none,River\n";
    let decision = add(2)?;
    let IdentityDecision::Add { person_id } = decision.decision else {
        return Err("expected an explicit add decision".into());
    };
    let first = preview(&app, id, Revision::INITIAL, source, vec![decision]).await?;
    let mut request = apply_request(id, Revision::INITIAL, &first)?;
    request.approved_digest = "0".repeat(64);
    assert_code(
        app.apply_csv_import(request, Cursor::new(source), app.csv_operation())
            .await,
        "people_csv.approval_mismatch",
    );
    assert_code(
        app.apply_csv_import(
            apply_request(id, Revision::INITIAL, &first)?,
            Cursor::new(b"external,name\none,Changed\n"),
            app.csv_operation(),
        )
        .await,
        "people_csv.staleReview",
    );
    let mut changed_request = preview_request(id, Revision::INITIAL, vec![decision]);
    changed_request
        .mapping
        .new_person_defaults
        .tags
        .push("reviewed".to_owned());
    let changed_mapping = app
        .preview_people_csv(changed_request, Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    let changed_decision = preview(&app, id, Revision::INITIAL, source, vec![add(2)?]).await?;
    for changed in [&changed_mapping, &changed_decision] {
        let mut request = apply_request(id, Revision::INITIAL, changed)?;
        request.approved_digest = first.preview.approval_digest.clone().ok_or("approval")?;
        assert_code(
            app.apply_csv_import(request, Cursor::new(source), app.csv_operation())
                .await,
            "people_csv.approval_mismatch",
        );
    }
    assert!(history(&app, id).await?.is_empty());
    let receipt = app
        .apply_csv_import(
            apply_request(id, Revision::INITIAL, &changed_mapping)?,
            Cursor::new(source),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert!(matches!(receipt.outcome, CsvApplyOutcomeV1::Applied { .. }));
    let applied = view(&app, id).await?;
    assert_reviewed_person(&applied, person_id, "River")?;
    // Exact external-ID matching updates the existing person without an identity decision.
    let update = preview(
        &app,
        id,
        Revision::new(1),
        b"external,name\none,Revised\n",
        vec![],
    )
    .await?;
    assert_eq!(update.preview.rows[0].status, PeopleRowStatus::Updated);
    app.apply_csv_import(
        apply_request(id, Revision::new(1), &update)?,
        Cursor::new(b"external,name\none,Revised\n"),
        app.csv_operation(),
    )
    .await
    .boxed()?;
    let updated = view(&app, id).await?;
    assert_reviewed_person(&updated, person_id, "Revised")?;
    Ok(())
}

struct PausedReader {
    input: Cursor<Vec<u8>>,
    reached: Arc<Barrier>,
    release: Arc<Barrier>,
    paused: bool,
}
impl Read for PausedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if !self.paused {
            self.paused = true;
            self.reached.wait();
            self.release.wait();
        }
        self.input.read(bytes)
    }
}
fn paused_reader(source: &[u8]) -> (PausedReader, Arc<Barrier>, Arc<Barrier>) {
    let reached = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    (
        PausedReader {
            input: Cursor::new(source.to_vec()),
            reached: reached.clone(),
            release: release.clone(),
            paused: false,
        },
        reached,
        release,
    )
}
async fn barrier(barrier: Arc<Barrier>) -> Result<(), Box<dyn Error>> {
    tokio::task::spawn_blocking(move || {
        barrier.wait();
    })
    .await?;
    Ok(())
}

async fn revision_race(no_changes: bool) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    let source = b"external,name\none,River\n";
    let decisions = if no_changes {
        vec![RowDecision {
            record: 2,
            decision: IdentityDecision::Skip,
        }]
    } else {
        vec![add(2)?]
    };
    let review = preview(&app, id, Revision::INITIAL, source, decisions).await?;
    let request = apply_request(id, Revision::INITIAL, &review)?;
    let second = EuthetoApp::open(deps).await.boxed()?;
    let concurrent = preview(&second, id, Revision::INITIAL, source, vec![add(2)?]).await?;
    let (reader, reached, release) = paused_reader(source);
    let caller_app = app.clone();
    let operation = app.csv_operation();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, reader, operation)
            .await
    });
    barrier(reached).await?;
    second
        .apply_csv_import(
            apply_request(id, Revision::INITIAL, &concurrent)?,
            Cursor::new(source),
            second.csv_operation(),
        )
        .await
        .boxed()?;
    barrier(release).await?;
    assert!(
        matches!(caller.await?, Err(AppError::Conflict { expected_revision, actual_revision })
        if expected_revision == Revision::INITIAL && actual_revision == Revision::new(1))
    );
    assert_eq!(history(&app, id).await?.len(), 1);
    assert!(!app.csv_report(review.preview_id).await.boxed()?.consumed);
    Ok(())
}
#[tokio::test]
async fn second_store_revision_race_is_rechecked_at_actual_write() -> TestResult {
    revision_race(false).await
}
#[tokio::test]
async fn no_changes_rechecks_revision_after_rebuild() -> TestResult {
    revision_race(true).await
}

#[tokio::test]
async fn redo_branch_truncation_requires_explicit_csv_confirmation() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let source = b"external,name\none,River\n";
    let first = preview(&app, id, Revision::INITIAL, source, vec![add(2)?]).await?;
    app.apply_csv_import(
        apply_request(id, Revision::INITIAL, &first)?,
        Cursor::new(source),
        app.csv_operation(),
    )
    .await
    .boxed()?;
    app.execute(AppCommand::Undo {
        request_id: request_id()?,
        scenario_id: id,
        expected_revision: Revision::new(1),
    })
    .await
    .boxed()?;
    let next = preview(&app, id, Revision::new(2), source, vec![add(2)?]).await?;
    let mut request = apply_request(id, Revision::new(2), &next)?;
    assert!(
        matches!(app.apply_csv_import(request.clone(), Cursor::new(source), app.csv_operation()).await,
        Err(AppError::Validation(report)) if report.issues.iter().any(|issue| issue.code == "history.redo_branch_requires_truncation"))
    );
    assert!(!history(&app, id).await?[0].applied);
    request.truncate_redo = true;
    app.apply_csv_import(request, Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(history(&app, id).await?.len(), 1);
    assert!(history(&app, id).await?[0].applied);
    assert_eq!(view(&app, id).await?.revision, Revision::new(3));
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "Keeps cross-kind capacity, eviction, and discard transitions in one scenario."
)]
#[tokio::test]
async fn shared_preview_count_kind_isolation_and_discard_preserve_other_capabilities() -> TestResult
{
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let csv = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let AppQueryResult::Bundle { bytes, .. } = app
        .query(AppQuery::ExportScenario {
            scenario_id: id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    else {
        return Err("export receipt".into());
    };
    let AppQueryResult::PortablePreview {
        preview_id: portable,
        ..
    } = app
        .query(AppQuery::PreviewImport {
            cancellation: app.setup_cancellation(),
            bytes: bytes.clone(),
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await
        .boxed()?
    else {
        return Err("preview receipt".into());
    };
    let AppQueryResult::UnopenedBundlePreview {
        preview_id: opaque, ..
    } = app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes,
        })
        .await
        .boxed()?
    else {
        return Err("opaque receipt".into());
    };
    for preview_id in [portable, opaque] {
        assert_code(
            app.discard_csv_preview(preview_id).await,
            "people_csv.preview_kind_mismatch",
        );
        assert_code(
            app.csv_report(preview_id).await,
            "people_csv.preview_kind_mismatch",
        );
    }
    assert_code(
        app.execute(AppCommand::CancelPortablePreview {
            preview_id: csv.preview_id,
        })
        .await,
        "portable.preview_capability_mismatch",
    );
    assert_code(
        app.execute(AppCommand::ApplyImport {
            cancellation: app.setup_cancellation(),
            request_id: request_id()?,
            preview_id: csv.preview_id,
            collision_plan: CollisionPlan::default(),
        })
        .await,
        "portable.preview_capability_mismatch",
    );
    assert_code(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            cancellation: app.setup_cancellation(),
            preview_id: csv.preview_id,
            destination: directory.path().join("wrong.zip"),
        })
        .await,
        "portable.preview_capability_mismatch",
    );
    assert!(!app.csv_report(csv.preview_id).await.boxed()?.consumed);
    // Three different kinds share one three-entry cache; the fourth evicts the oldest ID.
    let fourth = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    assert_code(
        app.csv_report(csv.preview_id).await,
        "people_csv.preview_unavailable",
    );
    app.execute(AppCommand::CancelPortablePreview {
        preview_id: portable,
    })
    .await
    .boxed()?;
    app.execute(AppCommand::CancelPortablePreview { preview_id: opaque })
        .await
        .boxed()?;
    app.discard_csv_preview(fourth.preview_id).await.boxed()?;
    assert_code(
        app.csv_report(fourth.preview_id).await,
        "people_csv.preview_unavailable",
    );
    assert!(!directory.path().join("wrong.zip").exists());
    Ok(())
}

#[tokio::test]
async fn request_cancellation_is_isolated_and_dropped_guard_signals_only_its_child() -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let root = deps.cancellation.clone();
    let app = EuthetoApp::open(deps).await.boxed()?;
    let id = create(&app).await?;
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let sibling = app.csv_operation();
    signal.cancel();
    assert_code(
        app.preview_people_csv(
            preview_request(id, Revision::INITIAL, vec![]),
            Cursor::new(SOURCE),
            operation,
        )
        .await,
        "operation.cancelled",
    );
    assert!(!root.is_cancelled());
    assert!(!sibling.cancellation().is_cancelled());
    app.detect_people_csv(Cursor::new(SOURCE), sibling)
        .await
        .boxed()?;
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let future = app.detect_people_csv(Cursor::new(SOURCE), operation);
    drop(future);
    assert!(signal.is_cancelled());
    assert!(!root.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn native_parent_cancels_csv_without_child_completion_cancelling_siblings() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let parent = CancellationToken::new();
    let sibling = eutheto_core::CsvOperation::child_of(&parent);
    app.detect_people_csv(
        Cursor::new(SOURCE),
        eutheto_core::CsvOperation::child_of(&parent),
    )
    .await
    .boxed()?;
    assert!(!parent.is_cancelled());
    app.detect_people_csv(Cursor::new(SOURCE), sibling)
        .await
        .boxed()?;
    let cancelled = eutheto_core::CsvOperation::child_of(&parent);
    parent.cancel();
    assert_code(
        app.detect_people_csv(Cursor::new(SOURCE), cancelled).await,
        "operation.cancelled",
    );
    app.detect_people_csv(Cursor::new(SOURCE), app.csv_operation())
        .await
        .boxed()?;
    Ok(())
}

struct CountedOverflow {
    read: Arc<std::sync::atomic::AtomicUsize>,
}
impl Read for CountedOverflow {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        bytes.fill(b'a');
        self.read
            .fetch_add(bytes.len(), std::sync::atomic::Ordering::SeqCst);
        Ok(bytes.len())
    }
}
#[tokio::test]
async fn detection_stops_at_source_cap_plus_one_and_sanitizes_reader_errors() -> TestResult {
    struct Failure;
    impl Read for Failure {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("PRIVATE_PATH_AND_CELL"))
        }
    }
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    assert_code(
        app.detect_people_csv(CountedOverflow { read: read.clone() }, app.csv_operation())
            .await,
        "people_csv.sourceByteLimit",
    );
    assert_eq!(
        read.load(std::sync::atomic::Ordering::SeqCst),
        MAX_CSV_SOURCE_BYTES + 1
    );
    let result = app.detect_people_csv(Failure, app.csv_operation()).await;
    assert!(!format!("{result:?}").contains("PRIVATE_PATH_AND_CELL"));
    assert_code(result, "people_csv.io");
    Ok(())
}

#[cfg(debug_assertions)]
async fn commit_boundary(
    phase: CommandCommitTestPhase,
    abandon: bool,
    discard: bool,
) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let hook = CommandCommitTestHook::new(phase);
    let (store, initialized) = SqliteScenarioStore::open_with_options(
        &deps.paths.database,
        OpenOptions::new(SnapshotPolicy::default()).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let store = Arc::new(store);
    let app =
        EuthetoApp::from_initialized_store(store.clone(), initialized, deps.clone()).boxed()?;
    let id = create(&app).await?;
    let before = store
        .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
        .await?;
    let review = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let request = apply_request(id, Revision::INITIAL, &review)?;
    let command_id = request.command_id;
    let event_request = request.request_id;
    let mut events = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, Cursor::new(SOURCE), operation)
            .await
    });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    if abandon {
        caller.abort();
    } else {
        signal.cancel();
    }
    if discard {
        app.discard_csv_preview(review.preview_id).await.boxed()?;
    }
    let abandoned = if abandon {
        assert!(caller.await.is_err_and(|error| error.is_cancelled()));
        None
    } else {
        Some(caller)
    };
    let release = hook.clone();
    tokio::task::spawn_blocking(move || release.release()).await?;
    let commit_wins = phase == CommandCommitTestPhase::AfterFinalCancellationCheck;
    if let Some(caller) = abandoned {
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), caller).await??;
        if commit_wins {
            assert_eq!(
                result.boxed()?.outcome,
                CsvApplyOutcomeV1::Applied {
                    command_id,
                    revision: Revision::new(1)
                }
            );
        } else {
            assert_code(result, "operation.cancelled");
        }
    }
    if commit_wins {
        assert_committed_csv(
            &app,
            &mut events,
            id,
            event_request,
            command_id,
            review.preview_id,
            discard,
        )
        .await?;
    } else {
        let after = store
            .library_snapshot(eutheto_store::NativeArchivalSelection::Exclude)
            .await?;
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            after.scenario_revision_high_water,
            before.scenario_revision_high_water
        );
        assert_eq!(
            after.scenario_identity_owners,
            before.scenario_identity_owners
        );
        assert_eq!(after.projects[0].document, before.projects[0].document);
        assert!(store.history(id).await?.is_empty());
        assert!(!app.csv_report(review.preview_id).await.boxed()?.consumed);
    }
    drop(app);
    drop(store);
    assert_reopened_csv_boundary(deps, id, commit_wins).await
}

#[cfg(debug_assertions)]
async fn assert_committed_csv(
    app: &EuthetoApp,
    events: &mut eutheto_core::EventSubscription,
    id: ScenarioId,
    event_request: RequestId,
    command_id: CommandId,
    preview_id: RequestId,
    discard: bool,
) -> TestResult {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let event = events.recv().await?;
        assert!(
            matches!(event.payload, EventPayload::ScenarioChanged { context, .. }
            if context.request_id == Some(event_request) && context.scenario_id == Some(id))
        );
        // The committed owner publishes first, then consumes its retained review.
        if discard {
            assert_code(
                app.csv_report(preview_id).await,
                "people_csv.preview_unavailable",
            );
        } else {
            loop {
                if app.csv_report(preview_id).await?.consumed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        Ok::<(), AppError>(())
    })
    .await?
    .boxed()?;
    assert_eq!(view(app, id).await?.revision, Revision::new(1));
    let entries = history(app, id).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, command_id);
    Ok(())
}

#[cfg(debug_assertions)]
async fn assert_reopened_csv_boundary(
    dependencies: AppDependencies,
    id: ScenarioId,
    committed: bool,
) -> TestResult {
    let reopened = EuthetoApp::open(dependencies).await.boxed()?;
    let expected_revision = if committed {
        Revision::new(1)
    } else {
        Revision::INITIAL
    };
    assert_eq!(view(&reopened, id).await?.revision, expected_revision);
    assert_eq!(history(&reopened, id).await?.len(), usize::from(committed));
    Ok(())
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn cancellation_before_final_check_rolls_back_without_consuming_review() -> TestResult {
    commit_boundary(
        CommandCommitTestPhase::BeforeFinalCancellationCheck,
        false,
        false,
    )
    .await
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn cancellation_after_final_check_awaits_committed_receipt_event_and_consumption()
-> TestResult {
    commit_boundary(
        CommandCommitTestPhase::AfterFinalCancellationCheck,
        false,
        false,
    )
    .await
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn abandoned_caller_after_final_check_still_commits_publishes_and_consumes() -> TestResult {
    commit_boundary(
        CommandCommitTestPhase::AfterFinalCancellationCheck,
        true,
        false,
    )
    .await
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn discarded_cache_after_final_check_does_not_replace_success_or_reinsert_report()
-> TestResult {
    commit_boundary(
        CommandCommitTestPhase::AfterFinalCancellationCheck,
        false,
        true,
    )
    .await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn abandoned_caller_before_final_check_rolls_back_without_consuming_review() -> TestResult {
    commit_boundary(
        CommandCommitTestPhase::BeforeFinalCancellationCheck,
        true,
        false,
    )
    .await
}

#[tokio::test]
async fn dropped_inflight_reader_cancels_without_affecting_a_sibling() -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let root = deps.cancellation.clone();
    let app = EuthetoApp::open(deps).await.boxed()?;
    let (reader, reached, release) = paused_reader(SOURCE);
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move { caller_app.detect_people_csv(reader, operation).await });
    barrier(reached).await?;
    caller.abort();
    assert!(caller.await.is_err_and(|error| error.is_cancelled()));
    assert!(signal.is_cancelled());
    assert!(!root.is_cancelled());
    app.detect_people_csv(Cursor::new(SOURCE), app.csv_operation())
        .await
        .boxed()?;
    barrier(release).await?;
    Ok(())
}

fn large_opaque_bundle(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    use eutheto_export::{CHECKSUMS_PATH, Checksums, canonical_json, sha256_hex};
    use std::io::Write;
    use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        entries.insert(entry.name()?.into_owned(), contents);
    }
    let mut checksums: Checksums =
        serde_json::from_slice(entries.get(CHECKSUMS_PATH).ok_or("missing checksums")?)?;
    // Opaque intake verifies bytes, not domain semantics. Four stored entries keep
    // individual and total entry ceilings valid while retaining over half the cache.
    let payload = vec![b'x'; 9 * 1024 * 1024];
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o600);
    for index in 0..4 {
        let path = format!("assets/opaque-{index}.bin");
        checksums.files.insert(path.clone(), sha256_hex(&payload));
        writer.start_file(path, options)?;
        writer.write_all(&payload)?;
    }
    entries.insert(CHECKSUMS_PATH.to_owned(), canonical_json(&checksums)?);
    for (path, content) in entries {
        writer.start_file(path, options)?;
        writer.write_all(&content)?;
    }
    Ok(writer.finish()?.into_inner())
}

#[tokio::test]
async fn shared_byte_budget_evicts_csv_and_portable_before_the_count_limit() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let csv = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let bytes = match app
        .query(AppQuery::ExportScenario {
            scenario_id: id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    {
        AppQueryResult::Bundle { bytes, .. } => large_opaque_bundle(&bytes)?,
        _ => return Err("wrong export receipt".into()),
    };
    let AppQueryResult::UnopenedBundlePreview {
        preview_id: first, ..
    } = app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes: bytes.clone(),
        })
        .await
        .boxed()?
    else {
        return Err("wrong opaque receipt".into());
    };
    assert!(!app.csv_report(csv.preview_id).await.boxed()?.consumed);
    // Only two entries existed before this call. Count alone cannot explain eviction.
    let AppQueryResult::UnopenedBundlePreview {
        preview_id: second, ..
    } = app
        .query(AppQuery::InspectUnopenedBundle {
            cancellation: app.setup_cancellation(),
            bytes,
        })
        .await
        .boxed()?
    else {
        return Err("wrong opaque receipt".into());
    };
    assert_code(
        app.csv_report(csv.preview_id).await,
        "people_csv.preview_unavailable",
    );
    assert_code(
        app.execute(AppCommand::CancelPortablePreview { preview_id: first })
            .await,
        "portable.preview_not_found",
    );
    app.execute(AppCommand::CancelPortablePreview { preview_id: second })
        .await
        .boxed()?;
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn serialized_csv_requests_reject_unknown_authority_fields_and_versions() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let preview = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let request = apply_request(id, Revision::INITIAL, &preview)?;
    let mut forged = serde_json::to_value(&request)?;
    forged["batch"] = serde_json::json!({"commands": []});
    assert!(serde_json::from_value::<CsvApplyRequestV1>(forged).is_err());
    let mut unsupported = request;
    unsupported.schema_version = 2;
    assert_code(
        app.apply_csv_import(unsupported, Cursor::new(SOURCE), app.csv_operation())
            .await,
        "people_csv.unsupportedVersion",
    );
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn concurrent_apply_of_one_review_has_exactly_one_revision_winner() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let preview = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let reached = Arc::new(Barrier::new(3));
    let release = Arc::new(Barrier::new(3));
    let mut callers = Vec::new();
    for _ in 0..2 {
        let reader = PausedReader {
            input: Cursor::new(SOURCE.to_vec()),
            reached: reached.clone(),
            release: release.clone(),
            paused: false,
        };
        let request = apply_request(id, Revision::INITIAL, &preview)?;
        let operation = app.csv_operation();
        let caller_app = app.clone();
        callers.push(tokio::spawn(async move {
            caller_app
                .apply_csv_import(request, reader, operation)
                .await
        }));
    }
    barrier(reached).await?;
    barrier(release).await?;
    let mut winners = Vec::new();
    let mut conflicts = 0;
    for caller in callers {
        match caller.await? {
            Ok(receipt) => winners.push(receipt.outcome),
            Err(AppError::Conflict {
                expected_revision,
                actual_revision,
            }) => {
                assert_eq!(expected_revision, Revision::INITIAL);
                assert_eq!(actual_revision, Revision::new(1));
                conflicts += 1;
            }
            Err(error) => return Err(format!("unexpected apply outcome: {error:?}").into()),
        }
    }
    assert_eq!(conflicts, 1);
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        winners,
        [CsvApplyOutcomeV1::Applied {
            command_id: entries[0].id,
            revision: Revision::new(1)
        }]
    );
    assert!(app.csv_report(preview.preview_id).await.boxed()?.consumed);
    Ok(())
}

#[tokio::test]
async fn csv_policy_error_retains_safe_record_location() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let request = preview_request(
        id,
        Revision::INITIAL,
        vec![RowDecision {
            record: 1,
            decision: IdentityDecision::Skip,
        }],
    );
    let failure = app
        .preview_people_csv(
            request,
            Cursor::new(b"PRIVATE_HEADER,PRIVATE_CELL\none,River\n"),
            app.csv_operation(),
        )
        .await;
    assert!(!format!("{failure:?}").contains("PRIVATE_"));
    let Err(AppError::Validation(report)) = failure else {
        return Err("expected a located CSV policy failure".into());
    };
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].code, "people_csv.invalidDecision");
    assert_eq!(
        report.issues[0].field_path.as_deref(),
        Some("/peopleCsv/records/1")
    );
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn streaming_reader_cancellation_uses_ordinary_operation_taxonomy() -> TestResult {
    struct CancellingReader(eutheto_core::CsvCancellation);
    impl Read for CancellingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.cancel();
            Ok(0)
        }
    }
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let operation = app.csv_operation();
    let reader = CancellingReader(operation.cancellation());
    assert_code(
        app.preview_people_csv(
            preview_request(id, Revision::INITIAL, vec![]),
            reader,
            operation,
        )
        .await,
        "operation.cancelled",
    );
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn concurrent_no_change_apply_consumes_review_once() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let decisions = [2, 4]
        .into_iter()
        .map(|record| RowDecision {
            record,
            decision: IdentityDecision::Skip,
        })
        .collect();
    let review = preview(&app, id, Revision::INITIAL, SOURCE, decisions).await?;
    let reached = Arc::new(Barrier::new(3));
    let release = Arc::new(Barrier::new(3));
    let mut callers = Vec::new();
    for _ in 0..2 {
        let reader = PausedReader {
            input: Cursor::new(SOURCE.to_vec()),
            reached: reached.clone(),
            release: release.clone(),
            paused: false,
        };
        let request = apply_request(id, Revision::INITIAL, &review)?;
        let operation = app.csv_operation();
        let caller_app = app.clone();
        callers.push(tokio::spawn(async move {
            caller_app
                .apply_csv_import(request, reader, operation)
                .await
        }));
    }
    barrier(reached).await?;
    barrier(release).await?;
    let mut successes = 0;
    for caller in callers {
        match caller.await? {
            Ok(receipt) => {
                assert_eq!(
                    receipt.outcome,
                    CsvApplyOutcomeV1::NoChanges {
                        revision: Revision::INITIAL
                    }
                );
                successes += 1;
            }
            Err(AppError::Protocol(error)) if error.code == "people_csv.preview_consumed" => {}
            Err(error) => return Err(format!("unexpected no-change outcome: {error:?}").into()),
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(view(&app, id).await?.revision, Revision::INITIAL);
    assert!(history(&app, id).await?.is_empty());
    assert!(app.csv_report(review.preview_id).await.boxed()?.consumed);
    Ok(())
}

struct RepeatedPreviewIds {
    repeat: std::sync::atomic::AtomicBool,
    id: uuid::Uuid,
}

impl eutheto_types::IdGenerator for RepeatedPreviewIds {
    fn next_uuid(&self) -> Result<uuid::Uuid, eutheto_types::IdGenerationError> {
        if self.repeat.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(self.id)
        } else {
            eutheto_types::IdGenerator::next_uuid(&SystemIdGenerator)
        }
    }
}

async fn colliding_portable_preview_keeps_csv(opaque: bool) -> TestResult {
    let directory = directory()?;
    let ids = Arc::new(RepeatedPreviewIds {
        repeat: std::sync::atomic::AtomicBool::new(false),
        id: "0195a5e4-7c00-7000-8000-000000000999".parse()?,
    });
    let mut deps = dependencies(&directory)?;
    deps.ids = ids.clone();
    let app = EuthetoApp::open(deps).await.boxed()?;
    let id = create(&app).await?;
    let AppQueryResult::Bundle { bytes, .. } = app
        .query(AppQuery::ExportScenario {
            scenario_id: id,
            cancellation: app.setup_cancellation(),
        })
        .await
        .boxed()?
    else {
        return Err("expected portable source".into());
    };
    ids.repeat.store(true, std::sync::atomic::Ordering::SeqCst);
    let csv = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let query = if opaque {
        AppQuery::InspectUnopenedBundle {
            bytes,
            cancellation: app.setup_cancellation(),
        }
    } else {
        AppQuery::PreviewImport {
            cancellation: app.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        }
    };
    assert_code(app.query(query).await, "portable.preview_id_unavailable");
    assert!(!app.csv_report(csv.preview_id).await.boxed()?.consumed);
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn portable_preview_id_collision_does_not_replace_csv_authority() -> TestResult {
    colliding_portable_preview_keeps_csv(false).await
}

#[tokio::test]
async fn unopened_preview_id_collision_does_not_replace_csv_authority() -> TestResult {
    colliding_portable_preview_keeps_csv(true).await
}

async fn skipped_assignment_request(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    target_id: eutheto_types::EntityId,
    source: String,
) -> Result<CsvApplyRequestV1, Box<dyn Error>> {
    use eutheto_core::{
        AssignmentCsvMapping, AssignmentCsvPreviewRequestV1, AssignmentImportTarget,
    };
    let preview = app
        .preview_assignment_csv(
            AssignmentCsvPreviewRequestV1 {
                schema_version: 1,
                scenario_id,
                expected_revision: Revision::INITIAL,
                mapping: AssignmentCsvMapping::Mapped {
                    dialect: CsvDialect::Comma,
                    has_header: true,
                    expected_columns: 2,
                    person_column: 0,
                    shift_column: 1,
                    person_mappings: BTreeMap::new(),
                    shift_mappings: BTreeMap::new(),
                },
                target: AssignmentImportTarget::Create {
                    id: target_id,
                    name: "Unverified imported plan".to_owned(),
                },
                skipped_records: (2..=10_001).collect(),
                allow_empty: true,
            },
            Cursor::new(source),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        preview.preview.disposition,
        PeopleImportDisposition::Reviewable
    );
    Ok(CsvApplyRequestV1 {
        schema_version: 1,
        preview_id: preview.preview_id,
        scenario_id,
        expected_revision: Revision::INITIAL,
        approved_digest: preview
            .preview
            .approval_digest
            .ok_or("missing assignment approval")?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
        actor: ActorRef {
            actor_id: None,
            display_name: "CSV integration".to_owned(),
        },
        source: CommandSource::System,
        truncate_redo: false,
    })
}

#[tokio::test]
async fn assignment_import_retains_full_report_and_rebuilds_its_own_family_atomically() -> TestResult
{
    use eutheto_core::CsvReportRows;
    use eutheto_types::EntityId;

    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let scenario_id = create(&app).await?;
    let before = view(&app, scenario_id).await?;
    let target_id = EntityId::new(&SystemIdGenerator)?;
    let source = format!("person,shift\n{}", "unknown,unknown\n".repeat(10_000));
    let request = skipped_assignment_request(&app, scenario_id, target_id, source.clone()).await?;
    assert_eq!(view(&app, scenario_id).await?.document, before.document);
    assert_code(
        app.apply_csv_import(
            request.clone(),
            Cursor::new(source.replacen("unknown", "other", 1)),
            app.csv_operation(),
        )
        .await,
        "assignment_csv.staleReview",
    );
    assert!(history(&app, scenario_id).await?.is_empty());
    assert!(!app.csv_report(request.preview_id).await.boxed()?.consumed);
    let applied = app
        .apply_csv_import(request.clone(), Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        applied.outcome,
        CsvApplyOutcomeV1::Applied {
            command_id: request.command_id,
            revision: Revision::new(1)
        }
    );
    let CsvReportRows::Assignments { excluded_rows } = &applied.report.rows else {
        return Err("wrong retained report family".into());
    };
    assert_eq!(excluded_rows.len(), 10_000);
    assert_eq!(
        excluded_rows.last().ok_or("missing final row")?.record,
        10_001
    );
    assert!(excluded_rows.iter().all(|row| row.skipped));
    assert!(serde_json::to_vec(&applied)?.len() > 128 * 1024);
    assert!(applied.report.consumed);
    let changed = view(&app, scenario_id).await?;
    assert_eq!(
        changed.document.domain.locked_assignments,
        before.document.domain.locked_assignments
    );
    assert_eq!(
        changed.document.domain.entities[&target_id]["kind"],
        "externalStartingPlan"
    );
    assert_eq!(
        changed.document.domain.entities[&target_id]["assignments"],
        serde_json::json!([])
    );
    let entries = history(&app, scenario_id).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, request.command_id);
    drop(app);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_code(
        reopened.csv_report(request.preview_id).await,
        "people_csv.preview_unavailable",
    );
    assert_eq!(
        view(&reopened, scenario_id).await?.document,
        changed.document
    );
    reopened
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(1),
        })
        .await
        .boxed()?;
    assert_eq!(
        view(&reopened, scenario_id).await?.document,
        before.document
    );
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(2),
        })
        .await
        .boxed()?;
    assert_eq!(
        view(&reopened, scenario_id).await?.document,
        changed.document
    );
    assert_eq!(history(&reopened, scenario_id).await?, entries);
    Ok(())
}

fn eligibility_endpoint(index: u32) -> String {
    format!("018f7b40-a000-7000-8000-{index:012x}")
}

async fn eligibility_seed(app: &EuthetoApp, scenario_id: ScenarioId) -> TestResult {
    use eutheto_types::{CommandBatch, CommandEnvelope, DomainCommandEnvelope, ScenarioCommand};
    use serde_json::json;
    let mut entities = vec![
        json!({"kind":"calendar","id":eligibility_endpoint(2),"name":"Days",
            "period":{"kind":"day","startTime":"00:00:00"}}),
        json!({"kind":"workloadBucket","id":eligibility_endpoint(3),"name":"Hours",
            "measurement":"elapsedMinutes","overlappingContribution":"union"}),
        json!({"kind":"location","id":eligibility_endpoint(5),"name":"North","transitions":[]}),
        json!({"kind":"qualification","id":eligibility_endpoint(11),"name":"Clinician","description":""}),
        json!({"kind":"team","id":eligibility_endpoint(12),"name":"Blue"}),
    ];
    for index in [4, 21, 22, 23] {
        entities.push(
            json!({"kind":"assignmentType","id":eligibility_endpoint(index),
            "name":format!("Clinic {index}"),"category":"clinic","timeBehavior":"elapsed",
            "defaultDurationMinutes":60,"qualifications":{"kind":"matches",
                "allQualificationIds":[eligibility_endpoint(11)],"anyQualificationIds":[]},
            "locationBehavior":{"kind":"required"},"workloadBucketIds":[eligibility_endpoint(3)]}),
        );
    }
    for index in [1, 13] {
        entities.push(json!({"kind":"person","id":eligibility_endpoint(index),
            "name":format!("River {index}"),"externalId":format!("staff-{index}"),
            "activeRange":{"kind":"always"},"qualificationGrants":[{
                "qualificationId":eligibility_endpoint(11).to_ascii_uppercase(),
                "effectiveFrom":"2020-01-01T00:00:00+00:00","expiresAt":"2030-01-01T00:00:00+00:00"}],
            "eligibleAssignmentTypeIds":[eligibility_endpoint(4).to_ascii_uppercase(),
                eligibility_endpoint(22).to_ascii_uppercase()],
            "homeLocationId":eligibility_endpoint(5).replace('-', ""),
            "workloadWeight":{"numerator":3,"denominator":2},
            "workloadTarget":{"bucketId":eligibility_endpoint(3),"calendarId":eligibility_endpoint(2),
                "membership":"reportingDate","target":120},"tags":["night","reviewed"],
            "teamIds":[eligibility_endpoint(12).to_ascii_uppercase()],
            "display":{"color":"#102030","avatarInitials":"RV"}}));
    }
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: CommandEnvelope {
            command_id: CommandId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: Revision::INITIAL,
            actor: ActorRef {
                actor_id: None,
                display_name: "Eligibility endpoints".to_owned(),
            },
            source: CommandSource::System,
            command: ScenarioCommand::ApplyBatch(CommandBatch {
                label: Some("Eligibility endpoints".to_owned()),
                commands: entities
                    .into_iter()
                    .map(|entity| {
                        ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                            command_type: eutheto_workforce::commands::ADD_ENTITY.to_owned(),
                            payload: json!({"entity":entity}),
                        })
                    })
                    .collect(),
            }),
        },
        truncate_redo: false,
    })
    .await
    .boxed()?;
    Ok(())
}

fn eligibility_mapping() -> eutheto_core::EligibilityCsvMapping {
    eutheto_core::EligibilityCsvMapping {
        dialect: CsvDialect::Comma,
        has_header: true,
        expected_columns: 3,
        person_column: 0,
        assignment_type_column: 1,
        eligible_column: 2,
        person_mappings: BTreeMap::new(),
        assignment_type_mappings: BTreeMap::new(),
    }
}

async fn eligibility_preview(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    source: &[u8],
    skipped_records: Vec<u32>,
) -> Result<eutheto_core::EligibilityCsvPreviewDtoV1, Box<dyn Error>> {
    app.preview_eligibility_csv(
        eutheto_core::EligibilityCsvPreviewRequestV1 {
            schema_version: 1,
            scenario_id,
            expected_revision: revision,
            mapping: eligibility_mapping(),
            skipped_records,
        },
        Cursor::new(source.to_vec()),
        app.csv_operation(),
    )
    .await
    .boxed()
}

fn eligibility_apply_request(
    scenario_id: ScenarioId,
    revision: Revision,
    preview: &eutheto_core::EligibilityCsvPreviewDtoV1,
) -> Result<CsvApplyRequestV1, Box<dyn Error>> {
    Ok(CsvApplyRequestV1 {
        schema_version: 1,
        preview_id: preview.preview_id,
        scenario_id,
        expected_revision: revision,
        approved_digest: preview
            .preview
            .approval_digest
            .clone()
            .ok_or("eligibility approval")?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
        actor: ActorRef {
            actor_id: None,
            display_name: "Eligibility integration".to_owned(),
        },
        source: CommandSource::System,
        truncate_redo: false,
    })
}

#[tokio::test]
async fn eligibility_grouped_updates_are_durable_exact_and_one_step_undoable() -> TestResult {
    use serde_json::json;
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    eligibility_seed(&app, id).await?;
    let before = view(&app, id).await?;
    let source = format!(
        "person,type,eligible\n{},{},true\n{},{},false\n{},{},true\n{},{},false\n",
        eligibility_endpoint(1).to_ascii_uppercase(),
        eligibility_endpoint(21),
        eligibility_endpoint(1),
        eligibility_endpoint(22),
        eligibility_endpoint(1),
        eligibility_endpoint(23),
        eligibility_endpoint(13),
        eligibility_endpoint(4),
    );
    let review = eligibility_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let batch = review.preview.batch.as_ref().ok_or("grouped batch")?;
    assert_eq!(batch.commands.len(), 2);
    assert!(
        batch
            .commands
            .iter()
            .all(|command| command.command_type == eutheto_workforce::commands::UPDATE_ENTITY)
    );
    assert_eq!(view(&app, id).await?.document, before.document);
    assert_eq!(history(&app, id).await?.len(), 1);
    let request = eligibility_apply_request(id, Revision::new(1), &review)?;
    let applied = app
        .apply_csv_import(request.clone(), Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        applied.outcome,
        CsvApplyOutcomeV1::Applied {
            command_id: request.command_id,
            revision: Revision::new(2),
        }
    );
    let mut expected = before.document.clone();
    expected
        .domain
        .entities
        .get_mut(&eligibility_endpoint(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([
        eligibility_endpoint(4).to_ascii_uppercase(),
        eligibility_endpoint(21),
        eligibility_endpoint(23),
    ]);
    expected
        .domain
        .entities
        .get_mut(&eligibility_endpoint(13).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] =
        json!([eligibility_endpoint(22).to_ascii_uppercase()]);
    assert_eq!(view(&app, id).await?.document.domain, expected.domain);
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].id, request.command_id);
    drop(app);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(view(&reopened, id).await?.document.domain, expected.domain);
    reopened
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id: id,
            expected_revision: Revision::new(2),
        })
        .await
        .boxed()?;
    assert_eq!(
        view(&reopened, id).await?.document.domain,
        before.document.domain
    );
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id: id,
            expected_revision: Revision::new(3),
        })
        .await
        .boxed()?;
    assert_eq!(view(&reopened, id).await?.document.domain, expected.domain);
    assert_eq!(history(&reopened, id).await?, entries);
    Ok(())
}

#[tokio::test]
async fn eligibility_sparse_no_op_preserves_raw_spelling_revision_and_history() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    eligibility_seed(&app, id).await?;
    let before = view(&app, id).await?;
    let entries = history(&app, id).await?;
    let source = format!(
        "person,type,eligible\n{},{},true\n{},{},false\n",
        eligibility_endpoint(1),
        eligibility_endpoint(4),
        eligibility_endpoint(1).to_ascii_uppercase(),
        eligibility_endpoint(21)
    );
    let review = eligibility_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    assert_eq!(
        review.preview.disposition,
        PeopleImportDisposition::NoChanges
    );
    assert!(review.preview.batch.is_none());
    assert_eq!(
        review
            .preview
            .rows
            .iter()
            .map(|row| (row.before, row.after))
            .collect::<Vec<_>>(),
        [(Some(true), Some(true)), (Some(false), Some(false))]
    );
    let receipt = app
        .apply_csv_import(
            eligibility_apply_request(id, Revision::new(1), &review)?,
            Cursor::new(source),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::new(1)
        }
    );
    assert_eq!(view(&app, id).await?.document, before.document);
    assert_eq!(view(&app, id).await?.revision, before.revision);
    assert_eq!(history(&app, id).await?, entries);
    assert!(receipt.report.consumed);
    Ok(())
}

#[tokio::test]
async fn eligibility_no_op_rechecks_actual_store_revision_after_source_rebuild() -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    eligibility_seed(&app, id).await?;
    let source = format!(
        "person,type,eligible\n{},{},true\n",
        eligibility_endpoint(1),
        eligibility_endpoint(4)
    );
    let review = eligibility_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let request = eligibility_apply_request(id, Revision::new(1), &review)?;
    let second = EuthetoApp::open(deps).await.boxed()?;
    let changed = source.replace(",true\n", ",false\n");
    let concurrent =
        eligibility_preview(&second, id, Revision::new(1), changed.as_bytes(), vec![]).await?;
    let (reader, reached, release) = paused_reader(source.as_bytes());
    let caller_app = app.clone();
    let operation = app.csv_operation();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, reader, operation)
            .await
    });
    barrier(reached).await?;
    second
        .apply_csv_import(
            eligibility_apply_request(id, Revision::new(1), &concurrent)?,
            Cursor::new(changed),
            second.csv_operation(),
        )
        .await
        .boxed()?;
    barrier(release).await?;
    assert!(matches!(caller.await?,
        Err(AppError::Conflict { expected_revision, actual_revision })
        if expected_revision == Revision::new(1) && actual_revision == Revision::new(2)));
    assert_eq!(history(&app, id).await?.len(), 2);
    assert!(!app.csv_report(review.preview_id).await.boxed()?.consumed);
    Ok(())
}

#[tokio::test]
async fn eligibility_retained_family_binds_approval_source_and_complete_report() -> TestResult {
    use eutheto_core::CsvReportRows;
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let source = format!(
        "person,type,eligible\n{}",
        "unknown,unknown,true\n".repeat(10_000)
    );
    let review = eligibility_preview(
        &app,
        id,
        Revision::INITIAL,
        source.as_bytes(),
        (2..=10_001).collect(),
    )
    .await?;
    let request = eligibility_apply_request(id, Revision::INITIAL, &review)?;
    // There is no caller-selectable family on apply; unknown fields cannot redirect authority.
    let mut forged = serde_json::to_value(&request)?;
    forged["family"] = serde_json::json!("people");
    assert!(serde_json::from_value::<CsvApplyRequestV1>(forged).is_err());
    let mut wrong_digest = request.clone();
    wrong_digest.approved_digest = "0".repeat(64);
    assert_code(
        app.apply_csv_import(
            wrong_digest,
            Cursor::new(source.clone()),
            app.csv_operation(),
        )
        .await,
        "people_csv.approval_mismatch",
    );
    assert_code(
        app.apply_csv_import(
            request.clone(),
            Cursor::new(source.replacen("unknown", "other", 1)),
            app.csv_operation(),
        )
        .await,
        "eligibility_csv.staleReview",
    );
    let retained = app.csv_report(review.preview_id).await.boxed()?;
    let CsvReportRows::Eligibility { excluded_rows } = &retained.rows else {
        return Err("eligibility report lost its retained family".into());
    };
    assert_eq!(excluded_rows.len(), 10_000);
    assert_eq!(excluded_rows[0].record, 2);
    assert_eq!(excluded_rows[9_999].record, 10_001);
    assert!(excluded_rows.iter().all(|row| row.skipped));
    assert!(!retained.consumed);
    let receipt = app
        .apply_csv_import(request, Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::INITIAL
        }
    );
    assert_eq!(receipt.report.rows, retained.rows);
    assert!(serde_json::to_vec(&receipt)?.len() > 128 * 1024);
    assert_eq!(
        app.csv_report(review.preview_id).await.boxed()?,
        receipt.report
    );
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

fn shift_mapping() -> eutheto_core::ShiftCsvMapping {
    use eutheto_core::{ShiftCsvColumn, ShiftCsvField};
    eutheto_core::ShiftCsvMapping {
        dialect: CsvDialect::Tab,
        has_header: true,
        expected_columns: 8,
        identity_column: Some(0),
        columns: [
            ShiftCsvField::AssignmentTypeId,
            ShiftCsvField::LocationId,
            ShiftCsvField::StartsAt,
            ShiftCsvField::EndsAt,
            ShiftCsvField::Coverage,
            ShiftCsvField::Tags,
            ShiftCsvField::ReportingAttribution,
        ]
        .into_iter()
        .zip(1_u16..)
        .map(|(field, index)| ShiftCsvColumn {
            index,
            field,
            blank: BlankPolicy::Preserve,
        })
        .collect(),
        reference_mappings: BTreeMap::new(),
    }
}

fn shift_source(id: u32, tags: &str) -> String {
    format!(
        "id\ttype\tlocation\tstart\tend\tcoverage\ttags\tattribution\n{}\t{}\t{}\t2026-01-02T09:00:00\t2026-01-02T10:00:00\t{{\"kind\":\"exact\",\"count\":1,\"qualificationMinimums\":[]}}\t{tags}\tstartLocalDate\n",
        eligibility_endpoint(id),
        eligibility_endpoint(4),
        eligibility_endpoint(5),
    )
}

async fn shift_seed(app: &EuthetoApp, scenario_id: ScenarioId) -> TestResult {
    use eutheto_types::{CommandBatch, CommandEnvelope, DomainCommandEnvelope, ScenarioCommand};
    use serde_json::json;
    let entities = vec![
        json!({"kind":"assignmentType","id":eligibility_endpoint(4),"name":"Clinic",
            "category":"clinic","timeBehavior":"elapsed","defaultDurationMinutes":60,
            "qualifications":{"kind":"unconstrained"},"locationBehavior":{"kind":"required"},
            "workloadBucketIds":[]}),
        json!({"kind":"location","id":eligibility_endpoint(5),"name":"North","transitions":[]}),
        json!({"kind":"shiftInstance","id":eligibility_endpoint(8),
            "assignmentTypeId":eligibility_endpoint(4).to_ascii_uppercase(),
            "locationId":eligibility_endpoint(5).replace('-', ""),
            "startsAt":{"instant":"2026-01-02T09:00:00+00:00",
                "local":"2026-01-02T09:00:00.000","offsetSeconds":0},
            "endsAt":{"instant":"2026-01-02T10:00:00+00:00",
                "local":"2026-01-02T10:00:00.000","offsetSeconds":0},
            "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]},
            "tags":["authored","order"],"reportingAttribution":"startLocalDate",
            "origin":{"kind":"manual"}}),
    ];
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: CommandEnvelope {
            command_id: CommandId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: Revision::INITIAL,
            actor: ActorRef {
                actor_id: None,
                display_name: "Shift endpoints".to_owned(),
            },
            source: CommandSource::System,
            command: ScenarioCommand::ApplyBatch(CommandBatch {
                label: Some("Shift endpoints".to_owned()),
                commands: entities
                    .into_iter()
                    .map(|entity| {
                        ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                            command_type: eutheto_workforce::commands::ADD_ENTITY.to_owned(),
                            payload: json!({"entity":entity}),
                        })
                    })
                    .collect(),
            }),
        },
        truncate_redo: false,
    })
    .await
    .boxed()?;
    Ok(())
}

async fn shift_preview(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    source: &[u8],
    decisions: Vec<eutheto_core::ShiftCsvRowDecision>,
) -> Result<eutheto_core::ShiftCsvPreviewDtoV1, Box<dyn Error>> {
    app.preview_shift_csv(
        eutheto_core::ShiftCsvPreviewRequestV1 {
            schema_version: 1,
            scenario_id,
            expected_revision: revision,
            mapping: shift_mapping(),
            decisions,
        },
        Cursor::new(source.to_vec()),
        app.csv_operation(),
    )
    .await
    .boxed()
}

fn shift_apply_request(
    scenario_id: ScenarioId,
    revision: Revision,
    preview: &eutheto_core::ShiftCsvPreviewDtoV1,
) -> Result<CsvApplyRequestV1, Box<dyn Error>> {
    Ok(CsvApplyRequestV1 {
        schema_version: 1,
        scenario_id,
        expected_revision: revision,
        preview_id: preview.preview_id,
        approved_digest: preview
            .preview
            .approval_digest
            .clone()
            .ok_or("shift approval")?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
        actor: ActorRef {
            actor_id: None,
            display_name: "Shift integration".to_owned(),
        },
        source: CommandSource::System,
        truncate_redo: false,
    })
}

fn shift_add(record: u32, index: u32) -> Result<eutheto_core::ShiftCsvRowDecision, Box<dyn Error>> {
    Ok(eutheto_core::ShiftCsvRowDecision {
        record,
        decision: eutheto_core::ShiftCsvDecision::Add {
            shift_id: eligibility_endpoint(index).parse()?,
        },
    })
}

#[tokio::test]
async fn shift_csv_durable_same_id_add_and_raw_update_have_exact_undo_redo() -> TestResult {
    use eutheto_core::{ShiftCsvIdentity, ShiftCsvRowStatus};
    use serde_json::json;
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    shift_seed(&app, id).await?;
    let before = view(&app, id).await?.document;
    let source = format!(
        "{}{}",
        shift_source(8, r#"["changed"]"#),
        shift_source(9, "[]")
            .lines()
            .nth(1)
            .ok_or("new shift row")?
            .to_owned()
            + "\n"
    );
    let review = shift_preview(
        &app,
        id,
        Revision::new(1),
        source.as_bytes(),
        vec![shift_add(3, 9)?],
    )
    .await?;
    assert_eq!(review.preview.rows[0].status, ShiftCsvRowStatus::Updated);
    assert_eq!(review.preview.rows[1].status, ShiftCsvRowStatus::Added);
    assert_eq!(
        review.preview.rows[1].identity,
        ShiftCsvIdentity::Supplied {
            shift_id: eligibility_endpoint(9).parse()?
        }
    );
    assert_eq!(view(&app, id).await?.document, before);
    let mut expected = before.domain.clone();
    expected
        .entities
        .get_mut(&eligibility_endpoint(8).parse()?)
        .ok_or("stored shift")?["tags"] = json!(["changed"]);
    expected.entities.insert(
        eligibility_endpoint(9).parse()?,
        review.preview.rows[1]
            .after
            .clone()
            .ok_or("new shift projection")?,
    );
    // Equivalent UUID/local/instant input must leave every raw field except tags untouched.
    assert_eq!(
        review.preview.rows[0]
            .after
            .as_ref()
            .ok_or("changed projection")?,
        &expected.entities[&eligibility_endpoint(8).parse()?]
    );
    let request = shift_apply_request(id, Revision::new(1), &review)?;
    let receipt = app
        .apply_csv_import(request.clone(), Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::Applied {
            command_id: request.command_id,
            revision: Revision::new(2),
        }
    );
    assert!(matches!(
        receipt.report.rows,
        eutheto_core::CsvReportRows::Shifts { .. }
    ));
    assert_eq!(view(&app, id).await?.document.domain, expected);
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    drop(app);
    let app = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(view(&app, id).await?.document.domain, expected);
    app.execute(AppCommand::Undo {
        request_id: request_id()?,
        scenario_id: id,
        expected_revision: Revision::new(2),
    })
    .await
    .boxed()?;
    assert_eq!(view(&app, id).await?.document.domain, before.domain);
    app.execute(AppCommand::Redo {
        request_id: request_id()?,
        scenario_id: id,
        expected_revision: Revision::new(3),
    })
    .await
    .boxed()?;
    assert_eq!(view(&app, id).await?.document.domain, expected);
    assert_eq!(history(&app, id).await?, entries);
    Ok(())
}

#[tokio::test]
async fn shift_csv_equivalent_fields_preserve_raw_no_op_and_history() -> TestResult {
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    shift_seed(&app, id).await?;
    let before = view(&app, id).await?;
    let source = shift_source(8, r#"["authored","order"]"#);
    let preview = shift_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    assert_eq!(
        preview.preview.rows[0].status,
        eutheto_core::ShiftCsvRowStatus::Unchanged
    );
    assert_eq!(
        preview.preview.rows[0].before.as_ref(),
        before
            .document
            .domain
            .entities
            .get(&eligibility_endpoint(8).parse()?)
    );
    assert!(preview.preview.rows[0].after.is_none());
    let receipt = app
        .apply_csv_import(
            shift_apply_request(id, Revision::new(1), &preview)?,
            Cursor::new(source),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::new(1)
        }
    );
    assert_eq!(view(&app, id).await?, before);
    assert_eq!(history(&app, id).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn shift_csv_wrapper_identity_collisions_and_retained_census_are_native() -> TestResult {
    use rusqlite::{Connection, params};
    use serde_json::json;
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    shift_seed(&app, id).await?;
    let source = shift_source(9, "[]");
    let pristine = shift_preview(
        &app,
        id,
        Revision::new(1),
        source.as_bytes(),
        vec![shift_add(2, 9)?],
    )
    .await?;
    let connection = Connection::open(&deps.paths.database)?;
    for semantic in [true, false] {
        let wrapper = serde_json::to_string(
            &json!({"vendor.shifts":{(eligibility_endpoint(9)):{"id":eligibility_endpoint(9)}}}),
        )?;
        connection.execute(
            "UPDATE scenarios SET portable_semantic_extensions_json = ?2,
             portable_extensions_json = ?3 WHERE id = ?1",
            params![
                id.to_string(),
                if semantic { wrapper.as_str() } else { "{}" },
                if semantic { "{}" } else { wrapper.as_str() }
            ],
        )?;
        let blocked = shift_preview(
            &app,
            id,
            Revision::new(1),
            source.as_bytes(),
            vec![shift_add(2, 9)?],
        )
        .await?;
        assert_eq!(
            blocked.preview.disposition,
            PeopleImportDisposition::Blocked
        );
        assert_eq!(
            blocked.preview.rows[0].issue,
            Some(eutheto_core::ShiftCsvRowIssue::ReservedIdentity)
        );
        assert!(blocked.preview.approval_digest.is_none());
        // Even without a revision change, a newly owned wrapper identity invalidates approval.
        assert_code(
            app.apply_csv_import(
                shift_apply_request(id, Revision::new(1), &pristine)?,
                Cursor::new(source.clone()),
                app.csv_operation(),
            )
            .await,
            "shift_csv.staleReview",
        );
    }
    assert_eq!(history(&app, id).await?.len(), 1);
    assert!(!app.csv_report(pristine.preview_id).await.boxed()?.consumed);
    Ok(())
}

async fn shift_revision_race(no_changes: bool) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    shift_seed(&app, id).await?;
    let source = shift_source(
        8,
        if no_changes {
            r#"["authored","order"]"#
        } else {
            r#"["first"]"#
        },
    );
    let review = shift_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let request = shift_apply_request(id, Revision::new(1), &review)?;
    let second = EuthetoApp::open(deps).await.boxed()?;
    let changed = shift_source(8, r#"["concurrent"]"#);
    let concurrent =
        shift_preview(&second, id, Revision::new(1), changed.as_bytes(), vec![]).await?;
    let (reader, reached, release) = paused_reader(source.as_bytes());
    let caller_app = app.clone();
    let operation = app.csv_operation();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, reader, operation)
            .await
    });
    barrier(reached).await?;
    second
        .apply_csv_import(
            shift_apply_request(id, Revision::new(1), &concurrent)?,
            Cursor::new(changed),
            second.csv_operation(),
        )
        .await
        .boxed()?;
    barrier(release).await?;
    assert!(matches!(caller.await?,
        Err(AppError::Conflict { expected_revision, actual_revision })
        if expected_revision == Revision::new(1) && actual_revision == Revision::new(2)));
    assert_eq!(history(&app, id).await?.len(), 2);
    assert!(!app.csv_report(review.preview_id).await.boxed()?.consumed);
    let renewed = shift_preview(&app, id, Revision::new(2), source.as_bytes(), vec![]).await?;
    assert_ne!(
        review.preview.approval_digest,
        renewed.preview.approval_digest
    );
    Ok(())
}

#[tokio::test]
async fn shift_csv_mutation_rechecks_concurrent_store_revision() -> TestResult {
    shift_revision_race(false).await
}

#[tokio::test]
async fn shift_csv_no_op_rechecks_actual_revision_after_rebuild() -> TestResult {
    shift_revision_race(true).await
}

#[tokio::test]
async fn shift_csv_full_report_is_family_bound_source_bound_and_no_op() -> TestResult {
    use eutheto_core::{CsvReportRows, ShiftCsvDecision, ShiftCsvRowDecision};
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let source = format!(
        "id\ttype\tlocation\tstart\tend\tcoverage\ttags\tattribution\n{}",
        "bad\tunknown\t\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\n".repeat(10_000)
    );
    let review = shift_preview(
        &app,
        id,
        Revision::INITIAL,
        source.as_bytes(),
        (2..=10_001)
            .map(|record| ShiftCsvRowDecision {
                record,
                decision: ShiftCsvDecision::Skip {},
            })
            .collect(),
    )
    .await?;
    let request = shift_apply_request(id, Revision::INITIAL, &review)?;
    let mut forged = serde_json::to_value(&request)?;
    forged["family"] = serde_json::json!("people");
    assert!(serde_json::from_value::<CsvApplyRequestV1>(forged).is_err());
    assert_code(
        app.apply_csv_import(
            request.clone(),
            Cursor::new(source.replacen("bad", "other", 1)),
            app.csv_operation(),
        )
        .await,
        "shift_csv.staleReview",
    );
    let retained = app.csv_report(review.preview_id).await.boxed()?;
    let CsvReportRows::Shifts { excluded_rows } = &retained.rows else {
        return Err("lost shift report family".into());
    };
    assert_eq!(excluded_rows.len(), 10_000);
    assert_eq!(excluded_rows[0].record, 2);
    assert_eq!(excluded_rows[9_999].record, 10_001);
    assert!(excluded_rows.iter().all(|row| row.skipped));
    let receipt = app
        .apply_csv_import(request, Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::INITIAL
        }
    );
    assert_eq!(receipt.report.rows, retained.rows);
    assert!(serde_json::to_vec(&receipt)?.len() > 128 * 1024);
    assert_eq!(
        app.csv_report(review.preview_id).await.boxed()?,
        receipt.report
    );
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[cfg(debug_assertions)]
async fn shift_commit_wins(abandon: bool) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let seed_app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&seed_app).await?;
    shift_seed(&seed_app, id).await?;
    drop(seed_app);
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::AfterFinalCancellationCheck);
    let (store, initialized) = SqliteScenarioStore::open_with_options(
        &deps.paths.database,
        OpenOptions::new(SnapshotPolicy::default()).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let store = Arc::new(store);
    let app =
        EuthetoApp::from_initialized_store(store.clone(), initialized, deps.clone()).boxed()?;
    let source = shift_source(8, r#"["committed"]"#);
    let review = shift_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let request = shift_apply_request(id, Revision::new(1), &review)?;
    let command_id = request.command_id;
    let event_request = request.request_id;
    let mut events = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, Cursor::new(source), operation)
            .await
    });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    if abandon {
        caller.abort();
    } else {
        signal.cancel();
        app.discard_csv_preview(review.preview_id).await.boxed()?;
    }
    let retained_caller = if abandon {
        assert!(caller.await.is_err_and(|error| error.is_cancelled()));
        None
    } else {
        Some(caller)
    };
    tokio::task::spawn_blocking(move || hook.release()).await?;
    if let Some(caller) = retained_caller {
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(10), caller)
            .await??
            .boxed()?;
        assert_eq!(
            receipt.outcome,
            CsvApplyOutcomeV1::Applied {
                command_id,
                revision: Revision::new(2),
            }
        );
        assert!(matches!(
            receipt.report.rows,
            eutheto_core::CsvReportRows::Shifts { .. }
        ));
        assert!(receipt.report.consumed);
    }
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await?
        .boxed()?;
    assert!(
        matches!(event.payload, EventPayload::ScenarioChanged { context, .. }
        if context.scenario_id == Some(id) && context.revision == Some(Revision::new(2))
            && context.request_id == Some(event_request))
    );
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].id, command_id);
    drop(app);
    drop(store);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(history(&reopened, id).await?, entries);
    assert_eq!(
        view(&reopened, id).await?.document.domain.entities[&eligibility_endpoint(8).parse()?]["tags"],
        serde_json::json!(["committed"])
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn shift_csv_late_cancellation_and_discard_cannot_replace_committed_receipt() -> TestResult {
    shift_commit_wins(false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn shift_csv_unknown_receipt_reconciles_committed_history_without_replay() -> TestResult {
    shift_commit_wins(true).await
}

fn availability_mapping() -> eutheto_core::AvailabilityCsvMapping {
    use eutheto_core::{AvailabilityCsvColumn, AvailabilityCsvField};
    eutheto_core::AvailabilityCsvMapping {
        dialect: CsvDialect::Tab,
        has_header: true,
        expected_columns: 12,
        identity_column: Some(0),
        columns: [
            AvailabilityCsvField::PersonId,
            AvailabilityCsvField::AvailabilityKind,
            AvailabilityCsvField::StartsAt,
            AvailabilityCsvField::EndsAt,
            AvailabilityCsvField::WeeklyWindows,
            AvailabilityCsvField::EffectiveStartDate,
            AvailabilityCsvField::EffectiveEndDateExclusive,
            AvailabilityCsvField::AssignmentTypeIds,
            AvailabilityCsvField::LocationIds,
            AvailabilityCsvField::Source,
            AvailabilityCsvField::Note,
        ]
        .into_iter()
        .zip(1_u16..)
        .map(|(field, index)| AvailabilityCsvColumn {
            index,
            field,
            blank: BlankPolicy::Preserve,
        })
        .collect(),
        person_mappings: BTreeMap::new(),
        assignment_type_mappings: BTreeMap::new(),
        location_mappings: BTreeMap::new(),
    }
}

fn availability_source(index: u32, note: &str) -> String {
    format!(
        "id\tperson\tkind\tstart\tend\tweekly\tfrom\tuntil\ttypes\tlocations\tsource\tnote\n{}\t{}\tapprovedTimeOff\t2026-01-02T09:00:00.123\t2026-01-02T10:00:00.456\t\t2026-01-01\t2026-02-01\t[\"{}\"]\t[\"{}\"]\tauthored\t{note}\n",
        eligibility_endpoint(index),
        eligibility_endpoint(1),
        eligibility_endpoint(4),
        eligibility_endpoint(5),
    )
}

async fn availability_seed(app: &EuthetoApp, scenario_id: ScenarioId) -> TestResult {
    use eutheto_types::{CommandBatch, CommandEnvelope, DomainCommandEnvelope, ScenarioCommand};
    use serde_json::json;
    let entities = vec![
        json!({"kind":"person","id":eligibility_endpoint(1),"name":"River",
            "activeRange":{"kind":"always"},"qualificationGrants":[],
            "eligibleAssignmentTypeIds":[],"workloadWeight":{"numerator":1,"denominator":1},
            "tags":[],"teamIds":[]}),
        json!({"kind":"assignmentType","id":eligibility_endpoint(4),"name":"Clinic",
            "category":"clinic","timeBehavior":"elapsed","defaultDurationMinutes":60,
            "qualifications":{"kind":"unconstrained"},"locationBehavior":{"kind":"required"},
            "workloadBucketIds":[]}),
        json!({"kind":"location","id":eligibility_endpoint(5),"name":"North","transitions":[]}),
        json!({"kind":"availability","id":eligibility_endpoint(8),
            "personId":eligibility_endpoint(1).to_ascii_uppercase(),
            "availabilityKind":"approvedTimeOff",
            "timeWindow":{"kind":"instant","startsAt":"2026-01-02T04:00:00.123-05:00",
                "endsAt":"2026-01-02T05:00:00.456-05:00"},
            "effectiveRange":{"startDate":"2026-01-01","endDateExclusive":"2026-02-01"},
            "assignmentTypeIds":[eligibility_endpoint(4).to_ascii_uppercase()],
            "locationIds":[eligibility_endpoint(5).replace('-', "")],
            "source":"authored","note":"original"}),
    ];
    app.execute(AppCommand::ApplyScenario {
        request_id: request_id()?,
        envelope: CommandEnvelope {
            command_id: CommandId::new(&SystemIdGenerator)?,
            scenario_id,
            expected_revision: Revision::INITIAL,
            actor: ActorRef {
                actor_id: None,
                display_name: "Availability seed".to_owned(),
            },
            source: CommandSource::System,
            command: ScenarioCommand::ApplyBatch(CommandBatch {
                label: Some("Availability seed".to_owned()),
                commands: entities
                    .into_iter()
                    .map(|entity| {
                        ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                            command_type: eutheto_workforce::commands::ADD_ENTITY.to_owned(),
                            payload: json!({"entity":entity}),
                        })
                    })
                    .collect(),
            }),
        },
        truncate_redo: false,
    })
    .await
    .boxed()?;
    Ok(())
}

async fn availability_preview(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    source: &[u8],
    decisions: Vec<eutheto_core::AvailabilityCsvRowDecision>,
) -> Result<eutheto_core::AvailabilityCsvPreviewDtoV1, Box<dyn Error>> {
    app.preview_availability_csv(
        eutheto_core::AvailabilityCsvPreviewRequestV1 {
            schema_version: 1,
            scenario_id,
            expected_revision: revision,
            mapping: availability_mapping(),
            decisions,
        },
        Cursor::new(source.to_vec()),
        app.csv_operation(),
    )
    .await
    .boxed()
}

fn availability_apply_request(
    scenario_id: ScenarioId,
    revision: Revision,
    preview: &eutheto_core::AvailabilityCsvPreviewDtoV1,
) -> Result<CsvApplyRequestV1, Box<dyn Error>> {
    Ok(CsvApplyRequestV1 {
        schema_version: 1,
        scenario_id,
        expected_revision: revision,
        preview_id: preview.preview_id,
        approved_digest: preview
            .preview
            .approval_digest
            .clone()
            .ok_or("availability approval")?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
        actor: ActorRef {
            actor_id: None,
            display_name: "Availability import".to_owned(),
        },
        source: CommandSource::System,
        truncate_redo: false,
    })
}

fn availability_add(
    record: u32,
    index: u32,
) -> Result<eutheto_core::AvailabilityCsvRowDecision, Box<dyn Error>> {
    Ok(eutheto_core::AvailabilityCsvRowDecision {
        record,
        decision: eutheto_core::AvailabilityCsvDecision::Add {
            availability_id: eligibility_endpoint(index).parse()?,
        },
    })
}

#[tokio::test]
async fn availability_csv_durable_raw_update_same_id_add_and_exact_undo_redo() -> TestResult {
    use eutheto_core::{AvailabilityCsvIdentity, AvailabilityCsvRowStatus, CsvReportRows};
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    availability_seed(&app, id).await?;
    let before = view(&app, id).await?.document;
    let source = availability_source(8, "changed")
        + availability_source(9, "new")
            .lines()
            .nth(1)
            .ok_or("new row")?
        + "\n";
    let review = availability_preview(
        &app,
        id,
        Revision::new(1),
        source.as_bytes(),
        vec![availability_add(3, 9)?],
    )
    .await?;
    assert_eq!(
        review.preview.rows[0].status,
        AvailabilityCsvRowStatus::Updated
    );
    assert_eq!(
        review.preview.rows[1].status,
        AvailabilityCsvRowStatus::Added
    );
    assert_eq!(
        review.preview.rows[1].identity,
        AvailabilityCsvIdentity::Supplied {
            availability_id: eligibility_endpoint(9).parse()?,
        }
    );
    let mut expected = before.domain.clone();
    expected
        .entities
        .get_mut(&eligibility_endpoint(8).parse()?)
        .ok_or("availability")?["note"] = serde_json::json!("changed");
    assert_eq!(
        review.preview.rows[0].after.as_ref(),
        expected.entities.get(&eligibility_endpoint(8).parse()?)
    );
    expected.entities.insert(
        eligibility_endpoint(9).parse()?,
        review.preview.rows[1]
            .after
            .clone()
            .ok_or("new projection")?,
    );
    assert_eq!(view(&app, id).await?.document, before);
    let request = availability_apply_request(id, Revision::new(1), &review)?;
    let receipt = app
        .apply_csv_import(request.clone(), Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::Applied {
            command_id: request.command_id,
            revision: Revision::new(2)
        }
    );
    assert!(matches!(
        receipt.report.rows,
        CsvReportRows::Availability { .. }
    ));
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    drop(app);
    let app = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(view(&app, id).await?.document.domain, expected);
    assert_code(
        app.csv_report(review.preview_id).await,
        "people_csv.preview_unavailable",
    );
    app.execute(AppCommand::Undo {
        request_id: request_id()?,
        scenario_id: id,
        expected_revision: Revision::new(2),
    })
    .await
    .boxed()?;
    assert_eq!(view(&app, id).await?.document.domain, before.domain);
    app.execute(AppCommand::Redo {
        request_id: request_id()?,
        scenario_id: id,
        expected_revision: Revision::new(3),
    })
    .await
    .boxed()?;
    assert_eq!(view(&app, id).await?.document.domain, expected);
    assert_eq!(history(&app, id).await?, entries);
    Ok(())
}

#[tokio::test]
async fn availability_csv_no_op_preserves_raw_and_consumes_approval_without_history() -> TestResult
{
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    availability_seed(&app, id).await?;
    let before = view(&app, id).await?;
    let source = availability_source(8, "original");
    let review =
        availability_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    assert_eq!(
        review.preview.rows[0].status,
        eutheto_core::AvailabilityCsvRowStatus::Unchanged
    );
    assert_eq!(
        review.preview.rows[0].before.as_ref(),
        before
            .document
            .domain
            .entities
            .get(&eligibility_endpoint(8).parse()?)
    );
    assert!(review.preview.rows[0].after.is_none());
    assert!(review.preview.rows[0].resolved_starts_at.is_some());
    let request = availability_apply_request(id, Revision::new(1), &review)?;
    let receipt = app
        .apply_csv_import(
            request.clone(),
            Cursor::new(source.clone()),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::new(1)
        }
    );
    assert!(receipt.report.consumed);
    assert_eq!(view(&app, id).await?, before);
    assert_eq!(history(&app, id).await?.len(), 1);
    assert_code(
        app.apply_csv_import(request, Cursor::new(source), app.csv_operation())
            .await,
        "people_csv.preview_consumed",
    );
    Ok(())
}

#[tokio::test]
async fn availability_csv_wrapper_census_blocks_initial_and_retained_identity_collisions()
-> TestResult {
    use rusqlite::{Connection, params};
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    availability_seed(&app, id).await?;
    let source = availability_source(9, "new");
    let pristine = availability_preview(
        &app,
        id,
        Revision::new(1),
        source.as_bytes(),
        vec![availability_add(2, 9)?],
    )
    .await?;
    let connection = Connection::open(&deps.paths.database)?;
    for semantic in [true, false] {
        let wrapper = serde_json::to_string(
            &serde_json::json!({"vendor.availability":{(eligibility_endpoint(9)):{"id":eligibility_endpoint(9)}}}),
        )?;
        connection.execute(
            "UPDATE scenarios SET portable_semantic_extensions_json = ?2, portable_extensions_json = ?3 WHERE id = ?1",
            params![id.to_string(), if semantic { wrapper.as_str() } else { "{}" }, if semantic { "{}" } else { wrapper.as_str() }],
        )?;
        let blocked = availability_preview(
            &app,
            id,
            Revision::new(1),
            source.as_bytes(),
            vec![availability_add(2, 9)?],
        )
        .await?;
        assert_eq!(
            blocked.preview.disposition,
            PeopleImportDisposition::Blocked
        );
        assert_eq!(
            blocked.preview.rows[0].issue,
            Some(eutheto_core::AvailabilityCsvRowIssue::ReservedIdentity)
        );
        assert!(blocked.preview.approval_digest.is_none());
        assert_code(
            app.apply_csv_import(
                availability_apply_request(id, Revision::new(1), &pristine)?,
                Cursor::new(source.clone()),
                app.csv_operation(),
            )
            .await,
            "availability_csv.staleReview",
        );
    }
    assert_eq!(history(&app, id).await?.len(), 1);
    assert!(!app.csv_report(pristine.preview_id).await.boxed()?.consumed);
    Ok(())
}

async fn availability_revision_race(no_changes: bool) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    availability_seed(&app, id).await?;
    let source = availability_source(8, if no_changes { "original" } else { "first" });
    let review =
        availability_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let request = availability_apply_request(id, Revision::new(1), &review)?;
    let second = EuthetoApp::open(deps).await.boxed()?;
    let changed = availability_source(8, "concurrent");
    let concurrent =
        availability_preview(&second, id, Revision::new(1), changed.as_bytes(), vec![]).await?;
    let (reader, reached, release) = paused_reader(source.as_bytes());
    let caller_app = app.clone();
    let operation = app.csv_operation();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, reader, operation)
            .await
    });
    barrier(reached).await?;
    second
        .apply_csv_import(
            availability_apply_request(id, Revision::new(1), &concurrent)?,
            Cursor::new(changed),
            second.csv_operation(),
        )
        .await
        .boxed()?;
    barrier(release).await?;
    assert!(
        matches!(caller.await?, Err(AppError::Conflict { expected_revision, actual_revision })
        if expected_revision == Revision::new(1) && actual_revision == Revision::new(2))
    );
    assert_eq!(history(&app, id).await?.len(), 2);
    assert!(!app.csv_report(review.preview_id).await.boxed()?.consumed);
    Ok(())
}

#[tokio::test]
async fn availability_csv_mutation_rechecks_concurrent_store_revision() -> TestResult {
    availability_revision_race(false).await
}

#[tokio::test]
async fn availability_csv_no_op_rechecks_actual_revision_after_rebuild() -> TestResult {
    availability_revision_race(true).await
}

#[tokio::test]
async fn availability_csv_complete_exclusions_and_family_source_approval_binding() -> TestResult {
    use eutheto_core::{AvailabilityCsvDecision, AvailabilityCsvRowDecision, CsvReportRows};
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let source = "id\tperson\tkind\tstart\tend\tweekly\tfrom\tuntil\ttypes\tlocations\tsource\tnote\n".to_owned()
        + &"bad\tunknown\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\tinvalid\t\t\n".repeat(10_000);
    let review = availability_preview(
        &app,
        id,
        Revision::INITIAL,
        source.as_bytes(),
        (2..=10_001)
            .map(|record| AvailabilityCsvRowDecision {
                record,
                decision: AvailabilityCsvDecision::Skip {},
            })
            .collect(),
    )
    .await?;
    let request = availability_apply_request(id, Revision::INITIAL, &review)?;
    let mut forged = serde_json::to_value(&request)?;
    forged["family"] = serde_json::json!("shifts");
    assert!(serde_json::from_value::<CsvApplyRequestV1>(forged).is_err());
    let mut wrong_digest = request.clone();
    wrong_digest.approved_digest = "0".repeat(64);
    assert_code(
        app.apply_csv_import(
            wrong_digest,
            Cursor::new(source.clone()),
            app.csv_operation(),
        )
        .await,
        "people_csv.approval_mismatch",
    );
    assert_code(
        app.apply_csv_import(
            request.clone(),
            Cursor::new(source.replacen("bad", "other", 1)),
            app.csv_operation(),
        )
        .await,
        "availability_csv.staleReview",
    );
    let retained = app.csv_report(review.preview_id).await.boxed()?;
    let CsvReportRows::Availability { excluded_rows } = &retained.rows else {
        return Err("availability report family".into());
    };
    assert_eq!(excluded_rows.len(), 10_000);
    assert_eq!(excluded_rows[0].record, 2);
    assert_eq!(excluded_rows[9_999].record, 10_001);
    assert!(excluded_rows.iter().all(|row| row.skipped));
    let receipt = app
        .apply_csv_import(request, Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::INITIAL
        }
    );
    assert_eq!(receipt.report.rows, retained.rows);
    assert!(serde_json::to_vec(&receipt)?.len() > 128 * 1024);
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[cfg(debug_assertions)]
async fn availability_commit_wins(abandon: bool) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let seed_app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&seed_app).await?;
    availability_seed(&seed_app, id).await?;
    drop(seed_app);
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::AfterFinalCancellationCheck);
    let (store, initialized) = SqliteScenarioStore::open_with_options(
        &deps.paths.database,
        OpenOptions::new(SnapshotPolicy::default()).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let store = Arc::new(store);
    let app =
        EuthetoApp::from_initialized_store(store.clone(), initialized, deps.clone()).boxed()?;
    let source = availability_source(8, "committed");
    let review =
        availability_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let request = availability_apply_request(id, Revision::new(1), &review)?;
    let command_id = request.command_id;
    let event_request = request.request_id;
    let mut events = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, Cursor::new(source), operation)
            .await
    });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    if abandon {
        caller.abort();
    } else {
        signal.cancel();
        app.discard_csv_preview(review.preview_id).await.boxed()?;
    }
    let retained_caller = if abandon {
        assert!(caller.await.is_err_and(|error| error.is_cancelled()));
        None
    } else {
        Some(caller)
    };
    tokio::task::spawn_blocking(move || hook.release()).await?;
    if let Some(caller) = retained_caller {
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(10), caller)
            .await??
            .boxed()?;
        assert_eq!(
            receipt.outcome,
            CsvApplyOutcomeV1::Applied {
                command_id,
                revision: Revision::new(2)
            }
        );
        assert!(receipt.report.consumed);
        assert!(matches!(
            receipt.report.rows,
            eutheto_core::CsvReportRows::Availability { .. }
        ));
    }
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await?
        .boxed()?;
    assert!(
        matches!(event.payload, EventPayload::ScenarioChanged { context, .. }
        if context.scenario_id == Some(id) && context.revision == Some(Revision::new(2)) && context.request_id == Some(event_request))
    );
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].id, command_id);
    drop(app);
    drop(store);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(history(&reopened, id).await?, entries);
    assert_eq!(
        view(&reopened, id).await?.document.domain.entities[&eligibility_endpoint(8).parse()?]["note"],
        "committed"
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn availability_csv_late_cancellation_and_discard_cannot_replace_committed_receipt()
-> TestResult {
    availability_commit_wins(false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn availability_csv_unknown_receipt_reconciles_committed_history_without_replay() -> TestResult
{
    availability_commit_wins(true).await
}

async fn matrix_preview(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    source: &[u8],
    skipped_cells: Vec<eutheto_core::EligibilityMatrixCellAddress>,
) -> Result<eutheto_core::EligibilityMatrixPreviewDtoV1, Box<dyn Error>> {
    app.preview_eligibility_matrix(
        eutheto_core::EligibilityMatrixPreviewRequestV1 {
            schema_version: 1,
            scenario_id,
            expected_revision: revision,
            mapping: eutheto_core::EligibilityMatrixMapping::default(),
            skipped_cells,
        },
        Cursor::new(source.to_vec()),
        app.csv_operation(),
    )
    .await
    .boxed()
}

fn matrix_apply_request(
    scenario_id: ScenarioId,
    revision: Revision,
    preview: &eutheto_core::EligibilityMatrixPreviewDtoV1,
) -> Result<CsvApplyRequestV1, Box<dyn Error>> {
    Ok(CsvApplyRequestV1 {
        schema_version: 1,
        preview_id: preview.preview_id,
        scenario_id,
        expected_revision: revision,
        approved_digest: preview
            .preview
            .approval_digest
            .clone()
            .ok_or("matrix approval")?,
        command_id: CommandId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
        actor: ActorRef {
            actor_id: None,
            display_name: "Matrix integration".to_owned(),
        },
        source: CommandSource::System,
        truncate_redo: false,
    })
}

#[tokio::test]
async fn eligibility_matrix_grouped_updates_reopen_with_exact_undo_redo() -> TestResult {
    use serde_json::json;
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    eligibility_seed(&app, id).await?;
    let before = view(&app, id).await?;
    // Reverse ID order is source order; blanks preserve the original raw membership spelling.
    let source = format!(
        "personId\t{}\t{}\t{}\t{}\n{}\ttrue\ttrue\t\tfalse\n{}\t\t\tfalse\t\n",
        eligibility_endpoint(23),
        eligibility_endpoint(21),
        eligibility_endpoint(4),
        eligibility_endpoint(22),
        eligibility_endpoint(1).to_ascii_uppercase(),
        eligibility_endpoint(13)
    );
    let review = matrix_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    assert_eq!(review.preview.cells, 8);
    let batch = review
        .preview
        .batch
        .as_ref()
        .ok_or("grouped matrix batch")?;
    assert_eq!(batch.commands.len(), 2);
    assert!(
        batch
            .commands
            .iter()
            .all(|command| command.command_type == eutheto_workforce::commands::UPDATE_ENTITY)
    );
    assert_eq!(review.preview.rows[2].before, Some(true));
    assert_eq!(review.preview.rows[2].after, Some(true));
    assert_eq!(view(&app, id).await?.document, before.document);
    let request = matrix_apply_request(id, Revision::new(1), &review)?;
    let receipt = app
        .apply_csv_import(request.clone(), Cursor::new(source), app.csv_operation())
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::Applied {
            command_id: request.command_id,
            revision: Revision::new(2)
        }
    );
    assert!(matches!(
        receipt.report.rows,
        eutheto_core::CsvReportRows::EligibilityMatrix { .. }
    ));
    let mut expected = before.document.clone();
    expected
        .domain
        .entities
        .get_mut(&eligibility_endpoint(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([
        eligibility_endpoint(4).to_ascii_uppercase(),
        eligibility_endpoint(23),
        eligibility_endpoint(21)
    ]);
    expected
        .domain
        .entities
        .get_mut(&eligibility_endpoint(13).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] =
        json!([eligibility_endpoint(22).to_ascii_uppercase()]);
    assert_eq!(view(&app, id).await?.document.domain, expected.domain);
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].id, request.command_id);
    drop(app);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(view(&reopened, id).await?.document.domain, expected.domain);
    reopened
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id: id,
            expected_revision: Revision::new(2),
        })
        .await
        .boxed()?;
    assert_eq!(
        view(&reopened, id).await?.document.domain,
        before.document.domain
    );
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id: id,
            expected_revision: Revision::new(3),
        })
        .await
        .boxed()?;
    assert_eq!(view(&reopened, id).await?.document.domain, expected.domain);
    assert_eq!(history(&reopened, id).await?, entries);
    Ok(())
}

#[tokio::test]
async fn eligibility_matrix_complete_report_bound_and_retained_authority() -> TestResult {
    use eutheto_core::{CsvReportRows, EligibilityMatrixCellAddress};
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let before = view(&app, id).await?;
    let source = "personId\tunknown\n".to_owned() + &"unknown\ttrue\n".repeat(10_000);
    let skips = (2..=10_001)
        .map(|record| EligibilityMatrixCellAddress { record, column: 1 })
        .collect();
    let review = matrix_preview(&app, id, Revision::INITIAL, source.as_bytes(), skips).await?;
    assert_eq!(
        review.preview.disposition,
        PeopleImportDisposition::NoChanges
    );
    let request = matrix_apply_request(id, Revision::INITIAL, &review)?;
    let mut wrong_digest = request.clone();
    wrong_digest.approved_digest = "0".repeat(64);
    assert_code(
        app.apply_csv_import(
            wrong_digest,
            Cursor::new(source.clone()),
            app.csv_operation(),
        )
        .await,
        "people_csv.approval_mismatch",
    );
    assert_code(
        app.apply_csv_import(
            request.clone(),
            Cursor::new(source.replacen("unknown", "different", 1)),
            app.csv_operation(),
        )
        .await,
        "eligibility_matrix.staleReview",
    );
    let mut forged = serde_json::to_value(&request)?;
    forged["family"] = serde_json::json!("eligibility");
    assert!(serde_json::from_value::<CsvApplyRequestV1>(forged).is_err());
    let retained = app.csv_report(review.preview_id).await.boxed()?;
    let CsvReportRows::EligibilityMatrix { excluded_cells } = &retained.rows else {
        return Err("matrix report family".into());
    };
    assert_eq!(excluded_cells.len(), 10_000);
    assert_eq!(
        excluded_cells[0].cell,
        EligibilityMatrixCellAddress {
            record: 2,
            column: 1
        }
    );
    assert_eq!(
        excluded_cells[9_999].cell,
        EligibilityMatrixCellAddress {
            record: 10_001,
            column: 1
        }
    );
    assert!(excluded_cells.iter().all(|cell| cell.skipped));
    assert!(
        serde_json::to_vec(excluded_cells)?.len()
            <= eutheto_core::MAX_ELIGIBILITY_MATRIX_REPORT_BYTES
    );
    let receipt = app
        .apply_csv_import(
            request.clone(),
            Cursor::new(source.clone()),
            app.csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        receipt.outcome,
        CsvApplyOutcomeV1::NoChanges {
            revision: Revision::INITIAL
        }
    );
    assert_eq!(receipt.report.rows, retained.rows);
    assert!(receipt.report.consumed);
    assert!(serde_json::to_vec(&receipt)?.len() > 128 * 1024);
    assert_eq!(
        app.csv_report(review.preview_id).await.boxed()?,
        receipt.report
    );
    assert_code(
        app.apply_csv_import(request, Cursor::new(source), app.csv_operation())
            .await,
        "people_csv.preview_consumed",
    );
    assert_eq!(view(&app, id).await?.document, before.document);
    assert!(history(&app, id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn eligibility_matrix_no_op_rechecks_actual_revision_after_rebuild() -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&app).await?;
    eligibility_seed(&app, id).await?;
    let source = format!(
        "personId\t{}\n{}\t\n",
        eligibility_endpoint(4),
        eligibility_endpoint(1)
    );
    let review = matrix_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    assert_eq!(
        review.preview.disposition,
        PeopleImportDisposition::NoChanges
    );
    let request = matrix_apply_request(id, Revision::new(1), &review)?;
    let second = EuthetoApp::open(deps).await.boxed()?;
    let changed = source.replace("\t\n", "\tfalse\n");
    let concurrent =
        matrix_preview(&second, id, Revision::new(1), changed.as_bytes(), vec![]).await?;
    let (reader, reached, release) = paused_reader(source.as_bytes());
    let caller_app = app.clone();
    let operation = app.csv_operation();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, reader, operation)
            .await
    });
    barrier(reached).await?;
    let concurrent_result = second
        .apply_csv_import(
            matrix_apply_request(id, Revision::new(1), &concurrent)?,
            Cursor::new(changed),
            second.csv_operation(),
        )
        .await;
    barrier(release).await?;
    concurrent_result.boxed()?;
    assert!(matches!(caller.await?,
        Err(AppError::Conflict { expected_revision, actual_revision })
        if expected_revision == Revision::new(1) && actual_revision == Revision::new(2)));
    assert_eq!(history(&app, id).await?.len(), 2);
    assert!(!app.csv_report(review.preview_id).await.boxed()?.consumed);
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[cfg(debug_assertions)]
async fn matrix_commit_wins(abandon: bool) -> TestResult {
    let directory = directory()?;
    let deps = dependencies(&directory)?;
    let seed_app = EuthetoApp::open(deps.clone()).await.boxed()?;
    let id = create(&seed_app).await?;
    eligibility_seed(&seed_app, id).await?;
    let before = view(&seed_app, id).await?.document;
    drop(seed_app);
    let hook = CommandCommitTestHook::new(CommandCommitTestPhase::AfterFinalCancellationCheck);
    let (store, initialized) = SqliteScenarioStore::open_with_options(
        &deps.paths.database,
        OpenOptions::new(SnapshotPolicy::default()).with_command_commit_test_hook(hook.clone()),
    )
    .await?;
    let store = Arc::new(store);
    let app =
        EuthetoApp::from_initialized_store(store.clone(), initialized, deps.clone()).boxed()?;
    let source = format!(
        "personId\t{}\t{}\n{}\ttrue\tfalse\n",
        eligibility_endpoint(23),
        eligibility_endpoint(22),
        eligibility_endpoint(1)
    );
    let review = matrix_preview(&app, id, Revision::new(1), source.as_bytes(), vec![]).await?;
    let request = matrix_apply_request(id, Revision::new(1), &review)?;
    let command_id = request.command_id;
    let event_request = request.request_id;
    let mut events = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let operation = app.csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_csv_import(request, Cursor::new(source), operation)
            .await
    });
    let reached = hook.clone();
    tokio::task::spawn_blocking(move || reached.wait_until_reached()).await?;
    let retained_caller = if abandon {
        caller.abort();
        assert!(caller.await.is_err_and(|error| error.is_cancelled()));
        None
    } else {
        signal.cancel();
        app.discard_csv_preview(review.preview_id).await.boxed()?;
        Some(caller)
    };
    tokio::task::spawn_blocking(move || hook.release()).await?;
    if let Some(caller) = retained_caller {
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(10), caller)
            .await??
            .boxed()?;
        assert_eq!(
            receipt.outcome,
            CsvApplyOutcomeV1::Applied {
                command_id,
                revision: Revision::new(2)
            }
        );
        assert!(receipt.report.consumed);
        assert!(matches!(
            receipt.report.rows,
            eutheto_core::CsvReportRows::EligibilityMatrix { .. }
        ));
        assert_code(
            app.csv_report(review.preview_id).await,
            "people_csv.preview_unavailable",
        );
    }
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await?
        .boxed()?;
    assert!(
        matches!(event.payload, EventPayload::ScenarioChanged { context, .. }
        if context.scenario_id == Some(id) && context.revision == Some(Revision::new(2))
            && context.request_id == Some(event_request))
    );
    if abandon {
        // Commit publishes before cache consumption; wait for the owner, never replay.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !app.csv_report(review.preview_id).await?.consumed {
                tokio::task::yield_now().await;
            }
            Ok::<(), AppError>(())
        })
        .await?
        .boxed()?;
    }
    let entries = history(&app, id).await?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].id, command_id);
    let mut expected = before;
    expected
        .domain
        .entities
        .get_mut(&eligibility_endpoint(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = serde_json::json!([
        eligibility_endpoint(4).to_ascii_uppercase(),
        eligibility_endpoint(23)
    ]);
    assert_eq!(view(&app, id).await?.document.domain, expected.domain);
    drop(app);
    drop(store);
    let reopened = EuthetoApp::open(deps).await.boxed()?;
    assert_eq!(history(&reopened, id).await?, entries);
    assert_eq!(view(&reopened, id).await?.document.domain, expected.domain);
    Ok(())
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn eligibility_matrix_late_cancellation_and_discard_preserve_committed_receipt() -> TestResult
{
    matrix_commit_wins(false).await
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn eligibility_matrix_abandoned_receipt_reconciles_durable_history_without_replay()
-> TestResult {
    matrix_commit_wins(true).await
}
