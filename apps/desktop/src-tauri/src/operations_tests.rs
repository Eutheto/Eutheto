use super::*;
use eutheto_core::{AppDependencies, AppPaths};
use eutheto_types::{FixedClock, FixedMonotonicClock, SystemIdGenerator};
use std::error::Error;
use tokio::sync::oneshot;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

async fn fixture() -> TestResult<(
    tempfile::TempDir,
    Arc<OperationRegistry>,
    Arc<FixedMonotonicClock>,
)> {
    let directory = tempfile::tempdir()?;
    let clock = Arc::new(FixedClock::new("2026-09-10T12:00:00Z".parse()?));
    let monotonic = Arc::new(FixedMonotonicClock::default());
    let ids = Arc::new(SystemIdGenerator);
    let app = EuthetoApp::open(AppDependencies {
        paths: AppPaths {
            database: directory.path().join("operations.sqlite3"),
            safety_backups: directory.path().join("backups"),
        },
        clock: clock.clone(),
        monotonic_clock: monotonic.clone(),
        ids: ids.clone(),
        cancellation: CancellationToken::new(),
    })
    .await
    .map_err(boxed)?;
    let registry = Arc::new(OperationRegistry::new(
        app,
        clock,
        monotonic.clone(),
        ids,
        ["main".to_owned(), "other".to_owned()],
    ));
    Ok((directory, registry, monotonic))
}
fn request() -> TestResult<OperationPrepareRequestV1> {
    Ok(OperationPrepareRequestV1 {
        request_id: RequestId::new(&SystemIdGenerator)?,
        schema_version: 1,
        purpose: OperationPurposeV1::ScenarioSummary,
        context: OperationContextV1::Scenario {
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            expected_revision: None,
        },
    })
}
fn claim(prepared: &OperationPreparedV1, request: &OperationPrepareRequestV1) -> OperationClaim {
    OperationClaim {
        operation_id: prepared.operation_id,
        request_id: request.request_id,
        purpose: Some(request.purpose.clone()),
        context: request.context.clone(),
    }
}
fn control(
    prepared: &OperationPreparedV1,
    request: &OperationPrepareRequestV1,
) -> OperationControlRequestV1 {
    OperationControlRequestV1 {
        request_id: request.request_id,
        schema_version: 1,
        operation_id: prepared.operation_id,
    }
}
async fn run(
    registry: &Arc<OperationRegistry>,
    window: &str,
    prepared: &OperationPreparedV1,
    request: &OperationPrepareRequestV1,
) -> Result<(), ApiError> {
    registry
        .run(
            window,
            claim(prepared, request),
            None,
            OperationPhaseV1::CapturingSnapshot,
            ((), None),
            |(), _| async { Ok(()) },
        )
        .await
}

type Held = (
    tokio::task::JoinHandle<Result<(), ApiError>>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
);
fn hold(
    registry: Arc<OperationRegistry>,
    window: &'static str,
    prepared: &OperationPreparedV1,
    request: &OperationPrepareRequestV1,
) -> Held {
    let operation = claim(prepared, request);
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let running = tokio::spawn(async move {
        registry
            .run(
                window,
                operation,
                None,
                OperationPhaseV1::CapturingSnapshot,
                ((), None),
                |(), _| async move {
                    let _ = entered.send(());
                    // Models an already-started blocking/store operation: cancellation cannot abort it.
                    let _ = released.await;
                    Ok(())
                },
            )
            .await
    });
    (running, started, release)
}

#[tokio::test]
async fn reservations_are_bounded_and_expire_without_reusing_claim_authority() -> TestResult {
    let (_directory, registry, clock) = fixture().await?;
    let request = request()?;
    let mut reservations = Vec::new();
    for _ in 0..16 {
        reservations.push(registry.prepare("main", &request).map_err(boxed)?);
    }
    assert_eq!(
        registry
            .prepare("main", &request)
            .err()
            .ok_or("capacity must be bounded")?
            .code,
        "operation.resource_limit"
    );
    clock.advance(Duration::from_secs(29))?;
    assert_eq!(
        registry
            .prepare("main", &request)
            .err()
            .ok_or("not expired")?
            .code,
        "operation.resource_limit"
    );
    clock.advance(Duration::from_secs(1))?;
    let fresh = registry.prepare("main", &request).map_err(boxed)?;
    assert_eq!(
        run(&registry, "main", &reservations[0], &request)
            .await
            .err()
            .ok_or("expired claim")?
            .code,
        "operation.not_active"
    );
    run(&registry, "main", &fresh, &request)
        .await
        .map_err(boxed)?;
    assert_eq!(
        run(&registry, "main", &fresh, &request)
            .await
            .err()
            .ok_or("retired claim")?
            .code,
        "operation.not_active"
    );
    Ok(())
}

