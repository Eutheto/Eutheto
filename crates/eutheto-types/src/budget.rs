//! Shared cancellation and monotonic parent-budget primitives.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Largest millisecond duration exactly representable by Phase-01 JSON/TypeScript clients.
pub const DURATION_MILLIS_MAX_V1: u64 = 9_007_199_254_740_991;

/// Error produced by strict millisecond-duration construction or conversion.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DurationMillisError {
    /// The value exceeds the version-1 serialized numeric bound.
    AboveMaximum,
    /// A standard duration contains precision finer than one millisecond.
    SubMillisecondPrecision,
}

impl fmt::Display for DurationMillisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AboveMaximum => "duration exceeds the version-1 millisecond bound",
            Self::SubMillisecondPrecision => {
                "duration is not an exact whole number of milliseconds"
            }
        })
    }
}

impl std::error::Error for DurationMillisError {}

/// Strict non-negative whole-millisecond duration for serialized contracts.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct DurationMillis(u64);

impl DurationMillis {
    /// Zero milliseconds.
    pub const ZERO: Self = Self(0);
    /// Largest version-1 serialized duration.
    pub const MAX: Self = Self(DURATION_MILLIS_MAX_V1);

    /// Creates a duration within the version-1 serialized numeric bound.
    ///
    /// # Errors
    ///
    /// Returns [`DurationMillisError::AboveMaximum`] when `value` cannot be
    /// represented exactly by every supported JSON/TypeScript client.
    pub const fn new(value: u64) -> Result<Self, DurationMillisError> {
        if value > DURATION_MILLIS_MAX_V1 {
            Err(DurationMillisError::AboveMaximum)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the underlying whole-millisecond value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Converts to a standard monotonic duration without loss.
    #[must_use]
    pub const fn to_duration(self) -> Duration {
        Duration::from_millis(self.0)
    }

    fn from_duration_floor_saturating(duration: Duration) -> Self {
        let bounded = duration.as_millis().min(u128::from(DURATION_MILLIS_MAX_V1));
        match u64::try_from(bounded) {
            Ok(value) => Self(value),
            Err(_) => Self::MAX,
        }
    }
}

impl<'de> Deserialize<'de> for DurationMillis {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl From<DurationMillis> for Duration {
    fn from(value: DurationMillis) -> Self {
        value.to_duration()
    }
}

impl TryFrom<Duration> for DurationMillis {
    type Error = DurationMillisError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        let milliseconds = value.as_millis();
        if milliseconds > u128::from(DURATION_MILLIS_MAX_V1) {
            return Err(DurationMillisError::AboveMaximum);
        }
        if !value.subsec_nanos().is_multiple_of(1_000_000) {
            return Err(DurationMillisError::SubMillisecondPrecision);
        }
        match u64::try_from(milliseconds) {
            Ok(milliseconds) => Self::new(milliseconds),
            Err(_) => Err(DurationMillisError::AboveMaximum),
        }
    }
}

/// Cloneable process-local cancellation token with hierarchical propagation.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    node: Arc<CancellationNode>,
}

#[derive(Debug, Default)]
struct CancellationNode {
    cancelled: AtomicBool,
    parent: Option<Arc<CancellationNode>>,
}

impl CancellationToken {
    /// Creates an uncancelled root token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an independently cancellable child that observes ancestor cancellation.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            node: Arc::new(CancellationNode {
                cancelled: AtomicBool::new(false),
                parent: Some(Arc::clone(&self.node)),
            }),
        }
    }

    /// Permanently marks this token's shared local node as cancelled.
    ///
    /// Descendants observe the cancellation through ancestor lookup, but their
    /// local flags and independently cancellable sibling nodes remain unchanged.
    pub fn cancel(&self) {
        self.node.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested locally or by an ancestor.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        let mut node = Some(self.node.as_ref());
        while let Some(current) = node {
            if current.cancelled.load(Ordering::Acquire) {
                return true;
            }
            node = current.parent.as_deref();
        }
        false
    }
}

/// Object-safe monotonic time source for deadline accounting.
pub trait MonotonicClock: Send + Sync {
    /// Returns elapsed monotonic time from this clock's arbitrary origin.
    fn now(&self) -> Duration;
}

/// System monotonic clock backed only by [`Instant`].
#[derive(Clone, Debug)]
pub struct SystemMonotonicClock {
    origin: Instant,
}

impl SystemMonotonicClock {
    /// Starts a monotonic clock at a new process-local origin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// Error produced while explicitly controlling a deterministic monotonic clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedMonotonicClockError {
    /// The requested value would move monotonic time backwards.
    WouldMoveBackwards,
    /// Advancing the clock would overflow [`Duration`].
    Overflow,
}

