use super::*;
use eutheto_types::{FixedMonotonicClock, SystemIdGenerator};
use std::{error::Error, io, sync::Barrier, thread};

type TestResult = Result<(), Box<dyn Error>>;

fn fixture() -> (Arc<ReportCustody>, Arc<FixedMonotonicClock>) {
    let clock = Arc::new(FixedMonotonicClock::default());
    let custody = Arc::new(ReportCustody::new(
        clock.clone(),
        Arc::new(SystemIdGenerator),
        ["main".to_owned(), "other".to_owned()],
    ));
    (custody, clock)
}

fn scope() -> Result<Scope, Box<dyn Error>> {
    Ok(Scope {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        expected_revision: Revision::INITIAL,
        kind: CaptureKind::Recipient,
    })
}

fn reserve(
    custody: &Arc<ReportCustody>,
    owner: &str,
    scope: Scope,
) -> Result<OperationId, Box<dyn Error>> {
    let operation_id = OperationId::new(&SystemIdGenerator)?;
    let created = custody.reservation_instant().map_err(boxed)?;
    custody
        .reserve(owner, operation_id, scope, created)
        .map_err(boxed)?;
    Ok(operation_id)
}

fn boxed(error: impl std::fmt::Debug) -> Box<dyn Error> {
    io::Error::other(format!("{error:?}")).into()
}

fn target(operation_id: OperationId) -> Target {
    Target::Operation { operation_id }
}

fn assert_code<T>(result: Result<T, ApiError>, code: &str) {
    assert!(matches!(result, Err(error) if error.code == code));
}

#[test]
fn release_before_begin_never_recreates_the_reservation() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let operation_id = reserve(&custody, "main", scope)?;
    assert_eq!(custody.state().charged_bytes, base + ENTRY_ALLOWANCE);
    assert_eq!(
        custody.release("main", scope.scenario_id, scope.kind, target(operation_id)),
        ReleaseAcknowledgement::Released
    );
    assert_code(
        custody.begin("main", operation_id, scope),
        "recipient.preview_not_active",
    );
    assert_eq!(custody.state().charged_bytes, base);
    assert_eq!(
        custody.release("main", scope.scenario_id, scope.kind, target(operation_id)),
        ReleaseAcknowledgement::NotActive
    );
    Ok(())
}

#[test]
fn reservations_count_globally_and_per_configured_window() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let first = reserve(&custody, "main", scope)?;
    reserve(
        &custody,
        "main",
        Scope {
            kind: CaptureKind::Schedule,
            ..scope
        },
    )?;
    let rejected = OperationId::new(&SystemIdGenerator)?;
    assert_code(
        custody.reserve("main", rejected, scope, Duration::ZERO),
        "recipient.preview_capacity",
    );
    reserve(
        &custody,
        "other",
        Scope {
            kind: CaptureKind::Schedule,
            ..scope
        },
    )?;
    assert_code(
        custody.reserve("other", rejected, scope, Duration::ZERO),
        "recipient.preview_capacity",
    );
    assert_eq!(custody.state().entries.len(), MAX_REPORTS);
    assert_code(
        custody.reserve("unknown", rejected, scope, Duration::ZERO),
        "recipient.preview_not_active",
    );
    custody.release_reserved("other", first);
    assert_eq!(custody.state().entries.len(), MAX_REPORTS);
    custody.release_reserved("main", first);
    custody
        .reserve("other", rejected, scope, Duration::ZERO)
        .map_err(boxed)?;
    assert_eq!(custody.state().entries.len(), MAX_REPORTS);
    Ok(())
}

#[test]
fn wrong_owner_and_exact_scope_cannot_begin_or_release() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let wrong_scope = Scope {
        scenario_id: ScenarioId::new(&SystemIdGenerator)?,
        ..scope
    };
    let wrong_revision = Scope {
        expected_revision: scope.expected_revision.checked_next()?,
        ..scope
    };
    let operation_id = reserve(&custody, "main", scope)?;
    assert_code(
        custody.begin("other", operation_id, scope),
        "recipient.preview_not_active",
    );
    assert_code(
        custody.begin("main", operation_id, wrong_scope),
        "recipient.preview_binding_mismatch",
    );
    assert_code(
        custody.begin("main", operation_id, wrong_revision),
        "recipient.preview_binding_mismatch",
    );
    assert_code(
        custody.begin(
            "main",
            operation_id,
            Scope {
                kind: CaptureKind::Schedule,
                ..scope
            },
        ),
        "recipient.preview_binding_mismatch",
    );
    assert_eq!(
        custody.release(
            "main",
            scope.scenario_id,
            CaptureKind::Schedule,
            target(operation_id)
        ),
        ReleaseAcknowledgement::NotActive,
    );
    assert_eq!(
        custody.release("other", scope.scenario_id, scope.kind, target(operation_id)),
        ReleaseAcknowledgement::NotActive
    );
    assert_eq!(
        custody.release(
            "main",
            wrong_scope.scenario_id,
            scope.kind,
            target(operation_id)
        ),
        ReleaseAcknowledgement::NotActive
    );
    let preparing = custody.begin("main", operation_id, scope).map_err(boxed)?;
    assert_code(
        custody.begin("main", operation_id, scope),
        "recipient.preview_not_active",
    );
    custody.release_reserved("main", operation_id);
    assert_eq!(custody.state().entries.len(), 1);
    drop(preparing);
    assert!(custody.state().entries.is_empty());
    Ok(())
}