#[tokio::test]
async fn claims_require_exact_owner_purpose_context_and_one_consumption() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let request = request()?;
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    assert_eq!(
        run(&registry, "other", &prepared, &request)
            .await
            .err()
            .ok_or("wrong owner")?
            .code,
        "operation.not_active"
    );
    assert_eq!(
        registry
            .cancel("other", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    assert_eq!(
        registry
            .release("other", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        ReleaseAcknowledgementV1::NotActive
    );
    let mut mismatch = request.clone();
    mismatch.purpose = OperationPurposeV1::SetupStatus;
    assert_eq!(
        run(&registry, "main", &prepared, &mismatch)
            .await
            .err()
            .ok_or("wrong purpose")?
            .code,
        "operation.claim_mismatch"
    );
    mismatch = request.clone();
    mismatch.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: None,
    };
    assert_eq!(
        run(&registry, "main", &prepared, &mismatch)
            .await
            .err()
            .ok_or("wrong context")?
            .code,
        "operation.claim_mismatch"
    );
    let (running, started, release) = hold(registry.clone(), "main", &prepared, &request);
    tokio::time::timeout(Duration::from_secs(10), started).await??;
    assert_eq!(
        run(&registry, "main", &prepared, &request)
            .await
            .err()
            .ok_or("duplicate claim")?
            .code,
        "operation.already_claimed"
    );
    assert_eq!(
        registry
            .release("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        ReleaseAcknowledgementV1::CancellationRequested
    );
    release.send(()).map_err(boxed)?;
    // Late cancellation cannot erase the successful terminal outcome of already-started work.
    running.await?.map_err(boxed)?;
    assert_eq!(
        registry
            .cancel("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    Ok(())
}

#[tokio::test]
async fn cancelled_or_released_reservations_never_start_work() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let request = request()?;
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    registry
        .cancel("main", &control(&prepared, &request))
        .map_err(boxed)?;
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = entered.clone();
    let result = registry
        .run(
            "main",
            claim(&prepared, &request),
            None,
            OperationPhaseV1::CapturingSnapshot,
            ((), None),
            move |(), _| async move {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
    assert_eq!(
        result.err().ok_or("cancelled before claim")?.code,
        "operation.cancelled"
    );
    assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    assert_eq!(
        registry
            .release("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        ReleaseAcknowledgementV1::Released
    );
    assert_eq!(
        run(&registry, "main", &prepared, &request)
            .await
            .err()
            .ok_or("released claim")?
            .code,
        "operation.not_active"
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn export_cancel_only_reaches_export_purposes_in_the_owning_window() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    for purpose in [
        OperationPurposeV1::ExportPreview,
        OperationPurposeV1::ExportCreate,
        OperationPurposeV1::SharePreview,
        OperationPurposeV1::ShareCreate,
        OperationPurposeV1::ScenarioSummary,
        OperationPurposeV1::SolveStart,
        OperationPurposeV1::ManualRepairRead,
        OperationPurposeV1::CsvReportSave,
        OperationPurposeV1::ProjectExportCreate,
    ] {
        let is_export = matches!(
            purpose,
            OperationPurposeV1::ExportPreview | OperationPurposeV1::ExportCreate
        );
        let mut request = request()?;
        request.purpose = purpose;
        request.context = OperationContextV1::Scenario {
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            expected_revision: Some(Revision::new(1)),
        };
        let prepared = registry.prepare("main", &request).map_err(boxed)?;
        assert_eq!(
            registry
                .cancel_export("other", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            CancellationAcknowledgementV1::NotActive
        );
        assert_eq!(
            registry
                .cancel_export("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            if is_export {
                CancellationAcknowledgementV1::CancellationRequested
            } else {
                CancellationAcknowledgementV1::NotActive
            }
        );
        if is_export {
            let released = registry.prepare("main", &request).map_err(boxed)?;
            assert_eq!(
                registry
                    .release("main", &control(&released, &request))
                    .map_err(boxed)?
                    .acknowledgement,
                ReleaseAcknowledgementV1::Released
            );
            assert_eq!(
                run(&registry, "main", &released, &request)
                    .await
                    .err()
                    .ok_or("unclaimed released export cannot start")?
                    .code,
                "operation.not_active"
            );
            assert_eq!(
                run(&registry, "main", &prepared, &request)
                    .await
                    .err()
                    .ok_or("cancelled export cannot start")?
                    .code,
                "operation.cancelled"
            );
        } else {
            let active = registry
                .claim("main", &claim(&prepared, &request))
                .map_err(boxed)?;
            assert!(!active.signal.cancellation.is_cancelled());
            drop(active);
        }
        assert_eq!(
            registry
                .cancel_export("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            CancellationAcknowledgementV1::NotActive
        );
    }
    let request = request()?;
    let prepared = registry
        .reserve("main", None, request.context.clone())
        .map_err(boxed)?;
    assert_eq!(
        registry
            .cancel_export("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    let active = registry
        .claim(
            "main",
            &OperationClaim {
                purpose: None,
                ..claim(&prepared, &request)
            },
        )
        .map_err(boxed)?;
    assert!(!active.signal.cancellation.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn claimed_export_observer_release_preserves_cancellation_and_owned_success() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    for purpose in [
        OperationPurposeV1::ExportPreview,
        OperationPurposeV1::ExportCreate,
    ] {
        let mut request = request()?;
        request.purpose = purpose;
        request.context = OperationContextV1::Scenario {
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            expected_revision: Some(Revision::new(1)),
        };
        let prepared = registry.prepare("main", &request).map_err(boxed)?;
        let (entered, started) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let time_limit = DurationMillis::new(30_000)?;
        let running = tokio::spawn({
            let registry = registry.clone();
            let operation = claim(&prepared, &request);
            async move {
                registry
                    .run_with_mutation_budget(
                        "main",
                        operation,
                        None,
                        OperationPhaseV1::PublishingFile,
                        ((), None),
                        time_limit,
                        |(), execution| async move {
                            let _ = entered.send(execution.cancellation());
                            let _ = released.await;
                            // Unit-level settlement only; real bytes/receipts are covered by worker_tests.
                            Ok::<_, ApiError>("owned-success")
                        },
                    )
                    .await
            }
        });
        let token = tokio::time::timeout(Duration::from_secs(10), started).await??;
        assert_eq!(
            registry
                .release("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            ReleaseAcknowledgementV1::Released
        );
        assert!(
            !token.is_cancelled(),
            "observer release cancelled owned export"
        );
        assert_eq!(
            registry
                .claim("main", &claim(&prepared, &request))
                .err()
                .ok_or("observer release must not unclaim publication")?
                .code,
            "operation.already_claimed"
        );
        assert_eq!(
            registry
                .cancel_export("other", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            CancellationAcknowledgementV1::NotActive
        );
        assert!(!token.is_cancelled());
        assert_eq!(
            registry
                .cancel_export("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            CancellationAcknowledgementV1::CancellationRequested
        );
        assert!(
            token.is_cancelled(),
            "explicit cancel became unreachable after release"
        );
        release.send(()).map_err(boxed)?;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), running)
                .await??
                .map_err(boxed)?,
            "owned-success",
            "late cancellation downgraded an owned successful outcome"
        );
        assert_eq!(
            registry
                .release("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            ReleaseAcknowledgementV1::NotActive
        );
        assert_eq!(
            registry
                .cancel_export("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement,
            CancellationAcknowledgementV1::NotActive
        );
    }
    Ok(())
}

#[tokio::test]
async fn releasing_a_claimed_solve_only_detaches_its_observer() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let mut request = request()?;
    request.purpose = OperationPurposeV1::SolveStart;
    request.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Some(Revision::new(1)),
    };
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let time_limit = DurationMillis::new(30_000)?;
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let running = tokio::spawn({
        let registry = registry.clone();
        let operation = claim(&prepared, &request);
        async move {
            registry
                .run_with_solve_budget(
                    "main",
                    operation,
                    None,
                    OperationPhaseV1::Validating,
                    ((), None),
                    time_limit,
                    |(), execution| async move {
                        let token = execution.cancellation();
                        let _ = entered.send(token.clone());
                        let _ = released.await;
                        Ok(token.is_cancelled())
                    },
                )
                .await
        }
    });
    let token = tokio::time::timeout(Duration::from_secs(10), started).await??;
    assert_eq!(
        registry
            .release("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        ReleaseAcknowledgementV1::Released
    );
    assert!(
        !token.is_cancelled(),
        "observer release cancelled durable work"
    );
    assert_eq!(
        registry
            .cancel("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    let wrong_revision = OperationContextV1::Scenario {
        scenario_id: match &request.context {
            OperationContextV1::Scenario { scenario_id, .. } => *scenario_id,
            OperationContextV1::Library { .. } => return Err("expected scenario context".into()),
        },
        expected_revision: Some(Revision::new(2)),
    };
    assert_eq!(
        registry
            .cancel_solve("main", &control(&prepared, &request), wrong_revision)
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    assert!(
        !token.is_cancelled(),
        "wrong revision cancelled durable work"
    );
    assert_eq!(
        registry
            .cancel_solve(
                "main",
                &control(&prepared, &request),
                request.context.clone()
            )
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::CancellationRequested
    );
    assert!(
        token.is_cancelled(),
        "explicit solve_cancel did not reach work"
    );
    release.send(()).map_err(boxed)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(10), running)
            .await??
            .map_err(boxed)?
    );
    Ok(())
}

#[tokio::test]
async fn live_solve_reattachment_requires_exact_claimed_window_request_and_revision() -> TestResult
{
    let (_directory, registry, _) = fixture().await?;
    let mut request = request()?;
    request.purpose = OperationPurposeV1::SolveStart;
    request.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Some(Revision::new(1)),
    };
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let owned = || {
        registry
            .active_solve_operation("main", request.request_id, &request.context)
            .map_err(boxed)
    };
    assert_eq!(owned()?, None, "an unclaimed solve cannot be reattached");
    let claimed = registry
        .claim("main", &claim(&prepared, &request))
        .map_err(boxed)?;
    assert_eq!(owned()?, Some(prepared.operation_id));
    assert_eq!(
        registry
            .active_solve_operation("other", request.request_id, &request.context)
            .map_err(boxed)?,
        None
    );
    assert_eq!(
        registry
            .active_solve_operation(
                "main",
                RequestId::new(&SystemIdGenerator)?,
                &request.context
            )
            .map_err(boxed)?,
        None
    );
    let wrong_revision = OperationContextV1::Scenario {
        scenario_id: match &request.context {
            OperationContextV1::Scenario { scenario_id, .. } => *scenario_id,
            OperationContextV1::Library { .. } => return Err("expected scenario context".into()),
        },
        expected_revision: Some(Revision::new(2)),
    };
    assert_eq!(
        registry
            .active_solve_operation("main", request.request_id, &wrong_revision)
            .map_err(boxed)?,
        None
    );
    assert_eq!(
        registry
            .release("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        ReleaseAcknowledgementV1::Released
    );
    assert_eq!(owned()?, Some(prepared.operation_id));
    claimed.signal.cancel();
    assert_eq!(
        owned()?,
        None,
        "cancelled work cannot reissue an active handle"
    );
    drop(claimed);
    assert_eq!(owned()?, None, "a settled reservation has no live handle");
    Ok(())
}

#[tokio::test]
async fn caller_abandonment_retains_admission_until_work_finishes_and_waiters_cancel_promptly()
-> TestResult {
    let (_directory, registry, clock) = fixture().await?;
    let request = request()?;
    let first = registry.prepare("main", &request).map_err(boxed)?;
    let second = registry.prepare("main", &request).map_err(boxed)?;
    let (first_caller, first_started, release_first) =
        hold(registry.clone(), "main", &first, &request);
    let (second_caller, second_started, release_second) =
        hold(registry.clone(), "main", &second, &request);
    tokio::time::timeout(Duration::from_secs(10), first_started).await??;
    tokio::time::timeout(Duration::from_secs(10), second_started).await??;
    first_caller.abort();
    assert!(
        first_caller
            .await
            .err()
            .ok_or("abandoned caller")?
            .is_cancelled()
    );
    clock.advance(Duration::from_secs(31))?;
    assert_eq!(
        registry
            .cancel("main", &control(&first, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::CancellationRequested
    );
    let queued = registry.prepare("main", &request).map_err(boxed)?;
    let waiting = Arc::new(Notify::new());
    let notified = waiting.notified();
    let progress: ProgressSink = Arc::new({
        let waiting = waiting.clone();
        move |event| {
            if event.phase == OperationPhaseV1::WaitingForAdmission {
                waiting.notify_one();
            }
        }
    });
    let queued_caller = tokio::spawn({
        let registry = registry.clone();
        let operation = claim(&queued, &request);
        async move {
            registry
                .run(
                    "main",
                    operation,
                    Some(progress),
                    OperationPhaseV1::CapturingSnapshot,
                    ((), None),
                    |(), _| async {
                        Err::<(), _>(
                            boundary_error(
                                "fixture.work_started",
                                "Cancelled waiter started work.",
                                None,
                            )
                            .into(),
                        )
                    },
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), notified).await?;
    registry
        .cancel("main", &control(&queued, &request))
        .map_err(boxed)?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), queued_caller)
            .await??
            .err()
            .ok_or("queued cancellation")?
            .code,
        "operation.cancelled"
    );
    let next = registry.prepare("main", &request).map_err(boxed)?;
    let (next_caller, mut next_started, release_next) =
        hold(registry.clone(), "main", &next, &request);
    // Both actual workers still own their permits, even though one invoke caller disappeared.
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut next_started)
            .await
            .is_err()
    );
    release_first.send(()).map_err(boxed)?;
    tokio::time::timeout(Duration::from_secs(10), next_started).await??;
    release_next.send(()).map_err(boxed)?;
    release_second.send(()).map_err(boxed)?;
    next_caller.await?.map_err(boxed)?;
    second_caller.await?.map_err(boxed)?;
    Ok(())
}

#[tokio::test]
async fn window_destruction_and_shutdown_close_only_their_owned_gates() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let request = request()?;
    let main = registry.prepare("main", &request).map_err(boxed)?;
    let other = registry.prepare("other", &request).map_err(boxed)?;
    registry.cancel_window("main");
    assert_eq!(
        run(&registry, "main", &main, &request)
            .await
            .err()
            .ok_or("destroyed window")?
            .code,
        "operation.cancelled"
    );
    assert_eq!(
        registry
            .prepare("main", &request)
            .err()
            .ok_or("closed owner gate")?
            .code,
        "operation.cancelled"
    );
    run(&registry, "other", &other, &request)
        .await
        .map_err(boxed)?;
    let other = registry.prepare("other", &request).map_err(boxed)?;
    registry.shutdown();
    assert_eq!(
        run(&registry, "other", &other, &request)
            .await
            .err()
            .ok_or("shutdown")?
            .code,
        "operation.cancelled"
    );
    assert_eq!(
        registry
            .prepare("other", &request)
            .err()
            .ok_or("closed root gate")?
            .code,
        "operation.cancelled"
    );
    Ok(())
}

#[tokio::test]
async fn abandoned_preflight_stays_charged_until_its_blocking_work_finishes() -> TestResult {
    struct Input {
        entered: Arc<Notify>,
        released: std::sync::mpsc::Receiver<()>,
    }
    fn preflight(_: &EuthetoApp, input: &Input, token: &CancellationToken) -> Result<(), ApiError> {
        input.entered.notify_one();
        let _ = input.released.recv();
        if token.is_cancelled() {
            Err(cancelled().into())
        } else {
            Ok(())
        }
    }
    let (_directory, registry, clock) = fixture().await?;
    let request = request()?;
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let entered = Arc::new(Notify::new());
    let (release, released) = std::sync::mpsc::channel();
    let running = tokio::spawn({
        let registry = registry.clone();
        let operation = claim(&prepared, &request);
        let entered = entered.clone();
        async move {
            registry
                .run(
                    "main",
                    operation,
                    None,
                    OperationPhaseV1::CapturingSnapshot,
                    (Input { entered, released }, Some(preflight)),
                    |_, _| async {
                        Err::<(), _>(
                            boundary_error(
                                "fixture.work_started",
                                "Cancelled preflight started work.",
                                None,
                            )
                            .into(),
                        )
                    },
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
    running.abort();
    assert!(running.await.err().ok_or("caller aborted")?.is_cancelled());
    clock.advance(Duration::from_secs(31))?;
    for _ in 0..15 {
        registry.prepare("main", &request).map_err(boxed)?;
    }
    assert_eq!(
        registry
            .prepare("main", &request)
            .err()
            .ok_or("active preflight lost its charge")?
            .code,
        "operation.resource_limit"
    );
    release.send(())?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if registry
                .cancel("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement
                == CancellationAcknowledgementV1::NotActive
            {
                return Ok::<(), Box<dyn Error>>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let next = registry.prepare("main", &request).map_err(boxed)?;
    run(&registry, "main", &next, &request)
        .await
        .map_err(boxed)?;
    Ok(())
}

#[tokio::test]
async fn settings_operations_require_their_exact_library_revision_context() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    for (purpose, expected_library_revision) in [
        (OperationPurposeV1::SettingsImportPreview, None),
        (OperationPurposeV1::SettingsExport, None),
        (
            OperationPurposeV1::SettingsImportApply,
            Some(Revision::new(7)),
        ),
    ] {
        let mut request = request()?;
        request.purpose = purpose;
        let scenario_context = request.context.clone();
        assert_eq!(
            registry
                .prepare("main", &request)
                .err()
                .ok_or("settings accepted scenario authority")?
                .code,
            "operation.context_mismatch"
        );
        request.context = OperationContextV1::Library {
            expected_library_revision: if expected_library_revision.is_some() {
                None
            } else {
                Some(Revision::new(7))
            },
        };
        assert_eq!(
            registry
                .prepare("main", &request)
                .err()
                .ok_or("incorrect settings revision admission")?
                .code,
            "operation.library_revision_mismatch"
        );
        request.context = OperationContextV1::Library {
            expected_library_revision,
        };
        let prepared = registry.prepare("main", &request).map_err(boxed)?;
        let mut mismatch = request.clone();
        mismatch.context = scenario_context;
        assert_eq!(
            run(&registry, "main", &prepared, &mismatch)
                .await
                .err()
                .ok_or("cross-kind claim")?
                .code,
            "operation.claim_mismatch"
        );
        mismatch.context = OperationContextV1::Library {
            expected_library_revision: Some(Revision::new(8)),
        };
        assert_eq!(
            run(&registry, "main", &prepared, &mismatch)
                .await
                .err()
                .ok_or("rebound library revision")?
                .code,
            "operation.claim_mismatch"
        );
        // Invalid claims preserve the original reservation; only its exact claim consumes it.
        run(&registry, "main", &prepared, &request)
            .await
            .map_err(boxed)?;
        assert_eq!(
            run(&registry, "main", &prepared, &request)
                .await
                .err()
                .ok_or("replayed library operation")?
                .code,
            "operation.not_active"
        );
    }
    for purpose in [
        OperationPurposeV1::ScenarioSummary,
        OperationPurposeV1::CsvApply,
    ] {
        let mut request = request()?;
        request.purpose = purpose;
        request.context = OperationContextV1::Library {
            expected_library_revision: Some(Revision::new(7)),
        };
        assert_eq!(
            registry
                .prepare("main", &request)
                .err()
                .ok_or("scenario purpose accepted library authority")?
                .code,
            "operation.context_mismatch"
        );
    }
    Ok(())
}
#[tokio::test]
async fn queued_solve_expires_before_heavy_admission() -> TestResult {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let (_directory, registry, _) = fixture().await?;
    let first_request = request()?;
    let first_prepared = registry.prepare("main", &first_request).map_err(boxed)?;
    let second_request = request()?;
    let second_prepared = registry.prepare("main", &second_request).map_err(boxed)?;
    let (first, first_started, first_release) =
        hold(registry.clone(), "main", &first_prepared, &first_request);
    let (second, second_started, second_release) =
        hold(registry.clone(), "main", &second_prepared, &second_request);
    tokio::time::timeout(Duration::from_secs(1), first_started).await??;
    tokio::time::timeout(Duration::from_secs(1), second_started).await??;

    let queued_request = request()?;
    let queued_prepared = registry.prepare("main", &queued_request).map_err(boxed)?;
    let entered = Arc::new(AtomicBool::new(false));
    let entered_by_work = Arc::clone(&entered);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        registry.run_with_solve_budget(
            "main",
            claim(&queued_prepared, &queued_request),
            None,
            OperationPhaseV1::Validating,
            ((), None),
            DurationMillis::new(20)?,
            move |(), _| async move {
                entered_by_work.store(true, Ordering::SeqCst);
                Ok(())
            },
        ),
    )
    .await?;
    assert_eq!(
        result
            .err()
            .ok_or("queued solve unexpectedly entered work")?
            .code,
        "operation.deadline_exceeded"
    );
    assert!(!entered.load(Ordering::SeqCst));

    first_release.send(()).map_err(boxed)?;
    second_release.send(()).map_err(boxed)?;
    first.await?.map_err(boxed)?;
    second.await?.map_err(boxed)?;
    Ok(())
}
#[tokio::test]
async fn solve_estimate_is_an_ordinary_cancellable_bounded_operation() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let mut request = request()?;
    request.purpose = OperationPurposeV1::SolveEstimate;
    request.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Some(Revision::INITIAL),
    };
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let (started, started_receiver) = oneshot::channel();
    let time_limit = DurationMillis::new(30_000)?;
    let running = {
        let registry = Arc::clone(&registry);
        let claim = claim(&prepared, &request);
        tokio::spawn(async move {
            registry
                .run_with_bounded_budget(
                    "main",
                    claim,
                    None,
                    OperationPhaseV1::Validating,
                    ((), None),
                    time_limit,
                    move |(), execution| async move {
                        let _ = started.send(());
                        let cancellation = execution.cancellation();
                        while !cancellation.is_cancelled() {
                            tokio::task::yield_now().await;
                        }
                        Err::<(), ApiError>(cancelled().into())
                    },
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(1), started_receiver).await??;
    let acknowledgement = registry
        .cancel("main", &control(&prepared, &request))
        .map_err(boxed)?;
    assert_eq!(
        acknowledgement.acknowledgement,
        CancellationAcknowledgementV1::CancellationRequested
    );
    let error = running
        .await?
        .err()
        .ok_or("estimate cancellation unexpectedly succeeded")?;
    assert_eq!(error.code, "operation.cancelled");
    Ok(())
}
#[tokio::test]
async fn bounded_estimate_expires_during_admitted_work_and_retains_ownership_until_release()
-> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let mut request = request()?;
    request.purpose = OperationPurposeV1::SolveEstimate;
    request.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Some(Revision::INITIAL),
    };
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let (started, started_receiver) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let time_limit = DurationMillis::new(20)?;
    let running = {
        let registry = Arc::clone(&registry);
        let operation = claim(&prepared, &request);
        tokio::spawn(async move {
            registry
                .run_with_bounded_budget(
                    "main",
                    operation,
                    None,
                    OperationPhaseV1::Validating,
                    ((), None),
                    time_limit,
                    move |(), execution| async move {
                        let _ = started.send(execution.cancellation());
                        let _ = released.await;
                        Ok::<(), ApiError>(())
                    },
                )
                .await
        })
    };
    let cancellation = tokio::time::timeout(Duration::from_secs(1), started_receiver).await??;
    let result = tokio::time::timeout(Duration::from_secs(2), running).await??;
    assert_eq!(
        result
            .err()
            .ok_or("expired estimate returned success")?
            .code,
        "operation.deadline_exceeded"
    );
    assert!(cancellation.is_cancelled());
    assert_eq!(
        registry
            .cancel("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::CancellationRequested
    );
    release.send(()).map_err(boxed)?;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if registry
                .cancel("main", &control(&prepared, &request))
                .map_err(boxed)?
                .acknowledgement
                == CancellationAcknowledgementV1::NotActive
            {
                return Ok::<(), Box<dyn Error>>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    assert_eq!(registry.heavy.available_permits(), HEAVY_PERMITS);
    Ok(())
}

#[tokio::test]
async fn mutation_deadline_preserves_settlement_and_owned_capacity() -> TestResult {
    let (_directory, registry, clock) = fixture().await?;
    let mut request = request()?;
    request.purpose = OperationPurposeV1::ApplyReviewedGeneration;
    request.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Some(Revision::INITIAL),
    };
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let (started, started_receiver) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let time_limit = DurationMillis::new(1000)?;
    let running = {
        let registry = Arc::clone(&registry);
        let operation = claim(&prepared, &request);
        tokio::spawn(async move {
            registry
                .run_with_mutation_budget(
                    "main",
                    operation,
                    None,
                    OperationPhaseV1::ApplyingPreview,
                    ((), None),
                    time_limit,
                    move |(), mut execution| async move {
                        let (budget, _) = execution.take_solve_admission().ok_or_else(|| {
                            boundary_error(
                                "test.missing_budget",
                                "Mutation admission omitted its budget.",
                                None,
                            )
                        })?;
                        assert!(
                            eutheto_types::OperationControl::Solve(budget.phase_view())
                                .check()
                                .is_ok()
                        );
                        let _ = started.send(());
                        let _ = released.await;
                        assert!(budget.is_expired());
                        // The transaction's final latch has passed. Expiry cannot erase its receipt.
                        Ok::<Revision, ApiError>(Revision::new(1))
                    },
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), started_receiver).await??;
    clock.advance(Duration::from_secs(2))?;
    assert!(!running.is_finished());
    assert_eq!(registry.heavy.available_permits(), HEAVY_PERMITS - 1);
    assert!(
        registry
            .entries()
            .map_err(boxed)?
            .contains_key(&prepared.operation_id)
    );
    release.send(()).map_err(boxed)?;
    let revision = tokio::time::timeout(Duration::from_secs(2), running)
        .await??
        .map_err(boxed)?;
    assert_eq!(revision, Revision::new(1));
    assert_eq!(registry.heavy.available_permits(), HEAVY_PERMITS);
    assert!(
        !registry
            .entries()
            .map_err(boxed)?
            .contains_key(&prepared.operation_id)
    );
    Ok(())
}
// Keep simultaneous timed-out callers and retained native owners in one capacity scenario.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn bounded_timeout_keeps_each_reservation_and_heavy_permit_until_worker_release() -> TestResult
{
    let (_directory, registry, _) = fixture().await?;
    let deadline_limit = DurationMillis::new(20)?;
    let mut retained = Vec::new();
    let mut callers = Vec::new();
    for _ in 0..HEAVY_PERMITS {
        let mut request = request()?;
        request.purpose = OperationPurposeV1::SolveEstimate;
        request.context = OperationContextV1::Scenario {
            scenario_id: ScenarioId::new(&SystemIdGenerator)?,
            expected_revision: Some(Revision::INITIAL),
        };
        let prepared = registry.prepare("main", &request).map_err(boxed)?;
        let (started, started_receiver) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let caller = {
            let registry = Arc::clone(&registry);
            let operation = claim(&prepared, &request);
            tokio::spawn(async move {
                registry
                    .run_with_bounded_budget(
                        "main",
                        operation,
                        None,
                        OperationPhaseV1::Validating,
                        ((), None),
                        deadline_limit,
                        move |(), execution| async move {
                            let _ = started.send(execution.cancellation());
                            let _ = released.await;
                            Ok::<(), ApiError>(())
                        },
                    )
                    .await
            })
        };
        let cancellation = tokio::time::timeout(Duration::from_secs(1), started_receiver).await??;
        retained.push((prepared, request, cancellation, release));
        callers.push(caller);
    }
    for caller in callers {
        let result = tokio::time::timeout(Duration::from_secs(2), caller).await??;
        assert_eq!(
            result
                .err()
                .ok_or("uncooperative operation returned success")?
                .code,
            "operation.deadline_exceeded"
        );
    }
    assert!(
        retained
            .iter()
            .all(|(_, _, cancellation, _)| cancellation.is_cancelled()),
        "deadline did not reach every retained worker"
    );
    assert_eq!(registry.heavy.available_permits(), 0);
    assert_eq!(registry.entries().map_err(boxed)?.len(), HEAVY_PERMITS);
    let first = retained.remove(0);
    assert_eq!(
        registry
            .run(
                "main",
                claim(&first.0, &first.1),
                None,
                OperationPhaseV1::Validating,
                ((), None),
                |(), _| async { Ok::<(), ApiError>(()) },
            )
            .await
            .err()
            .ok_or("timed-out reservation was reused")?
            .code,
        "operation.already_claimed"
    );

    let mut queued_request = request()?;
    queued_request.purpose = OperationPurposeV1::SolveEstimate;
    queued_request.context = OperationContextV1::Scenario {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Some(Revision::INITIAL),
    };
    let queued = registry.prepare("main", &queued_request).map_err(boxed)?;
    let (queued_started, mut queued_started_receiver) = oneshot::channel();
    let (queued_release, queued_released) = oneshot::channel();
    let queued_caller = {
        let registry = Arc::clone(&registry);
        let operation = claim(&queued, &queued_request);
        tokio::spawn(async move {
            registry
                .run(
                    "main",
                    operation,
                    None,
                    OperationPhaseV1::Validating,
                    ((), None),
                    move |(), _| async move {
                        let _ = queued_started.send(());
                        let _ = queued_released.await;
                        Ok::<(), ApiError>(())
                    },
                )
                .await
        })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut queued_started_receiver)
            .await
            .is_err(),
        "a queued worker reused a retained heavy permit"
    );
    first.3.send(()).map_err(boxed)?;
    tokio::time::timeout(Duration::from_secs(1), &mut queued_started_receiver).await??;
    queued_release.send(()).map_err(boxed)?;
    let second = retained.remove(0);
    second.3.send(()).map_err(boxed)?;
    queued_caller.await?.map_err(boxed)?;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if registry.entries().map_err(boxed)?.is_empty()
                && registry.heavy.available_permits() == HEAVY_PERMITS
            {
                return Ok::<(), Box<dyn Error>>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}

fn alternatives_request() -> TestResult<(
    OperationPrepareRequestV1,
    eutheto_core::WorkforceAlternativesRequest,
)> {
    let mut request = request()?;
    let scenario_id = ScenarioId::new(&SystemIdGenerator)?;
    let revision = Revision::new(7);
    request.purpose = OperationPurposeV1::SolveAlternatives;
    request.context = OperationContextV1::Scenario {
        scenario_id,
        expected_revision: Some(revision),
    };
    let group = eutheto_core::WorkforceAlternativesRequest {
        request_id: request.request_id,
        scenario_id,
        expected_revision: revision,
        baseline_source: eutheto_core::StoredBaselineSourceV1::Accepted,
        options: super::super::solve_options(eutheto_types::SolveMode::Deep, None)
            .map_err(boxed)?,
    };
    Ok((request, group))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn alternatives_atomic_owner_survives_detachment_and_scopes_cancellation() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let (request, group) = alternatives_request()?;
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    let original_claim = claim(&prepared, &request);
    assert!(
        registry
            .register_alternatives("other", &original_claim, group.clone())
            .is_err()
    );
    assert!(
        registry
            .register_alternatives("main", &original_claim, group.clone())
            .map_err(boxed)?
            .is_none()
    );
    let redundant = registry.prepare("main", &request).map_err(boxed)?;
    let existing = registry
        .register_alternatives("main", &claim(&redundant, &request), group.clone())
        .map_err(boxed)?
        .ok_or("missing active owner")?;
    assert_eq!(existing.cancel_operation_id, Some(prepared.operation_id));
    registry
        .release("main", &control(&redundant, &request))
        .map_err(boxed)?;
    let mut competing = request.clone();
    competing.request_id = RequestId::new(&SystemIdGenerator)?;
    let competing_prepared = registry.prepare("main", &competing).map_err(boxed)?;
    let mut competing_group = group.clone();
    competing_group.request_id = competing.request_id;
    assert_eq!(
        registry
            .register_alternatives(
                "main",
                &claim(&competing_prepared, &competing),
                competing_group
            )
            .err()
            .ok_or("second group was admitted")?
            .code,
        "solve.alternatives_active"
    );
    registry
        .release("main", &control(&competing_prepared, &competing))
        .map_err(boxed)?;
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let running = tokio::spawn({
        let registry = Arc::clone(&registry);
        let time_limit = group.options.time_limit_milliseconds;
        async move {
            registry
                .run_with_solve_budget(
                    "main",
                    original_claim,
                    None,
                    OperationPhaseV1::Validating,
                    ((), None),
                    time_limit,
                    |(), execution| async move {
                        let token = execution.cancellation();
                        let _ = entered.send(token.clone());
                        let _ = released.await;
                        Ok(token.is_cancelled())
                    },
                )
                .await
        }
    });
    let token = tokio::time::timeout(Duration::from_secs(10), started).await??;
    registry
        .release("main", &control(&prepared, &request))
        .map_err(boxed)?;
    assert!(
        !token.is_cancelled(),
        "detaching the panel cancelled its native group"
    );
    let lookup = || {
        registry
            .alternatives_group("main", group.scenario_id, group.expected_revision, None)
            .map_err(boxed)
    };
    assert_eq!(
        lookup()?
            .ok_or("detachment lost active group")?
            .cancel_operation_id,
        Some(prepared.operation_id)
    );
    assert!(
        registry
            .alternatives_group("other", group.scenario_id, group.expected_revision, None)
            .map_err(boxed)?
            .is_none()
    );
    assert!(
        registry
            .alternatives_group("main", group.scenario_id, Revision::new(8), None)
            .map_err(boxed)?
            .is_none()
    );
    assert_eq!(
        registry
            .cancel("main", &control(&prepared, &request))
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    assert_eq!(
        registry
            .cancel_solve(
                "other",
                &control(&prepared, &request),
                request.context.clone()
            )
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    assert_eq!(
        registry
            .cancel_solve(
                "main",
                &control(&prepared, &request),
                OperationContextV1::Scenario {
                    scenario_id: group.scenario_id,
                    expected_revision: Some(Revision::new(8)),
                }
            )
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::NotActive
    );
    assert!(
        !token.is_cancelled(),
        "a foreign cancellation reached the native group"
    );
    assert_eq!(
        registry
            .cancel_solve(
                "main",
                &control(&prepared, &request),
                request.context.clone()
            )
            .map_err(boxed)?
            .acknowledgement,
        CancellationAcknowledgementV1::CancellationRequested
    );
    assert!(token.is_cancelled());
    release.send(()).map_err(boxed)?;
    assert!(
        tokio::time::timeout(Duration::from_secs(10), running)
            .await??
            .map_err(boxed)?
    );
    let retained = lookup()?.ok_or("settlement lost terminal discovery")?;
    assert!(retained.settled);
    assert_eq!(retained.cancel_operation_id, None);
    assert_eq!(registry.heavy.available_permits(), HEAVY_PERMITS);
    assert!(registry.entries().map_err(boxed)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn alternatives_terminal_cache_is_bounded_exact_and_replay_free() -> TestResult {
    let (_directory, registry, _) = fixture().await?;
    let (mut request, mut group) = alternatives_request()?;
    let first = group.request_id;
    for index in 0..=MAX_RESERVATIONS {
        if index > 0 {
            request.request_id = RequestId::new(&SystemIdGenerator)?;
            group.request_id = request.request_id;
        }
        let prepared = registry.prepare("main", &request).map_err(boxed)?;
        assert!(
            registry
                .register_alternatives("main", &claim(&prepared, &request), group.clone())
                .map_err(boxed)?
                .is_none()
        );
        registry
            .run_with_solve_budget(
                "main",
                claim(&prepared, &request),
                None,
                OperationPhaseV1::Validating,
                ((), None),
                group.options.time_limit_milliseconds,
                |(), _| async { Ok(()) },
            )
            .await
            .map_err(boxed)?;
    }
    assert!(
        registry
            .alternatives_group(
                "main",
                group.scenario_id,
                group.expected_revision,
                Some(first)
            )
            .map_err(boxed)?
            .is_none()
    );
    let retained = registry
        .alternatives_group("main", group.scenario_id, group.expected_revision, None)
        .map_err(boxed)?
        .ok_or("latest terminal group disappeared")?;
    assert_eq!(retained.request_id, group.request_id);
    assert!(retained.settled);
    assert_eq!(retained.cancel_operation_id, None);
    assert_eq!(registry.heavy.available_permits(), HEAVY_PERMITS);
    assert!(registry.entries().map_err(boxed)?.is_empty());
    let redundant = registry.prepare("main", &request).map_err(boxed)?;
    assert!(
        registry
            .register_alternatives("main", &claim(&redundant, &request), group.clone())
            .map_err(boxed)?
            .ok_or("retained group was re-admitted")?
            .settled
    );
    group.options.time_limit_milliseconds = DurationMillis::new(29_000)?;
    assert_eq!(
        registry
            .register_alternatives("main", &claim(&redundant, &request), group.clone())
            .err()
            .ok_or("request identity accepted different options")?
            .code,
        "solve.alternatives_request_conflict"
    );
    registry
        .release("main", &control(&redundant, &request))
        .map_err(boxed)?;
    registry.cancel_window("main");
    assert!(
        registry
            .alternatives_group("main", group.scenario_id, group.expected_revision, None)
            .is_err()
    );
    assert!(
        registry
            .alternatives_group("other", group.scenario_id, group.expected_revision, None)
            .map_err(boxed)?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn alternatives_abandoned_invoke_retains_owner_then_recovers_interrupted_metadata()
-> TestResult {
    use eutheto_core::{
        WorkforceAlternativeMemberStatusV1, WorkforceAlternativeMemberV1,
        WorkforceAlternativeProfileV1, WorkforceAlternativesSnapshotV1,
        WorkforceAlternativesStatusV1, WorkforceAlternativesStopV1,
    };
    let (_directory, registry, _) = fixture().await?;
    let (request, group) = alternatives_request()?;
    let prepared = registry.prepare("main", &request).map_err(boxed)?;
    registry
        .register_alternatives("main", &claim(&prepared, &request), group.clone())
        .map_err(boxed)?;
    let snapshot = WorkforceAlternativesSnapshotV1 {
        schema_version: 1,
        request_id: group.request_id,
        scenario_id: group.scenario_id,
        scenario_revision: group.expected_revision,
        started_at: "2026-09-10T12:00:00Z".parse()?,
        capture_id: None,
        status: WorkforceAlternativesStatusV1::Preparing,
        stop_reason: None,
        diagnostic_code: None,
        known_alarm: None,
        members: WorkforceAlternativeProfileV1::ORDER.map(|profile| WorkforceAlternativeMemberV1 {
            profile,
            request_id: None,
            run_id: None,
            status: WorkforceAlternativeMemberStatusV1::Queued,
            phase: None,
            outcome: None,
            equivalence: None,
            warning_code: None,
            known_alarm: None,
            unconfirmed_result_id: None,
            finalization_unknown: false,
        }),
    };
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let caller = tokio::spawn({
        let registry = Arc::clone(&registry);
        let observer = Arc::clone(&registry);
        let operation = claim(&prepared, &request);
        let operation_id = prepared.operation_id;
        let time_limit = group.options.time_limit_milliseconds;
        async move {
            registry
                .run_with_solve_budget(
                    "main",
                    operation,
                    None,
                    OperationPhaseV1::Validating,
                    ((), None),
                    time_limit,
                    move |(), execution| async move {
                        let mut wrong_source = snapshot.clone();
                        wrong_source.scenario_revision = Revision::new(8);
                        assert!(
                            observer
                                .observe_alternatives("main", operation_id, &wrong_source)
                                .is_err()
                        );
                        observer.observe_alternatives("main", operation_id, &snapshot)?;
                        let _ = entered.send(execution.cancellation());
                        let _ = released.await;
                        Ok(())
                    },
                )
                .await
        }
    });
    let token = tokio::time::timeout(Duration::from_secs(10), started).await??;
    caller.abort();
    assert!(
        caller
            .await
            .err()
            .ok_or("invoke caller was not abandoned")?
            .is_cancelled()
    );
    assert!(
        !token.is_cancelled(),
        "caller abandonment cancelled the native Alternatives owner"
    );
    assert!(
        !registry
            .alternatives_group("main", group.scenario_id, group.expected_revision, None)
            .map_err(boxed)?
            .ok_or("lost active group")?
            .settled
    );
    release.send(()).map_err(boxed)?;
    let retained = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let value = registry
                .alternatives_group("main", group.scenario_id, group.expected_revision, None)
                .map_err(boxed)?
                .ok_or("lost terminal group")?;
            if value.settled {
                return TestResult::Ok(value);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let interrupted = retained
        .snapshot
        .ok_or("lost retained source observation")?;
    assert_eq!(interrupted.status, WorkforceAlternativesStatusV1::Stopped);
    assert_eq!(
        interrupted.stop_reason,
        Some(WorkforceAlternativesStopV1::Interrupted)
    );
    assert!(
        interrupted
            .members
            .iter()
            .all(|member| member.status == WorkforceAlternativeMemberStatusV1::Skipped)
    );
    assert_eq!(retained.cancel_operation_id, None);
    Ok(())
}
