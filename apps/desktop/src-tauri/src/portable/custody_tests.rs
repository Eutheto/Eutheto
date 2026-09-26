use super::*;
use eutheto_core::{
    AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult, BackupSelection,
    PreparedPortableBinding, SafetyBackupOutcome,
};
use eutheto_import::{
    CollisionPlan, ImportOptions, RestoreAuthorization, RestoreMode, SafetyBackupEvidence,
};
use eutheto_types::{
    AppError, DomainPackRef, FixedClock, FixedIdGenerator, FixedMonotonicClock, GapPolicy, Horizon,
    OverlapPolicy, ScenarioSettings, SystemIdGenerator, UnitSystem,
};
use std::{error::Error, io, time::Duration};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    io::Error::other(format!("{error:?}")).into()
}

fn request_id() -> TestResult<RequestId> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}

fn creator() -> TestResult<Creator> {
    Ok(Creator {
        operation_id: OperationId::new(&SystemIdGenerator)?,
        request_id: request_id()?,
    })
}

fn creator_target(creator: Creator) -> PreviewTarget {
    PreviewTarget::Creator {
        operation_id: creator.operation_id,
        request_id: creator.request_id,
    }
}

async fn fixture() -> TestResult<(tempfile::TempDir, Arc<PortableCustody>, ScenarioId)> {
    let directory = tempfile::tempdir()?;
    // Increasing UUIDv7 values make real core-cache eviction deterministic.
    let ids = (1..=256)
        .map(|index| format!("01900000-0000-7000-8000-{index:012x}").parse())
        .collect::<Result<Vec<_>, _>>()?;
    let app = EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("custody.sqlite3"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new("2026-09-10T12:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(FixedIdGenerator::new(ids)),
        cancellation: CancellationToken::new(),
    })
    .await
    .map_err(boxed)?;
    let AppCommandResult::Project(project) = app
        .execute(AppCommand::CreateProject {
            request_id: request_id()?,
            title: "Portable custody".to_owned(),
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
        .map_err(boxed)?
    else {
        return Err("wrong create receipt".into());
    };
    Ok((
        directory,
        Arc::new(PortableCustody::new(app)),
        project.scenario_id,
    ))
}

fn reserve(
    custody: &Arc<PortableCustody>,
    owner: &str,
    kind: ReviewKind,
) -> TestResult<PreviewReservation> {
    custody
        .reserve(owner, creator()?, kind, CancellationToken::new())
        .map_err(boxed)
}

async fn backup(custody: &PortableCustody) -> TestResult<Vec<u8>> {
    let AppQueryResult::BackupBundle { bytes, .. } = custody
        .app
        .query(AppQuery::ExportBackup {
            title: "Custody backup".to_owned(),
            selection: BackupSelection::default(),
            cancellation: custody.app.setup_cancellation(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong backup receipt".into());
    };
    Ok(bytes)
}

async fn core_restore(
    custody: &PortableCustody,
    bytes: Vec<u8>,
) -> TestResult<(RequestId, Revision)> {
    let AppQueryResult::PortablePreview {
        preview_id,
        preview,
    } = custody
        .app
        .query(AppQuery::PreviewRestore {
            bytes,
            options: ImportOptions {
                restore_mode: RestoreMode::ReplaceLibrary,
                include_results: true,
                include_assets: true,
            },
            cancellation: custody.app.setup_cancellation(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong restore preview receipt".into());
    };
    Ok((preview_id, preview.binding.local_library_revision))
}

async fn bound_restore(
    custody: &Arc<PortableCustody>,
    owner: &str,
    bytes: Vec<u8>,
) -> TestResult<(RequestId, Revision)> {
    let reservation = reserve(custody, owner, ReviewKind::Restore)?;
    let (preview_id, library_revision) = core_restore(custody, bytes).await?;
    reservation
        .publish_core(preview_id, ReviewBinding::Restore { library_revision })
        .map_err(boxed)?;
    Ok((preview_id, library_revision))
}

async fn apply_restore(
    custody: &PortableCustody,
    preview_id: RequestId,
    safety_backup: SafetyBackupEvidence,
) -> TestResult<Result<AppCommandResult, AppError>> {
    Ok(custody
        .app
        .execute(AppCommand::ApplyRestore {
            request_id: request_id()?,
            preview_id,
            collision_plan: CollisionPlan::default(),
            authorization: RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
            cancellation: custody.app.setup_cancellation(),
        })
        .await)
}

fn disable_safety_backups(directory: &tempfile::TempDir) -> TestResult {
    let path = directory.path().join("backups");
    if path.is_dir() {
        std::fs::remove_dir_all(&path)?;
    }
    std::fs::write(path, b"not a directory")?;
    Ok(())
}

async fn fail_safety_backup(
    custody: &PortableCustody,
    preview_id: RequestId,
    library_revision: Revision,
) -> TestResult {
    assert!(matches!(
        apply_restore(custody, preview_id, SafetyBackupEvidence::NotRequired).await?,
        Err(AppError::Protocol(error)) if error.code == "restore.safety_backup_failed"
    ));
    assert!(
        custody
            .app
            .portable_restore_retry_is_retained(preview_id, library_revision)
            .await
    );
    Ok(())
}

async fn wait_for_restore_reservation(
    custody: &Arc<PortableCustody>,
    owner: &str,
    creator: Creator,
) -> TestResult<PreviewReservation> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match custody.reserve(
                owner,
                creator,
                ReviewKind::Restore,
                CancellationToken::new(),
            ) {
                Ok(reservation) => return Ok(reservation),
                Err(error)
                    if error.code == "portable.preview_capacity"
                        || error.code == "portable.preview_not_found" =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(boxed(error)),
            }
        }
    })
    .await?
}

async fn wait_for_retry_removal(
    custody: &PortableCustody,
    preview_id: RequestId,
    library_revision: Revision,
) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        while custody
            .app
            .portable_restore_retry_is_retained(preview_id, library_revision)
            .await
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Rejected bindings and the subsequent real publication form one capability lifecycle."
)]
async fn rejected_export_bindings_preserve_the_rightful_publication() -> TestResult {
    let (directory, custody, scenario_id) = fixture().await?;
    let AppQueryResult::Bundle {
        bytes,
        scenario_revision,
        library_revision,
    } = custody
        .app
        .query(AppQuery::ExportScenario {
            scenario_id,
            cancellation: custody.app.setup_cancellation(),
        })
        .await
        .map_err(boxed)?
    else {
        return Err("wrong export receipt".into());
    };
    let digest = eutheto_export::sha256_hex(&bytes);
    let preview_id = request_id()?;
    let creator = creator()?;
    custody
        .reserve(
            "main",
            creator,
            ReviewKind::ScenarioExport,
            CancellationToken::new(),
        )
        .map_err(boxed)?
        .publish_prepared(
            preview_id,
            PreparedPortableOutput {
                bytes,
                sha256: digest.clone(),
                kind: PreparedPortableKind::Scenario {
                    scenario_id,
                    revision: scenario_revision,
                    library_revision,
                    title: "Portable custody".to_owned(),
                },
            },
        )
        .map_err(boxed)?;
    let binding = ReviewBinding::ScenarioExport {
        scenario_id,
        scenario_revision,
        library_revision,
    };
    custody.discard("other", &PreviewTarget::Preview { preview_id });
    custody.discard("other", &creator_target(creator));
    assert!(custody.acquire("other", preview_id, binding).is_err());
    for wrong_binding in [
        ReviewBinding::Backup { library_revision },
        ReviewBinding::ScenarioExport {
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            scenario_revision,
            library_revision,
        },
        ReviewBinding::ScenarioExport {
            scenario_id,
            scenario_revision: Revision::new(scenario_revision.value() + 1),
            library_revision,
        },
        ReviewBinding::ScenarioExport {
            scenario_id,
            scenario_revision,
            library_revision: Revision::new(library_revision.value() + 1),
        },
    ] {
        assert_eq!(
            custody
                .acquire("main", preview_id, wrong_binding)
                .err()
                .ok_or("mismatched export acquired")?
                .code,
            "portable.preview_binding_mismatch"
        );
    }
    let mut lease = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    assert!(custody.acquire("main", preview_id, binding).is_err());
    let output = lease.take_output().map_err(boxed)?;
    let destination = directory.path().join("reviewed.eutheto");
    assert!(matches!(
        custody
            .app
            .execute(AppCommand::PublishPreparedPortable {
                destination: destination.clone(),
                bytes: output.bytes,
                expected_sha256: output.sha256,
                binding: PreparedPortableBinding::Scenario {
                    scenario_id,
                    expected_revision: scenario_revision,
                    expected_library_revision: library_revision,
                },
                cancellation: custody.app.setup_cancellation(),
            })
            .await
            .map_err(boxed)?,
        AppCommandResult::BundleWritten
    ));
    assert_eq!(
        eutheto_export::sha256_hex(&std::fs::read(destination)?),
        digest
    );
    drop(lease);
    assert!(custody.acquire("main", preview_id, binding).is_err());
    Ok(())
}