#[test]
fn expiry_is_conservative_and_active_work_does_not_expire() -> TestResult {
    let (custody, clock) = fixture();
    let scope = scope()?;
    let first = reserve(&custody, "main", scope)?;
    let active_id = reserve(&custody, "main", scope)?;
    let preparing = custody.begin("main", active_id, scope).map_err(boxed)?;
    clock.advance(RESERVED_TTL)?;
    assert_code(
        custody.begin("main", first, scope),
        "recipient.preview_not_active",
    );
    assert_eq!(custody.state().entries.len(), 1);
    let late_id = OperationId::new(&SystemIdGenerator)?;
    assert_code(
        custody.reserve("main", late_id, scope, Duration::ZERO),
        "recipient.preview_not_active",
    );
    reserve(&custody, "main", scope)?;
    clock.advance(READY_TTL)?;
    custody.reservation_instant().map_err(boxed)?;
    assert_eq!(custody.state().entries.len(), 1);
    drop(preparing);
    assert!(custody.state().entries.is_empty());
    Ok(())
}

#[test]
fn close_and_shutdown_hold_active_count_until_guard_settlement() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let active_id = reserve(&custody, "main", scope)?;
    let active = custody.begin("main", active_id, scope).map_err(boxed)?;
    reserve(&custody, "main", scope)?;
    let other_id = reserve(&custody, "other", scope)?;
    let other = custody.begin("other", other_id, scope).map_err(boxed)?;
    custody.close_window("unknown");
    assert_eq!(custody.state().windows.len(), 2);
    custody.close_window("main");
    assert_eq!(custody.state().entries.len(), 2);
    assert_eq!(custody.state().charged_bytes, base + 2 * ENTRY_ALLOWANCE);
    assert_eq!(
        custody.release("main", scope.scenario_id, scope.kind, target(active_id)),
        ReleaseAcknowledgement::Closing
    );
    assert_code(
        custody.reserve(
            "main",
            OperationId::new(&SystemIdGenerator)?,
            scope,
            Duration::ZERO,
        ),
        "recipient.preview_not_active",
    );
    custody.shutdown();
    assert_eq!(custody.state().charged_bytes, base + 2 * ENTRY_ALLOWANCE);
    drop(active);
    assert_eq!(custody.state().charged_bytes, base + ENTRY_ALLOWANCE);
    drop(other);
    assert_eq!(custody.state().charged_bytes, base);
    assert_code(
        custody.reserve(
            "other",
            OperationId::new(&SystemIdGenerator)?,
            scope,
            Duration::ZERO,
        ),
        "recipient.preview_not_active",
    );
    Ok(())
}

#[test]
fn checked_byte_ledger_rejects_overflow_and_refunds_only_on_settlement() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let operation_id = reserve(&custody, "main", scope)?;
    let preparing = custody.begin("main", operation_id, scope).map_err(boxed)?;
    // Exercise the same checked ledger used by stage, without manufacturing a Core capture.
    let payload = MAX_BYTES - base - ENTRY_ALLOWANCE;
    custody.state().charge_entry(0, payload).map_err(boxed)?;
    assert_eq!(custody.state().charged_bytes, MAX_BYTES);
    assert_code(
        custody.state().charge_entry(0, payload + 1),
        "recipient.preview_capacity",
    );
    assert_code(
        custody.state().charge_entry(0, usize::MAX),
        "recipient.preview_capacity",
    );
    assert_eq!(custody.state().charged_bytes, MAX_BYTES);
    assert_code(
        custody.reserve(
            "other",
            OperationId::new(&SystemIdGenerator)?,
            scope,
            Duration::ZERO,
        ),
        "recipient.preview_capacity",
    );
    assert_eq!(
        custody.release("main", scope.scenario_id, scope.kind, target(operation_id)),
        ReleaseAcknowledgement::Closing
    );
    custody.close_window("main");
    custody.shutdown();
    assert_eq!(custody.state().charged_bytes, MAX_BYTES);
    drop(preparing);
    assert_eq!(custody.state().charged_bytes, base);
    custody.release_reserved("main", operation_id);
    assert_eq!(custody.state().charged_bytes, base);
    Ok(())
}

