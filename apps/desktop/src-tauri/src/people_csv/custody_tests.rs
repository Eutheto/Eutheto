use super::*;
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    PeopleCsvPreviewRequestV1,
};
use eutheto_import::{ImportOptions, RestoreMode};
use eutheto_types::{
    DomainPackRef, FixedClock, FixedMonotonicClock, GapPolicy, Horizon, OverlapPolicy,
    ScenarioSettings, SystemIdGenerator, UnitSystem,
};
use std::{error::Error, time::Duration};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    io::Error::other(format!("{error:?}")).into()
}
fn creator() -> TestResult<Creator> {
    Ok(Creator {
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: RequestId::new(&SystemIdGenerator)?,
    })
}
fn source_target(id: SourceId) -> SourceTarget {
    SourceTarget::Source { source_id: id }
}
fn creator_source(value: Creator) -> SourceTarget {
    SourceTarget::Creator {
        operation_id: value.operation_id,
        request_id: value.request_id,
    }
}
fn creator_preview(value: Creator) -> PreviewTarget {
    PreviewTarget::Creator {
        operation_id: value.operation_id,
        request_id: value.request_id,
    }
}
async fn fixture() -> TestResult<(tempfile::TempDir, Arc<CsvCustody>, ScenarioId)> {
    let directory = tempfile::tempdir()?;
    let ids = Arc::new(SystemIdGenerator);
    let app = EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("custody.sqlite3"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new("2026-09-10T12:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: ids.clone(),
        cancellation: CancellationToken::new(),
    })
    .await
    .map_err(boxed)?;
    let result = app
        .execute(AppCommand::CreateProject {
            request_id: RequestId::new(&SystemIdGenerator)?,
            title: "CSV custody".to_owned(),
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
        .map_err(boxed)?;
    let AppCommandResult::Project(project) = result else {
        return Err("wrong create receipt".into());
    };
    Ok((
        directory,
        Arc::new(CsvCustody::new(app, ids)),
        project.scenario_id,
    ))
}
fn reserve(custody: &Arc<CsvCustody>, scenario: ScenarioId) -> TestResult<SourceReservation> {
    custody
        .reserve_source("main", scenario, creator()?, CancellationToken::new())
        .map_err(boxed)
}
fn source(custody: &Arc<CsvCustody>, scenario: ScenarioId) -> TestResult<SourceUse> {
    let id = reserve(custody, scenario)?
        .publish(b"name\nAda\n".to_vec())
        .map_err(boxed)?;
    custody.source("main", scenario, id).map_err(boxed)
}
async fn core_preview(
    custody: &Arc<CsvCustody>,
    scenario: ScenarioId,
    source: &SourceUse,
) -> TestResult<RequestId> {
    // Use the real core contract, avoiding a native test-only dependency on the domain crate.
    let request: PeopleCsvPreviewRequestV1 = serde_json::from_value(serde_json::json!({
        "schemaVersion": 1, "scenarioId": scenario, "expectedRevision": 0,
        "mapping": {
            "dialect": "comma", "hasHeader": true, "expectedColumns": 1,
            "columns": [{"index": 0, "field": "name", "blank": "preserve"}],
            "newPersonDefaults": {
                "activeRange": {"kind": "always"}, "qualificationGrants": [],
                "eligibleAssignmentTypeIds": [],
                "workloadWeight": {"numerator": 1, "denominator": 1},
                "tags": [], "teamIds": []
            }, "referenceMappings": {}
        }, "decisions": []
    }))?;
    Ok(custody
        .app
        .preview_people_csv(request, source.reader(), custody.app.people_csv_operation())
        .await
        .map_err(boxed)?
        .preview_id)
}
async fn bind(
    custody: &Arc<CsvCustody>,
    scenario: ScenarioId,
    source: &SourceUse,
) -> TestResult<RequestId> {
    let reservation = custody
        .reserve_preview(
            source,
            Revision::INITIAL,
            creator()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let id = core_preview(custody, scenario, source).await?;
    reservation.publish(id).map_err(boxed)
}
async fn wait_for_preview_slot(
    custody: &Arc<CsvCustody>,
    source: &SourceUse,
) -> TestResult<PreviewReservation> {
    let creator = creator()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match custody.reserve_preview(
                source,
                Revision::INITIAL,
                creator,
                CancellationToken::new(),
            ) {
                Ok(reservation) => return Ok(reservation),
                Err(error) if error.code == "people_csv.native_capacity" => {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(boxed(error)),
            }
        }
    })
    .await?
}

#[tokio::test]
async fn pending_and_closed_reader_sources_stay_charged_without_copying() -> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let bytes = b"name\nAda\n".to_vec();
    let allocation = bytes.as_ptr();
    let id = reserve(&custody, scenario)?.publish(bytes).map_err(boxed)?;
    let use_guard = custody.source("main", scenario, id).map_err(boxed)?;
    let mut reader = use_guard.reader();
    assert_eq!(
        reader.lease.bytes.as_ptr(),
        allocation,
        "publication and reader share the selected allocation"
    );
    let second = reserve(&custody, scenario)?;
    let third = reserve(&custody, scenario)?;
    assert!(
        reserve(&custody, scenario).is_err(),
        "pending acquisition already occupies a slot"
    );
    custody.close_source("main", scenario, &source_target(id));
    assert!(custody.source("main", scenario, id).is_err());
    let mut text = String::new();
    reader.read_to_string(&mut text)?;
    assert_eq!(text, "name\nAda\n");
    drop(reader);
    assert!(
        reserve(&custody, scenario).is_err(),
        "outer guard survives parser completion"
    );
    let mut late_reader = use_guard.reader();
    drop(use_guard);
    assert!(
        reserve(&custody, scenario).is_err(),
        "reader also owns the charge"
    );
    let mut text = String::new();
    late_reader.read_to_string(&mut text)?;
    assert_eq!(text, "name\nAda\n");
    drop(late_reader);
    let replacement = reserve(&custody, scenario)?;
    drop((replacement, second, third));
    Ok(())
}

