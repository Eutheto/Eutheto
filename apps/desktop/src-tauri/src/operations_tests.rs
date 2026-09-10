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