impl fmt::Display for FixedMonotonicClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WouldMoveBackwards => "fixed monotonic clock cannot move backwards",
            Self::Overflow => "fixed monotonic clock overflow",
        })
    }
}

impl std::error::Error for FixedMonotonicClockError {}

/// Deterministic monotonic clock advanced explicitly by tests or simulations.
#[derive(Clone, Debug, Default)]
pub struct FixedMonotonicClock {
    now: Arc<Mutex<Duration>>,
}

impl FixedMonotonicClock {
    /// Creates a clock fixed at `now` until explicitly changed.
    #[must_use]
    pub fn new(now: Duration) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    /// Moves the clock to a later or equal value.
    ///
    /// # Errors
    ///
    /// Returns [`FixedMonotonicClockError::WouldMoveBackwards`] if `now` is
    /// earlier than the current monotonic value.
    pub fn set(&self, now: Duration) -> Result<(), FixedMonotonicClockError> {
        let mut current = self.lock_now();
        if now < *current {
            return Err(FixedMonotonicClockError::WouldMoveBackwards);
        }
        *current = now;
        Ok(())
    }

    /// Advances the clock by an elapsed duration.
    ///
    /// # Errors
    ///
    /// Returns [`FixedMonotonicClockError::Overflow`] when the result cannot be
    /// represented by [`Duration`].
    pub fn advance(&self, elapsed: Duration) -> Result<(), FixedMonotonicClockError> {
        let mut current = self.lock_now();
        let Some(next) = current.checked_add(elapsed) else {
            return Err(FixedMonotonicClockError::Overflow);
        };
        *current = next;
        Ok(())
    }

    fn lock_now(&self) -> MutexGuard<'_, Duration> {
        match self.now.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl MonotonicClock for FixedMonotonicClock {
    fn now(&self) -> Duration {
        *self.lock_now()
    }
}

/// Error creating one absolute parent solve deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParentSolveBudgetError {
    /// The clock value plus requested duration exceeded [`Duration`].
    DeadlineOverflow,
}

impl fmt::Display for ParentSolveBudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("parent solve deadline overflow")
    }
}

impl std::error::Error for ParentSolveBudgetError {}

/// Copyable observation of remaining parent budget and termination state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemainingSolveBudget {
    /// Remaining whole milliseconds, saturated to zero at the deadline.
    pub remaining_milliseconds: DurationMillis,
    /// Whether the absolute parent deadline has been reached.
    pub expired: bool,
    /// Whether explicit cancellation has independently been requested.
    pub cancelled: bool,
}

struct SolveBudgetState {
    deadline: Duration,
    clock: Arc<dyn MonotonicClock>,
    cancellation: CancellationToken,
}

impl SolveBudgetState {
    fn remaining_duration(&self) -> (Duration, bool) {
        let now = self.clock.now();
        if now >= self.deadline {
            (Duration::ZERO, true)
        } else {
            match self.deadline.checked_sub(now) {
                Some(remaining) => (remaining, false),
                None => (Duration::ZERO, true),
            }
        }
    }

    fn snapshot(&self) -> RemainingSolveBudget {
        let (remaining, expired) = self.remaining_duration();
        RemainingSolveBudget {
            remaining_milliseconds: DurationMillis::from_duration_floor_saturating(remaining),
            expired,
            cancelled: self.cancellation.is_cancelled(),
        }
    }
}

/// One end-to-end solve budget anchored to a single absolute monotonic deadline.
#[derive(Clone)]
pub struct ParentSolveBudget {
    state: Arc<SolveBudgetState>,
}

impl ParentSolveBudget {
    /// Creates the sole parent deadline from a duration, clock, and cancellation token.
    ///
    /// # Errors
    ///
    /// Returns [`ParentSolveBudgetError::DeadlineOverflow`] if the clock's
    /// current value plus `time_limit` cannot be represented by [`Duration`].
    pub fn new(
        time_limit: DurationMillis,
        clock: Arc<dyn MonotonicClock>,
        cancellation: CancellationToken,
    ) -> Result<Self, ParentSolveBudgetError> {
        let Some(deadline) = clock.now().checked_add(time_limit.to_duration()) else {
            return Err(ParentSolveBudgetError::DeadlineOverflow);
        };
        Ok(Self {
            state: Arc::new(SolveBudgetState {
                deadline,
                clock,
                cancellation,
            }),
        })
    }