#[tokio::test]
async fn three_bounded_sources_enforce_logical_byte_capacity_and_failed_reservations_retire()
-> TestResult {
    const SOURCE_BYTES: usize = 16 * 1024 * 1024;
    let (_directory, custody, scenario) = fixture().await?;
    assert_eq!(
        reserve(&custody, scenario)?
            .publish(vec![0; SOURCE_BYTES + 1])
            .err()
            .ok_or("oversized source accepted")?
            .code,
        "people_csv.source_too_large"
    );
    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = reserve(&custody, scenario)?
            .publish(vec![0; SOURCE_BYTES])
            .map_err(boxed)?;
        ids.push(id);
    }
    assert!(reserve(&custody, scenario).is_err());
    for id in ids {
        custody.close_source("main", scenario, &source_target(id));
    }
    let pending = reserve(&custody, scenario)?;
    let others = [reserve(&custody, scenario)?, reserve(&custody, scenario)?];
    drop(pending);
    let replacement = reserve(&custody, scenario)?;
    drop((replacement, others));
    Ok(())
}

#[tokio::test]
async fn source_cancellation_creator_cleanup_and_owner_checks_do_not_transfer_authority()
-> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let old = creator()?;
    let cancellation = CancellationToken::new();
    let pending = custody
        .reserve_source("main", scenario, old, cancellation.clone())
        .map_err(boxed)?;
    assert!(
        custody
            .reserve_source("main", scenario, old, CancellationToken::new())
            .is_err()
    );
    cancellation.cancel();
    assert_eq!(
        pending
            .publish(vec![1])
            .err()
            .ok_or("cancelled source published")?
            .code,
        "operation.cancelled"
    );
    assert!(
        custody
            .reserve_source("main", scenario, creator()?, cancellation)
            .is_err()
    );
    let new = Creator {
        operation_id: creator()?.operation_id,
        request_id: old.request_id,
    };
    let cancellation = CancellationToken::new();
    let id = custody
        .reserve_source("main", scenario, new, cancellation.clone())
        .map_err(boxed)?
        .publish(vec![7])
        .map_err(boxed)?;
    cancellation.cancel();
    custody.close_source("main", scenario, &creator_source(old));
    custody.close_source("other", scenario, &source_target(id));
    let other_scenario = ScenarioId::new(&SystemIdGenerator)?;
    custody.close_source("main", other_scenario, &source_target(id));
    assert!(custody.source("other", scenario, id).is_err());
    assert!(custody.source("main", other_scenario, id).is_err());
    let mut reader = custody
        .source("main", scenario, id)
        .map_err(boxed)?
        .reader();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    assert_eq!(
        bytes,
        [7],
        "cancelling the creator does not close an owned source"
    );
    custody.close_source("main", scenario, &creator_source(new));
    custody.close_source("main", scenario, &creator_source(new));
    assert!(custody.source("main", scenario, id).is_err());
    Ok(())
}