struct RegressingClock(Mutex<Duration>);

impl MonotonicClock for RegressingClock {
    fn now(&self) -> Duration {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[test]
fn clock_regression_refuses_admission_but_cleanup_remains_available() -> TestResult {
    let clock = Arc::new(RegressingClock(Mutex::new(Duration::from_secs(10))));
    let custody = Arc::new(ReportCustody::new(
        clock.clone(),
        Arc::new(SystemIdGenerator),
        ["main".to_owned()],
    ));
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let operation_id = reserve(&custody, "main", scope)?;
    let active_id = reserve(&custody, "main", scope)?;
    let active = custody.begin("main", active_id, scope).map_err(boxed)?;
    *clock.0.lock().map_err(boxed)? = Duration::from_secs(9);
    assert_code(custody.reservation_instant(), "recipient.clock_invalid");
    assert_code(
        custody.begin("main", operation_id, scope),
        "recipient.clock_invalid",
    );
    assert_code(
        custody.reserve(
            "main",
            OperationId::new(&SystemIdGenerator)?,
            scope,
            Duration::from_secs(9),
        ),
        "recipient.clock_invalid",
    );
    assert_eq!(
        custody.release("main", scope.scenario_id, scope.kind, target(operation_id)),
        ReleaseAcknowledgement::Released
    );
    custody.close_window("main");
    assert_eq!(custody.state().charged_bytes, base + ENTRY_ALLOWANCE);
    drop(active);
    assert_eq!(custody.state().charged_bytes, base);
    Ok(())
}

#[test]
fn reservation_time_overflow_and_future_instants_are_refused_atomically() -> TestResult {
    let (custody, clock) = fixture();
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let operation_id = OperationId::new(&SystemIdGenerator)?;
    assert_code(
        custody.reserve("main", operation_id, scope, Duration::from_secs(1)),
        "recipient.clock_invalid",
    );
    clock.set(Duration::MAX)?;
    assert_code(
        custody.reserve("main", operation_id, scope, Duration::MAX),
        "recipient.clock_invalid",
    );
    assert_eq!(custody.state().charged_bytes, base);
    assert!(custody.state().entries.is_empty());
    Ok(())
}

#[test]
fn begin_release_race_has_one_owner_and_one_refund() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let operation_id = reserve(&custody, "main", scope)?;
    let barrier = Arc::new(Barrier::new(2));
    let begin_custody = Arc::clone(&custody);
    let begin_barrier = Arc::clone(&barrier);
    let begin = thread::spawn(move || {
        begin_barrier.wait();
        begin_custody.begin("main", operation_id, scope)
    });
    barrier.wait();
    let acknowledgement =
        custody.release("main", scope.scenario_id, scope.kind, target(operation_id));
    let result = begin
        .join()
        .map_err(|_| io::Error::other("begin thread failed"))?;
    match result {
        Ok(preparing) => {
            assert_eq!(acknowledgement, ReleaseAcknowledgement::Closing);
            assert_eq!(custody.state().charged_bytes, base + ENTRY_ALLOWANCE);
            drop(preparing);
        }
        Err(error) => {
            assert_eq!(acknowledgement, ReleaseAcknowledgement::Released);
            assert_eq!(error.code, "recipient.preview_not_active");
        }
    }
    assert_eq!(custody.state().charged_bytes, base);
    assert!(custody.state().entries.is_empty());
    Ok(())
}

#[test]
fn generic_release_race_removes_only_unclaimed_custody() -> TestResult {
    let (custody, _) = fixture();
    let scope = scope()?;
    let base = custody.state().charged_bytes;
    let operation_id = reserve(&custody, "main", scope)?;
    let barrier = Arc::new(Barrier::new(2));
    let begin_custody = Arc::clone(&custody);
    let begin_barrier = Arc::clone(&barrier);
    let begin = thread::spawn(move || {
        begin_barrier.wait();
        begin_custody.begin("main", operation_id, scope)
    });
    barrier.wait();
    custody.release_reserved("main", operation_id);
    match begin
        .join()
        .map_err(|_| io::Error::other("begin thread failed"))?
    {
        Ok(preparing) => {
            assert_eq!(custody.state().charged_bytes, base + ENTRY_ALLOWANCE);
            assert!(!custody.state().entries[0].closing);
            drop(preparing);
        }
        Err(error) => assert_eq!(error.code, "recipient.preview_not_active"),
    }
    assert_eq!(custody.state().charged_bytes, base);
    assert!(custody.state().entries.is_empty());
    Ok(())
}