    /// Creates a child phase view bound to this parent's original deadline.
    #[must_use]
    pub fn phase_view(&self) -> SolveBudgetView {
        SolveBudgetView {
            state: Arc::clone(&self.state),
        }
    }

    /// Observes remaining time, expiry, and cancellation at one instant.
    #[must_use]
    pub fn snapshot(&self) -> RemainingSolveBudget {
        self.state.snapshot()
    }

    /// Returns remaining duration, saturated to zero at the deadline.
    #[must_use]
    pub fn remaining_duration(&self) -> Duration {
        self.state.remaining_duration().0
    }

    /// Returns whether the absolute deadline has been reached.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.state.remaining_duration().1
    }

    /// Returns whether explicit cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancellation.is_cancelled()
    }
}

/// Cloneable child view that can only retain, never reset, its parent deadline.
#[derive(Clone)]
pub struct SolveBudgetView {
    state: Arc<SolveBudgetState>,
}

impl SolveBudgetView {
    /// Creates a nested phase view bound to the same original deadline.
    #[must_use]
    pub fn phase_view(&self) -> Self {
        self.clone()
    }

    /// Observes remaining time, expiry, and cancellation at one instant.
    #[must_use]
    pub fn snapshot(&self) -> RemainingSolveBudget {
        self.state.snapshot()
    }

    /// Returns remaining duration, saturated to zero at the deadline.
    #[must_use]
    pub fn remaining_duration(&self) -> Duration {
        self.state.remaining_duration().0
    }

    /// Returns whether the absolute deadline has been reached.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.state.remaining_duration().1
    }

    /// Returns whether explicit cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancellation.is_cancelled()
    }
}

/// Explicit control for ordinary cancellable work or a phase of one immutable solve deadline.
#[derive(Clone)]
pub enum OperationControl {
    Cancellation(CancellationToken),
    Solve(SolveBudgetView),
}

/// An interrupted operation is neither invalid data nor a correctness failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationInterruption {
    Cancelled,
    DeadlineExceeded,
}

impl OperationControl {
    /// Checks cancellation first, then the original solve deadline when present.
    ///
    /// # Errors
    /// Returns the observed interruption without changing or extending the parent budget.
    pub fn check(&self) -> Result<(), OperationInterruption> {
        match self {
            Self::Cancellation(token) if token.is_cancelled() => {
                Err(OperationInterruption::Cancelled)
            }
            Self::Solve(budget) => {
                let remaining = budget.snapshot();
                if remaining.cancelled {
                    Err(OperationInterruption::Cancelled)
                } else if remaining.expired
                    || remaining.remaining_milliseconds == DurationMillis::ZERO
                {
                    Err(OperationInterruption::DeadlineExceeded)
                } else {
                    Ok(())
                }
            }
            Self::Cancellation(_) => Ok(()),
        }
    }
}

