use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult, EuthetoApp,
    PeopleCsvApplyOutcomeV1, PeopleCsvApplyRequestV1, PeopleCsvPreviewDtoV1,
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
    Ok(tempfile::Builder::new()
        .prefix("eutheto-csv-test-")
        .tempdir_in(dirs::home_dir().ok_or("home directory unavailable")?)?)
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
        app.people_csv_operation(),
    )
    .await
    .boxed()
}
fn apply_request(
    id: ScenarioId,
    revision: Revision,
    preview: &PeopleCsvPreviewDtoV1,
) -> Result<PeopleCsvApplyRequestV1, Box<dyn Error>> {
    Ok(PeopleCsvApplyRequestV1 {
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
    let retained = app
        .people_csv_rejected_rows(review.preview_id)
        .await
        .boxed()?;
    assert!(!retained.consumed);
    let request = apply_request(id, Revision::INITIAL, &review)?;
    let command_id = request.command_id;
    let result = app
        .apply_people_csv_import(
            request.clone(),
            Cursor::new(SOURCE),
            app.people_csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        result.outcome,
        PeopleCsvApplyOutcomeV1::Applied {
            command_id,
            revision: Revision::new(1)
        }
    );
    assert_eq!(result.report.rejected_rows, retained.rejected_rows);
    assert!(result.report.consumed);
    assert_eq!(
        app.people_csv_rejected_rows(review.preview_id)
            .await
            .boxed()?,
        result.report
    );
    assert_code(
        app.apply_people_csv_import(request, Cursor::new(SOURCE), app.people_csv_operation())
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
        reopened.people_csv_rejected_rows(review.preview_id).await,
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
        app.apply_people_csv_import(
            apply_request(id, Revision::INITIAL, &blocked)?,
            Cursor::new(source),
            app.people_csv_operation(),
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
        .apply_people_csv_import(
            apply_request(id, Revision::INITIAL, &no_changes)?,
            Cursor::new(source),
            app.people_csv_operation(),
        )
        .await
        .boxed()?;
    assert_eq!(
        result.outcome,
        PeopleCsvApplyOutcomeV1::NoChanges {
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
        app.apply_people_csv_import(request, Cursor::new(source), app.people_csv_operation())
            .await,
        "people_csv.approval_mismatch",
    );
    assert_code(
        app.apply_people_csv_import(
            apply_request(id, Revision::INITIAL, &first)?,
            Cursor::new(b"external,name\none,Changed\n"),
            app.people_csv_operation(),
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
        .preview_people_csv(
            changed_request,
            Cursor::new(source),
            app.people_csv_operation(),
        )
        .await
        .boxed()?;
    let changed_decision = preview(&app, id, Revision::INITIAL, source, vec![add(2)?]).await?;
    for changed in [&changed_mapping, &changed_decision] {
        let mut request = apply_request(id, Revision::INITIAL, changed)?;
        request.approved_digest = first.preview.approval_digest.clone().ok_or("approval")?;
        assert_code(
            app.apply_people_csv_import(request, Cursor::new(source), app.people_csv_operation())
                .await,
            "people_csv.approval_mismatch",
        );
    }
    assert!(history(&app, id).await?.is_empty());
    let receipt = app
        .apply_people_csv_import(
            apply_request(id, Revision::INITIAL, &changed_mapping)?,
            Cursor::new(source),
            app.people_csv_operation(),
        )
        .await
        .boxed()?;
    assert!(matches!(
        receipt.outcome,
        PeopleCsvApplyOutcomeV1::Applied { .. }
    ));
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
    app.apply_people_csv_import(
        apply_request(id, Revision::new(1), &update)?,
        Cursor::new(b"external,name\none,Revised\n"),
        app.people_csv_operation(),
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
    let operation = app.people_csv_operation();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_people_csv_import(request, reader, operation)
            .await
    });
    barrier(reached).await?;
    second
        .apply_people_csv_import(
            apply_request(id, Revision::INITIAL, &concurrent)?,
            Cursor::new(source),
            second.people_csv_operation(),
        )
        .await
        .boxed()?;
    barrier(release).await?;
    assert!(
        matches!(caller.await?, Err(AppError::Conflict { expected_revision, actual_revision })
        if expected_revision == Revision::INITIAL && actual_revision == Revision::new(1))
    );
    assert_eq!(history(&app, id).await?.len(), 1);
    assert!(
        !app.people_csv_rejected_rows(review.preview_id)
            .await
            .boxed()?
            .consumed
    );
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
    app.apply_people_csv_import(
        apply_request(id, Revision::INITIAL, &first)?,
        Cursor::new(source),
        app.people_csv_operation(),
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
        matches!(app.apply_people_csv_import(request.clone(), Cursor::new(source), app.people_csv_operation()).await,
        Err(AppError::Validation(report)) if report.issues.iter().any(|issue| issue.code == "history.redo_branch_requires_truncation"))
    );
    assert!(!history(&app, id).await?[0].applied);
    request.truncate_redo = true;
    app.apply_people_csv_import(request, Cursor::new(source), app.people_csv_operation())
        .await
        .boxed()?;
    assert_eq!(history(&app, id).await?.len(), 1);
    assert!(history(&app, id).await?[0].applied);
    assert_eq!(view(&app, id).await?.revision, Revision::new(3));
    Ok(())
}

#[tokio::test]
async fn shared_preview_count_kind_isolation_and_discard_preserve_other_capabilities() -> TestResult
{
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let csv = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let AppQueryResult::Bundle { bytes, .. } =
        app.query(AppQuery::ExportScenario(id)).await.boxed()?
    else {
        return Err("export receipt".into());
    };
    let AppQueryResult::PortablePreview {
        preview_id: portable,
        ..
    } = app
        .query(AppQuery::PreviewImport {
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
        .query(AppQuery::InspectUnopenedBundle { bytes })
        .await
        .boxed()?
    else {
        return Err("opaque receipt".into());
    };
    for preview_id in [portable, opaque] {
        assert_code(
            app.discard_people_csv_preview(preview_id).await,
            "people_csv.preview_kind_mismatch",
        );
        assert_code(
            app.people_csv_rejected_rows(preview_id).await,
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
            request_id: request_id()?,
            preview_id: csv.preview_id,
            collision_plan: CollisionPlan::default(),
        })
        .await,
        "portable.preview_capability_mismatch",
    );
    assert_code(
        app.execute(AppCommand::ExactReexportUnopenedBundle {
            preview_id: csv.preview_id,
            destination: directory.path().join("wrong.zip"),
        })
        .await,
        "portable.preview_capability_mismatch",
    );
    assert!(
        !app.people_csv_rejected_rows(csv.preview_id)
            .await
            .boxed()?
            .consumed
    );
    // Three different kinds share one three-entry cache; the fourth evicts the oldest ID.
    let fourth = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    assert_code(
        app.people_csv_rejected_rows(csv.preview_id).await,
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
    app.discard_people_csv_preview(fourth.preview_id)
        .await
        .boxed()?;
    assert_code(
        app.people_csv_rejected_rows(fourth.preview_id).await,
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
    let operation = app.people_csv_operation();
    let signal = operation.cancellation();
    let sibling = app.people_csv_operation();
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
    let operation = app.people_csv_operation();
    let signal = operation.cancellation();
    let future = app.detect_people_csv(Cursor::new(SOURCE), operation);
    drop(future);
    assert!(signal.is_cancelled());
    assert!(!root.is_cancelled());
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
        app.detect_people_csv(
            CountedOverflow { read: read.clone() },
            app.people_csv_operation(),
        )
        .await,
        "people_csv.sourceByteLimit",
    );
    assert_eq!(
        read.load(std::sync::atomic::Ordering::SeqCst),
        MAX_CSV_SOURCE_BYTES + 1
    );
    let result = app
        .detect_people_csv(Failure, app.people_csv_operation())
        .await;
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
    let before = store.library_snapshot().await?;
    let review = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let request = apply_request(id, Revision::INITIAL, &review)?;
    let command_id = request.command_id;
    let event_request = request.request_id;
    let mut events = app.subscribe(EventTopic::ScenarioChanged).await.boxed()?;
    let operation = app.people_csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move {
        caller_app
            .apply_people_csv_import(request, Cursor::new(SOURCE), operation)
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
        app.discard_people_csv_preview(review.preview_id)
            .await
            .boxed()?;
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
                PeopleCsvApplyOutcomeV1::Applied {
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
        let after = store.library_snapshot().await?;
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
        assert!(
            !app.people_csv_rejected_rows(review.preview_id)
                .await
                .boxed()?
                .consumed
        );
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
                app.people_csv_rejected_rows(preview_id).await,
                "people_csv.preview_unavailable",
            );
        } else {
            loop {
                if app.people_csv_rejected_rows(preview_id).await?.consumed {
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
    let operation = app.people_csv_operation();
    let signal = operation.cancellation();
    let caller_app = app.clone();
    let caller = tokio::spawn(async move { caller_app.detect_people_csv(reader, operation).await });
    barrier(reached).await?;
    caller.abort();
    assert!(caller.await.is_err_and(|error| error.is_cancelled()));
    assert!(signal.is_cancelled());
    assert!(!root.is_cancelled());
    app.detect_people_csv(Cursor::new(SOURCE), app.people_csv_operation())
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
        entries.insert(entry.name().to_owned(), contents);
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
    let bytes = match app.query(AppQuery::ExportScenario(id)).await.boxed()? {
        AppQueryResult::Bundle { bytes, .. } => large_opaque_bundle(&bytes)?,
        _ => return Err("wrong export receipt".into()),
    };
    let AppQueryResult::UnopenedBundlePreview {
        preview_id: first, ..
    } = app
        .query(AppQuery::InspectUnopenedBundle {
            bytes: bytes.clone(),
        })
        .await
        .boxed()?
    else {
        return Err("wrong opaque receipt".into());
    };
    assert!(
        !app.people_csv_rejected_rows(csv.preview_id)
            .await
            .boxed()?
            .consumed
    );
    // Only two entries existed before this call. Count alone cannot explain eviction.
    let AppQueryResult::UnopenedBundlePreview {
        preview_id: second, ..
    } = app
        .query(AppQuery::InspectUnopenedBundle { bytes })
        .await
        .boxed()?
    else {
        return Err("wrong opaque receipt".into());
    };
    assert_code(
        app.people_csv_rejected_rows(csv.preview_id).await,
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
    assert!(serde_json::from_value::<PeopleCsvApplyRequestV1>(forged).is_err());
    let mut unsupported = request;
    unsupported.schema_version = 2;
    assert_code(
        app.apply_people_csv_import(unsupported, Cursor::new(SOURCE), app.people_csv_operation())
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
        let operation = app.people_csv_operation();
        let caller_app = app.clone();
        callers.push(tokio::spawn(async move {
            caller_app
                .apply_people_csv_import(request, reader, operation)
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
        [PeopleCsvApplyOutcomeV1::Applied {
            command_id: entries[0].id,
            revision: Revision::new(1)
        }]
    );
    assert!(
        app.people_csv_rejected_rows(preview.preview_id)
            .await
            .boxed()?
            .consumed
    );
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
            app.people_csv_operation(),
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
    struct CancellingReader(eutheto_core::PeopleCsvCancellation);
    impl Read for CancellingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.cancel();
            Ok(0)
        }
    }
    let directory = directory()?;
    let app = EuthetoApp::open(dependencies(&directory)?).await.boxed()?;
    let id = create(&app).await?;
    let operation = app.people_csv_operation();
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
        let operation = app.people_csv_operation();
        let caller_app = app.clone();
        callers.push(tokio::spawn(async move {
            caller_app
                .apply_people_csv_import(request, reader, operation)
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
                    PeopleCsvApplyOutcomeV1::NoChanges {
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
    assert!(
        app.people_csv_rejected_rows(review.preview_id)
            .await
            .boxed()?
            .consumed
    );
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
    let AppQueryResult::Bundle { bytes, .. } =
        app.query(AppQuery::ExportScenario(id)).await.boxed()?
    else {
        return Err("expected portable source".into());
    };
    ids.repeat.store(true, std::sync::atomic::Ordering::SeqCst);
    let csv = preview(&app, id, Revision::INITIAL, SOURCE, vec![add(2)?, add(4)?]).await?;
    let query = if opaque {
        AppQuery::InspectUnopenedBundle { bytes }
    } else {
        AppQuery::PreviewImport {
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        }
    };
    assert_code(app.query(query).await, "portable.preview_id_unavailable");
    assert!(
        !app.people_csv_rejected_rows(csv.preview_id)
            .await
            .boxed()?
            .consumed
    );
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