#[tokio::test]
async fn reports_survive_source_close_but_apply_requires_exact_binding_and_discard_waits_for_use()
-> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let source = source(&custody, scenario)?;
    let preview = bind(&custody, scenario, &source).await?;
    assert!(
        custody
            .preview_for_report("other", scenario, preview)
            .is_err()
    );
    assert!(
        custody
            .preview_for_report("main", ScenarioId::new(&SystemIdGenerator)?, preview)
            .is_err()
    );
    assert!(
        custody
            .preview_for_apply(
                "main",
                scenario,
                source.source_id(),
                Revision::new(1),
                preview
            )
            .is_err()
    );
    assert!(
        custody
            .preview_for_apply(
                "main",
                scenario,
                SourceId(RequestId::new(&SystemIdGenerator)?),
                Revision::INITIAL,
                preview
            )
            .is_err()
    );
    let active = custody
        .preview_for_apply(
            "main",
            scenario,
            source.source_id(),
            Revision::INITIAL,
            preview,
        )
        .map_err(boxed)?;
    custody.close_source("main", scenario, &source_target(source.source_id()));
    let report_use = custody
        .preview_for_report("main", scenario, preview)
        .map_err(boxed)?;
    assert_eq!(
        custody
            .app
            .people_csv_rejected_rows(report_use.preview_id())
            .await
            .map_err(boxed)?
            .preview_id,
        preview
    );
    custody.discard_preview(
        "other",
        scenario,
        &PreviewTarget::Preview {
            preview_id: preview,
        },
    );
    assert!(
        custody
            .preview_for_report("main", scenario, preview)
            .is_ok()
    );
    custody.discard_preview(
        "main",
        scenario,
        &PreviewTarget::Preview {
            preview_id: preview,
        },
    );
    assert!(
        custody
            .preview_for_report("main", scenario, preview)
            .is_err()
    );
    drop(report_use);
    assert!(
        custody
            .app
            .people_csv_rejected_rows(active.preview_id())
            .await
            .is_ok(),
        "active receipt guard defers core discard"
    );
    drop(active);
    tokio::time::timeout(Duration::from_secs(5), async {
        while custody.app.people_csv_rejected_rows(preview).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn unavailable_core_report_and_active_discard_cannot_exhaust_native_preview_slots()
-> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let source = source(&custody, scenario)?;
    let preview = bind(&custody, scenario, &source).await?;
    let active = custody
        .preview_for_report("main", scenario, preview)
        .map_err(boxed)?;
    let pending = [
        custody
            .reserve_preview(
                &source,
                Revision::INITIAL,
                creator()?,
                CancellationToken::new(),
            )
            .map_err(boxed)?,
        custody
            .reserve_preview(
                &source,
                Revision::INITIAL,
                creator()?,
                CancellationToken::new(),
            )
            .map_err(boxed)?,
    ];
    custody
        .app
        .discard_people_csv_preview(preview)
        .await
        .map_err(boxed)?;
    assert!(
        custody
            .app
            .people_csv_rejected_rows(active.preview_id())
            .await
            .is_err()
    );
    custody.discard_preview(
        "main",
        scenario,
        &PreviewTarget::Preview {
            preview_id: preview,
        },
    );
    assert!(
        custody
            .reserve_preview(
                &source,
                Revision::INITIAL,
                creator()?,
                CancellationToken::new()
            )
            .is_err()
    );
    drop(active);
    let replacement = wait_for_preview_slot(&custody, &source).await?;
    drop((replacement, pending));
    Ok(())
}

#[tokio::test]
async fn late_cancelled_preview_publication_is_finalized_and_creator_pair_stays_distinct()
-> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let source = source(&custody, scenario)?;
    let old = creator()?;
    let cancellation = CancellationToken::new();
    let pending = custody
        .reserve_preview(&source, Revision::INITIAL, old, cancellation.clone())
        .map_err(boxed)?;
    assert!(
        custody
            .reserve_preview(&source, Revision::INITIAL, old, CancellationToken::new())
            .is_err()
    );
    custody.discard_preview("main", scenario, &creator_preview(old));
    assert!(
        custody
            .reserve_preview(&source, Revision::INITIAL, old, CancellationToken::new())
            .is_err()
    );
    let preview = core_preview(&custody, scenario, &source).await?;
    cancellation.cancel();
    assert_eq!(
        pending
            .publish(preview)
            .err()
            .ok_or("cancelled preview published")?
            .code,
        "operation.cancelled"
    );
    let new = Creator {
        operation_id: creator()?.operation_id,
        request_id: old.request_id,
    };
    let replacement = custody
        .reserve_preview(&source, Revision::INITIAL, new, CancellationToken::new())
        .map_err(boxed)?;
    custody.discard_preview("main", scenario, &creator_preview(old));
    let next_preview = core_preview(&custody, scenario, &source).await?;
    replacement.publish(next_preview).map_err(boxed)?;
    let active = custody
        .preview_for_report("main", scenario, next_preview)
        .map_err(boxed)?;
    assert!(
        custody
            .app
            .people_csv_rejected_rows(active.preview_id())
            .await
            .is_ok()
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while custody.app.people_csv_rejected_rows(preview).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn wrong_kind_core_discard_retires_native_slot_without_removing_portable_authority()
-> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let source = source(&custody, scenario)?;
    let AppQueryResult::Bundle { bytes, .. } = custody
        .app
        .query(AppQuery::ExportScenario {
            scenario_id: scenario,
            cancellation: custody.app.setup_cancellation(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong export receipt".into());
    };
    let AppQueryResult::PortablePreview { preview_id, .. } = custody
        .app
        .query(AppQuery::PreviewImport {
            cancellation: custody.app.setup_cancellation(),
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ImportScenario,
                include_results: false,
                include_assets: false,
            },
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong portable preview receipt".into());
    };
    custody
        .reserve_preview(
            &source,
            Revision::INITIAL,
            creator()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?
        .publish(preview_id)
        .map_err(boxed)?;
    let pending = [
        custody
            .reserve_preview(
                &source,
                Revision::INITIAL,
                creator()?,
                CancellationToken::new(),
            )
            .map_err(boxed)?,
        custody
            .reserve_preview(
                &source,
                Revision::INITIAL,
                creator()?,
                CancellationToken::new(),
            )
            .map_err(boxed)?,
    ];
    custody.discard_preview("main", scenario, &PreviewTarget::Preview { preview_id });
    let replacement = wait_for_preview_slot(&custody, &source).await?;
    let error = custody
        .app
        .people_csv_rejected_rows(preview_id)
        .await
        .err()
        .ok_or("portable preview treated as CSV")?;
    let eutheto_types::AppError::Protocol(error) = error else {
        return Err("wrong kind error category".into());
    };
    assert_eq!(error.code, "people_csv.preview_kind_mismatch");
    custody
        .app
        .execute(AppCommand::CancelPortablePreview { preview_id })
        .await
        .map_err(boxed)?;
    drop((replacement, pending));
    Ok(())
}

#[tokio::test]
async fn window_close_and_shutdown_reject_late_publication_without_releasing_active_charges()
-> TestResult {
    let (_directory, custody, scenario) = fixture().await?;
    let source = source(&custody, scenario)?;
    let pending_source = reserve(&custody, scenario)?;
    let pending_preview = custody
        .reserve_preview(
            &source,
            Revision::INITIAL,
            creator()?,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let preview = core_preview(&custody, scenario, &source).await?;
    custody.close_window("main");
    assert!(
        custody
            .source("main", scenario, source.source_id())
            .is_err()
    );
    assert!(reserve(&custody, scenario).is_err());
    assert!(pending_source.publish(vec![1]).is_err());
    assert!(pending_preview.publish(preview).is_err());
    let mut bytes = Vec::new();
    source.reader().read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"name\nAda\n");
    let other = custody
        .reserve_source("other", scenario, creator()?, CancellationToken::new())
        .map_err(boxed)?;
    custody.shutdown();
    assert!(other.publish(vec![1]).is_err());
    assert!(
        custody
            .reserve_source("other", scenario, creator()?, CancellationToken::new())
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn source_identifier_collisions_are_bounded_without_replacing_live_bytes() -> TestResult {
    let (_directory, original, scenario) = fixture().await?;
    let repeated = RequestId::new(&SystemIdGenerator)?;
    let fresh = RequestId::new(&SystemIdGenerator)?;
    let ids = eutheto_types::FixedIdGenerator::new(
        std::iter::repeat_n(repeated.as_uuid(), MAX_ID_ATTEMPTS + 1)
            .chain(std::iter::once(fresh.as_uuid())),
    );
    let custody = Arc::new(CsvCustody::new(original.app.clone(), Arc::new(ids)));
    let id = reserve(&custody, scenario)?
        .publish(vec![42])
        .map_err(boxed)?;
    let error = custody
        .reserve_source("main", scenario, creator()?, CancellationToken::new())
        .err()
        .ok_or("collisions replaced a live source")?;
    assert_eq!(error.code, "people_csv.id_unavailable");
    let replacement = reserve(&custody, scenario)?
        .publish(vec![9])
        .map_err(boxed)?;
    assert_ne!(replacement, id);
    let mut bytes = Vec::new();
    custody
        .source("main", scenario, id)
        .map_err(boxed)?
        .reader()
        .read_to_end(&mut bytes)?;
    assert_eq!(bytes, [42]);
    Ok(())
}