impl fmt::Debug for OperationControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancellation(_) => "OperationControl::Cancellation",
            Self::Solve(_) => "OperationControl::Solve",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CancellationToken, DURATION_MILLIS_MAX_V1, DurationMillis, DurationMillisError,
        FixedMonotonicClock, MonotonicClock, ParentSolveBudget,
    };
    use std::sync::Arc;
    use std::time::Duration;

    fn duration_millis(value: u64) -> Result<DurationMillis, DurationMillisError> {
        DurationMillis::new(value)
    }

    fn budget(
        limit_milliseconds: u64,
        clock: &Arc<FixedMonotonicClock>,
        cancellation: CancellationToken,
    ) -> Result<ParentSolveBudget, Box<dyn std::error::Error>> {
        let shared_clock: Arc<dyn MonotonicClock> = clock.clone();
        Ok(ParentSolveBudget::new(
            duration_millis(limit_milliseconds)?,
            shared_clock,
            cancellation,
        )?)
    }

    #[test]
    fn cancellation_clones_share_a_node() {
        let original = CancellationToken::new();
        let clone = original.clone();
        assert!(!original.is_cancelled());
        clone.cancel();
        assert!(original.is_cancelled());
        assert!(clone.is_cancelled());
    }

    #[test]
    fn parent_cancellation_propagates_to_descendants() {
        let parent = CancellationToken::new();
        let child = parent.child();
        let grandchild = child.child();

        parent.cancel();

        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
        assert!(grandchild.is_cancelled());
    }

    #[test]
    fn child_cancellation_is_isolated_from_parent_and_sibling() {
        let parent = CancellationToken::new();
        let child = parent.child();
        let sibling = parent.child();

        child.cancel();

        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
    }

    #[test]
    fn fixed_clock_reports_exact_remaining_time() -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(FixedMonotonicClock::default());
        let budget = budget(750, &clock, CancellationToken::new())?;
        clock.advance(Duration::from_millis(125))?;
        assert_eq!(
            budget.snapshot().remaining_milliseconds,
            duration_millis(625)?
        );
        assert_eq!(budget.remaining_duration(), Duration::from_millis(625));
        assert!(!budget.is_expired());
        Ok(())
    }

    #[test]
    fn nested_phase_views_never_reset_the_parent_deadline() -> Result<(), Box<dyn std::error::Error>>
    {
        let clock = Arc::new(FixedMonotonicClock::default());
        let parent = budget(1_000, &clock, CancellationToken::new())?;
        clock.advance(Duration::from_millis(400))?;
        let phase = parent.phase_view();
        clock.advance(Duration::from_millis(300))?;
        let nested = phase.phase_view();
        assert_eq!(
            phase.snapshot().remaining_milliseconds,
            duration_millis(300)?
        );
        assert_eq!(
            nested.snapshot().remaining_milliseconds,
            duration_millis(300)?
        );
        Ok(())
    }

    #[test]
    fn deadline_expiry_is_exact_and_saturating() -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(FixedMonotonicClock::default());
        let parent = budget(250, &clock, CancellationToken::new())?;
        clock.advance(Duration::from_millis(250))?;
        assert_eq!(parent.remaining_duration(), Duration::ZERO);
        assert!(parent.is_expired());
        clock.advance(Duration::from_secs(1))?;
        let after = parent.snapshot();
        assert_eq!(after.remaining_milliseconds, DurationMillis::ZERO);
        assert!(after.expired);
        assert!(!after.cancelled);
        Ok(())
    }

    #[test]
    fn cancellation_and_expiry_remain_distinct_states() -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(FixedMonotonicClock::default());
        let cancellation = CancellationToken::new();
        let parent = budget(500, &clock, cancellation.clone())?;
        cancellation.cancel();
        let cancelled = parent.snapshot();
        assert!(cancelled.cancelled);
        assert!(!cancelled.expired);
        assert_eq!(cancelled.remaining_milliseconds, duration_millis(500)?);
        clock.advance(Duration::from_millis(500))?;
        let both = parent.snapshot();
        assert!(both.cancelled);
        assert!(both.expired);
        Ok(())
    }

    #[test]
    fn operation_control_clones_retain_deadline_and_cancellation_precedence()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::{OperationControl, OperationInterruption};
        let clock = Arc::new(FixedMonotonicClock::default());
        let token = CancellationToken::new();
        let parent = budget(10, &clock, token.clone())?;
        let control = OperationControl::Solve(parent.phase_view());
        clock.advance(Duration::from_millis(9))?;
        let nested = control.clone();
        assert_eq!(nested.check(), Ok(()));
        clock.advance(Duration::from_millis(1))?;
        assert_eq!(nested.check(), Err(OperationInterruption::DeadlineExceeded));
        token.cancel();
        assert_eq!(control.check(), Err(OperationInterruption::Cancelled));
        assert_eq!(nested.check(), Err(OperationInterruption::Cancelled));
        assert_eq!(
            OperationControl::Cancellation(token).check(),
            Err(OperationInterruption::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn duration_conversion_enforces_precision_and_serialized_bounds()
    -> Result<(), Box<dyn std::error::Error>> {
        let maximum = DurationMillis::new(DURATION_MILLIS_MAX_V1)?;
        assert_eq!(
            maximum.to_duration(),
            Duration::from_millis(DURATION_MILLIS_MAX_V1)
        );
        assert_eq!(DurationMillis::try_from(maximum.to_duration())?, maximum);
        assert_eq!(
            DurationMillis::new(DURATION_MILLIS_MAX_V1 + 1),
            Err(DurationMillisError::AboveMaximum)
        );
        assert_eq!(
            DurationMillis::try_from(Duration::from_millis(DURATION_MILLIS_MAX_V1 + 1)),
            Err(DurationMillisError::AboveMaximum)
        );
        assert_eq!(
            DurationMillis::try_from(Duration::from_nanos(1)),
            Err(DurationMillisError::SubMillisecondPrecision)
        );
        assert!(serde_json::from_str::<DurationMillis>("-1").is_err());
        assert!(serde_json::from_str::<DurationMillis>("1.5").is_err());
        assert!(serde_json::from_str::<DurationMillis>("\"1\"").is_err());
        assert!(
            serde_json::from_str::<DurationMillis>(&format!("{}", DURATION_MILLIS_MAX_V1 + 1))
                .is_err()
        );
        Ok(())
    }
}