#[tokio::test]
async fn core_and_prepared_reservations_have_independent_three_entry_capacity() -> TestResult {
    let (_directory, custody, _scenario) = fixture().await?;
    let core = [
        reserve(&custody, "main", ReviewKind::Import)?,
        reserve(&custody, "main", ReviewKind::Restore)?,
        reserve(&custody, "main", ReviewKind::Unopened)?,
    ];
    let prepared = [
        reserve(&custody, "main", ReviewKind::ScenarioExport)?,
        reserve(&custody, "main", ReviewKind::Backup)?,
        reserve(&custody, "other", ReviewKind::Backup)?,
    ];
    assert_eq!(
        custody
            .reserve(
                "other",
                creator()?,
                ReviewKind::Restore,
                CancellationToken::new()
            )
            .err()
            .ok_or("fourth core reservation admitted")?
            .code,
        "portable.preview_capacity"
    );
    assert_eq!(
        custody
            .reserve(
                "other",
                creator()?,
                ReviewKind::Backup,
                CancellationToken::new()
            )
            .err()
            .ok_or("fourth prepared reservation admitted")?
            .code,
        "portable.preview_capacity"
    );
    let [first, second, third] = core;
    drop(first);
    let replacement = reserve(&custody, "other", ReviewKind::Restore)?;
    assert_eq!(
        custody
            .reserve(
                "other",
                creator()?,
                ReviewKind::Backup,
                CancellationToken::new()
            )
            .err()
            .ok_or("core release freed prepared capacity")?
            .code,
        "portable.preview_capacity"
    );
    let [first_prepared, second_prepared, third_prepared] = prepared;
    drop(first_prepared);
    let prepared_replacement = reserve(&custody, "other", ReviewKind::ScenarioExport)?;
    assert_eq!(
        custody
            .reserve(
                "other",
                creator()?,
                ReviewKind::Import,
                CancellationToken::new()
            )
            .err()
            .ok_or("prepared release freed core capacity")?
            .code,
        "portable.preview_capacity"
    );
    drop((
        second,
        third,
        replacement,
        second_prepared,
        third_prepared,
        prepared_replacement,
    ));
    Ok(())
}

#[tokio::test]
async fn archive_bytes_stay_charged_until_the_publication_lease_settles() -> TestResult {
    let (_directory, custody, _scenario) = fixture().await?;
    let preview_id = request_id()?;
    let binding = ReviewBinding::Backup {
        library_revision: Revision::INITIAL,
    };
    // Opaque bytes exercise custody's quota only; this is not a core-valid archive or publication.
    reserve(&custody, "main", ReviewKind::Backup)?
        .publish_prepared(
            preview_id,
            PreparedPortableOutput {
                bytes: vec![0; usize::try_from(PORTABLE_LIMITS.max_archive_bytes)?],
                sha256: String::new(),
                kind: PreparedPortableKind::Backup {
                    library_revision: Revision::INITIAL,
                    title: "Quota boundary".to_owned(),
                },
            },
        )
        .map_err(boxed)?;
    let mut lease = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    let output = lease.take_output().map_err(boxed)?;
    custody.discard("main", &PreviewTarget::Preview { preview_id });
    assert!(custody.acquire("main", preview_id, binding).is_err());
    let error = reserve(&custody, "other", ReviewKind::Backup)?
        .publish_prepared(
            request_id()?,
            PreparedPortableOutput {
                bytes: vec![1],
                sha256: String::new(),
                kind: PreparedPortableKind::Backup {
                    library_revision: Revision::INITIAL,
                    title: "Over budget".to_owned(),
                },
            },
        )
        .err()
        .ok_or("active publication bytes were uncharged")?;
    assert_eq!(error.code, "portable.preview_too_large");
    drop(lease);
    let replacement_id = request_id()?;
    reserve(&custody, "other", ReviewKind::Backup)?
        .publish_prepared(replacement_id, output)
        .map_err(boxed)?;
    let replacement = custody
        .acquire("other", replacement_id, binding)
        .map_err(boxed)?;
    drop(replacement);
    Ok(())
}

#[tokio::test]
async fn creator_cleanup_rejects_late_core_publication_without_releasing_active_capacity()
-> TestResult {
    let (_directory, custody, _scenario) = fixture().await?;
    let bytes = backup(&custody).await?;
    let old = creator()?;
    let pending = custody
        .reserve("main", old, ReviewKind::Restore, CancellationToken::new())
        .map_err(boxed)?;
    let sibling = Creator {
        operation_id: creator()?.operation_id,
        request_id: old.request_id,
    };
    let surviving = custody
        .reserve(
            "main",
            sibling,
            ReviewKind::Restore,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let third = reserve(&custody, "other", ReviewKind::Import)?;
    custody.discard("main", &creator_target(old));
    assert_eq!(
        custody
            .reserve(
                "other",
                creator()?,
                ReviewKind::Restore,
                CancellationToken::new()
            )
            .err()
            .ok_or("creator cancellation released active creation")?
            .code,
        "portable.preview_capacity"
    );
    let (discarded_id, revision) = core_restore(&custody, bytes.clone()).await?;
    assert_eq!(
        pending
            .publish_core(
                discarded_id,
                ReviewBinding::Restore {
                    library_revision: revision
                }
            )
            .err()
            .ok_or("late creator result published")?
            .code,
        "operation.cancelled"
    );
    let replacement = wait_for_restore_reservation(&custody, "main", old).await?;
    assert!(matches!(
        apply_restore(&custody, discarded_id, SafetyBackupEvidence::NotRequired).await?,
        Err(AppError::Protocol(error)) if error.code == "portable.preview_not_found"
    ));
    let (surviving_id, revision) = core_restore(&custody, bytes).await?;
    surviving
        .publish_core(
            surviving_id,
            ReviewBinding::Restore {
                library_revision: revision,
            },
        )
        .map_err(boxed)?;
    custody.discard("main", &creator_target(old));
    let lease = custody
        .acquire(
            "main",
            surviving_id,
            ReviewBinding::Restore {
                library_revision: revision,
            },
        )
        .map_err(boxed)?;
    assert!(matches!(
        apply_restore(&custody, surviving_id, SafetyBackupEvidence::NotRequired)
            .await?
            .map_err(boxed)?,
        AppCommandResult::PortableApplied {
            safety_backup: SafetyBackupOutcome::CreatedAndVerified { .. },
            ..
        }
    ));
    drop((lease, replacement, third));
    Ok(())
}

#[tokio::test]
async fn restore_handback_requires_real_failure_and_preserves_the_rightful_retry() -> TestResult {
    let (directory, custody, scenario_id) = fixture().await?;
    let bytes = backup(&custody).await?;
    disable_safety_backups(&directory)?;
    let (unattempted_id, revision) = bound_restore(&custody, "main", bytes.clone()).await?;
    let binding = ReviewBinding::Restore {
        library_revision: revision,
    };
    let unattempted = custody
        .acquire("main", unattempted_id, binding)
        .map_err(boxed)?;
    assert!(
        !unattempted
            .retain_restore_retry(&CancellationToken::new())
            .await
    );
    assert!(custody.acquire("main", unattempted_id, binding).is_err());

    let (preview_id, revision) = bound_restore(&custody, "main", bytes).await?;
    custody.discard("other", &PreviewTarget::Preview { preview_id });
    assert!(custody.acquire("other", preview_id, binding).is_err());
    assert!(
        custody
            .acquire(
                "main",
                preview_id,
                ReviewBinding::Import {
                    library_revision: revision
                }
            )
            .is_err()
    );
    assert!(
        custody
            .acquire(
                "main",
                preview_id,
                ReviewBinding::Restore {
                    library_revision: Revision::new(revision.value() + 1),
                }
            )
            .is_err()
    );
    let lease = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    fail_safety_backup(&custody, preview_id, revision).await?;
    assert!(lease.retain_restore_retry(&CancellationToken::new()).await);
    assert!(custody.acquire("other", preview_id, binding).is_err());
    let retry = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    let AppCommandResult::PortableApplied {
        scenarios,
        library_revision,
        safety_backup,
    } = apply_restore(
        &custody,
        preview_id,
        SafetyBackupEvidence::FailedWithStrongConfirmation {
            proof: "REPLACE WITHOUT BACKUP".to_owned(),
        },
    )
    .await?
    .map_err(boxed)?
    else {
        return Err("wrong restore apply receipt".into());
    };
    assert!(matches!(
        safety_backup,
        SafetyBackupOutcome::ConfirmedBypass
    ));
    assert_eq!(
        scenarios
            .iter()
            .map(|scenario| (scenario.source_scenario_id, scenario.scenario_id))
            .collect::<Vec<_>>(),
        vec![(scenario_id, scenario_id)]
    );
    assert!(library_revision > revision);
    assert_eq!(
        custody
            .app
            .application_settings_snapshot()
            .await
            .map_err(boxed)?
            .library_revision,
        library_revision
    );
    assert!(
        !custody
            .app
            .portable_restore_retry_is_retained(preview_id, revision)
            .await
    );
    drop(retry);
    assert!(custody.acquire("main", preview_id, binding).is_err());
    Ok(())
}

#[tokio::test]
async fn cancellation_after_a_real_backup_failure_refuses_retry_and_retires_core_authority()
-> TestResult {
    let (directory, custody, _scenario) = fixture().await?;
    let bytes = backup(&custody).await?;
    disable_safety_backups(&directory)?;
    let (preview_id, revision) = bound_restore(&custody, "main", bytes).await?;
    let binding = ReviewBinding::Restore {
        library_revision: revision,
    };
    let lease = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    fail_safety_backup(&custody, preview_id, revision).await?;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(!lease.retain_restore_retry(&cancellation).await);
    assert!(custody.acquire("main", preview_id, binding).is_err());
    wait_for_retry_removal(&custody, preview_id, revision).await?;
    assert!(matches!(
        apply_restore(&custody, preview_id, SafetyBackupEvidence::NotRequired).await?,
        Err(AppError::Protocol(error)) if error.code == "portable.preview_not_found"
    ));
    Ok(())
}

#[tokio::test]
async fn creator_discard_during_apply_prevents_real_failure_handback() -> TestResult {
    let (directory, custody, _scenario) = fixture().await?;
    let bytes = backup(&custody).await?;
    disable_safety_backups(&directory)?;
    let creator = creator()?;
    let reservation = custody
        .reserve(
            "main",
            creator,
            ReviewKind::Restore,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let (preview_id, revision) = core_restore(&custody, bytes).await?;
    let binding = ReviewBinding::Restore {
        library_revision: revision,
    };
    reservation
        .publish_core(preview_id, binding)
        .map_err(boxed)?;
    let lease = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    fail_safety_backup(&custody, preview_id, revision).await?;
    custody.discard("main", &creator_target(creator));
    assert!(custody.acquire("main", preview_id, binding).is_err());
    assert!(
        custody
            .app
            .portable_restore_retry_is_retained(preview_id, revision)
            .await
    );
    assert!(
        custody
            .reserve(
                "main",
                creator,
                ReviewKind::Restore,
                CancellationToken::new()
            )
            .is_err()
    );
    assert!(!lease.retain_restore_retry(&CancellationToken::new()).await);
    let replacement = wait_for_restore_reservation(&custody, "main", creator).await?;
    assert!(
        !custody
            .app
            .portable_restore_retry_is_retained(preview_id, revision)
            .await
    );
    assert!(matches!(
        apply_restore(&custody, preview_id, SafetyBackupEvidence::NotRequired).await?,
        Err(AppError::Protocol(error)) if error.code == "portable.preview_not_found"
    ));
    drop(replacement);
    Ok(())
}

#[tokio::test]
async fn window_teardown_keeps_active_creation_and_apply_charged_until_settlement() -> TestResult {
    let (directory, custody, _scenario) = fixture().await?;
    let bytes = backup(&custody).await?;
    disable_safety_backups(&directory)?;
    let (preview_id, revision) = bound_restore(&custody, "main", bytes.clone()).await?;
    let binding = ReviewBinding::Restore {
        library_revision: revision,
    };
    let applying = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    fail_safety_backup(&custody, preview_id, revision).await?;
    let creating = reserve(&custody, "main", ReviewKind::Restore)?;
    let (other_id, other_revision) = bound_restore(&custody, "other", bytes.clone()).await?;
    custody.close_window("main");
    assert!(reserve(&custody, "main", ReviewKind::Restore).is_err());
    assert!(custody.acquire("main", preview_id, binding).is_err());
    assert_eq!(
        custody
            .reserve(
                "other",
                creator()?,
                ReviewKind::Restore,
                CancellationToken::new()
            )
            .err()
            .ok_or("window close released active charges")?
            .code,
        "portable.preview_capacity"
    );
    // Closing cannot remove core authority underneath an active native apply lease.
    assert!(
        custody
            .app
            .portable_restore_retry_is_retained(preview_id, revision)
            .await
    );
    let (late_id, late_revision) = core_restore(&custody, bytes).await?;
    assert_eq!(
        creating
            .publish_core(
                late_id,
                ReviewBinding::Restore {
                    library_revision: late_revision
                }
            )
            .err()
            .ok_or("closed-window creation published")?
            .code,
        "operation.cancelled"
    );
    assert!(
        !applying
            .retain_restore_retry(&CancellationToken::new())
            .await
    );
    wait_for_retry_removal(&custody, preview_id, revision).await?;
    let replacement = wait_for_restore_reservation(&custody, "other", creator()?).await?;
    let other = custody
        .acquire(
            "other",
            other_id,
            ReviewBinding::Restore {
                library_revision: other_revision,
            },
        )
        .map_err(boxed)?;
    fail_safety_backup(&custody, other_id, other_revision).await?;
    assert!(other.retain_restore_retry(&CancellationToken::new()).await);
    custody.discard(
        "other",
        &PreviewTarget::Preview {
            preview_id: other_id,
        },
    );
    wait_for_retry_removal(&custody, other_id, other_revision).await?;
    drop(replacement);
    Ok(())
}

#[tokio::test]
async fn real_core_eviction_prevents_native_restore_retry_handback() -> TestResult {
    let (directory, custody, _scenario) = fixture().await?;
    let bytes = backup(&custody).await?;
    disable_safety_backups(&directory)?;
    let (preview_id, revision) = bound_restore(&custody, "main", bytes.clone()).await?;
    let binding = ReviewBinding::Restore {
        library_revision: revision,
    };
    let lease = custody
        .acquire("main", preview_id, binding)
        .map_err(boxed)?;
    fail_safety_backup(&custody, preview_id, revision).await?;
    let mut replacements = Vec::new();
    // The core cache is shared with producers outside this native custody group.
    for _ in 0..3 {
        replacements.push(core_restore(&custody, bytes.clone()).await?.0);
    }
    assert!(
        !custody
            .app
            .portable_restore_retry_is_retained(preview_id, revision)
            .await
    );
    assert!(!lease.retain_restore_retry(&CancellationToken::new()).await);
    assert!(custody.acquire("main", preview_id, binding).is_err());
    assert!(matches!(
        apply_restore(&custody, preview_id, SafetyBackupEvidence::NotRequired).await?,
        Err(AppError::Protocol(error)) if error.code == "portable.preview_not_found"
    ));
    for replacement in replacements {
        assert!(matches!(
            custody
                .app
                .execute(AppCommand::CancelPortablePreview {
                    preview_id: replacement
                })
                .await
                .map_err(boxed)?,
            AppCommandResult::PortablePreviewCancelled
        ));
    }
    Ok(())
}
