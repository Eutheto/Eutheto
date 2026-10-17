//! Generic cooperative admission for bounded planning-IR preparation.
//!
//! This is logical work/transient accounting, not an allocator or RSS prediction.  Callers keep
//! one budget for the complete preparation phase, including any repeated diagnostic attempts.

use eutheto_types::{CancellationToken, OperationControl, OperationInterruption};
use serde::Serialize;
use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
/// Failure while admitting or cooperatively executing planning preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningPreparationError {
    /// The inherited operation was cancelled.
    Cancelled,
    /// The inherited phase deadline elapsed.
    DeadlineExceeded,
    /// Cumulative preparation work exceeded its finite ceiling.
    WorkLimitExceeded,
    /// Cumulative transient reservation exceeded its finite ceiling.
    TransientLimitExceeded,
    /// Serialized IR exceeded the caller's encoded-byte ceiling.
    SerializedLimitExceeded,
    /// A checked accounting operation overflowed.
    ArithmeticOverflow,
    /// The borrowed value could not be serialized.
    Serialization,
}

impl fmt::Display for PlanningPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "planning preparation cancelled",
            Self::DeadlineExceeded => "planning preparation deadline exceeded",
            Self::WorkLimitExceeded => "planning preparation work limit exceeded",
            Self::TransientLimitExceeded => "planning preparation transient limit exceeded",
            Self::SerializedLimitExceeded => "planning IR serialized-byte limit exceeded",
            Self::ArithmeticOverflow => "planning preparation accounting overflow",
            Self::Serialization => "planning preparation serialization failed",
        })
    }
}

impl std::error::Error for PlanningPreparationError {}

/// Cumulative work and peak-live transient admission for one preparation phase.
///
/// The budget owns a clone of the caller's control view so it remains valid while a preparation
/// phase crosses synchronous generic helpers.  It never creates a new deadline or cancellation
/// source.  Work is cumulative across repeated attempts; transient reservations are released
/// when their scratch/output lifetime ends and retain the observed peak.
#[derive(Debug)]
pub struct PlanningPreparationBudget {
    control: OperationControl,
    max_work: u64,
    max_transient_bytes: u64,
    work: u64,
    raw_transient_bytes: u64,
    transient_bytes: Arc<AtomicU64>,
    peak_transient_bytes: u64,
}

/// A live transient reservation released only when its custodian is dropped.
#[derive(Debug)]
pub(crate) struct TransientReservation {
    counter: Arc<AtomicU64>,
    bytes: u64,
}

impl TransientReservation {
    pub(crate) fn belongs_to(&self, budget: &PlanningPreparationBudget) -> bool {
        Arc::ptr_eq(&self.counter, &budget.transient_bytes)
    }
}

impl Drop for TransientReservation {
    #[allow(
        clippy::expect_used,
        reason = "Reservations are linear and raw releases cannot debit guard-owned bytes"
    )]
    fn drop(&mut self) {
        let mut live = self.counter.load(Ordering::Relaxed);
        loop {
            let next = live
                .checked_sub(self.bytes)
                .expect("owned transient reservation is retired exactly once");
            match self.counter.compare_exchange_weak(
                live,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => live = actual,
            }
        }
    }
}

impl PlanningPreparationBudget {
    const MIN_TRANSIENT_BYTES: u64 = 64 * 1024 * 1024;

    /// Creates a budget bounded directly by the caller's planning-IR limits.
    ///
    /// Encoded IR size and preparation memory are separate ceilings: serialization admission
    /// uses `max_ir_bytes`, while borrowed traversal scratch may use a bounded multiple of it.
    #[must_use]
    pub fn new(control: OperationControl, limits: crate::PlanningIrLimitsV1) -> Self {
        Self {
            control,
            max_work: limits.max_total_refs.saturating_mul(8),
            max_transient_bytes: limits
                .max_ir_bytes
                .saturating_mul(4)
                .max(Self::MIN_TRANSIENT_BYTES),
            work: 0,
            raw_transient_bytes: 0,
            transient_bytes: Arc::new(AtomicU64::new(0)),
            peak_transient_bytes: 0,
        }
    }
    /// Reserves live capacity as a manually retired raw reservation.
    ///
    /// # Errors
    /// Returns interruption, checked arithmetic failure, or live-capacity exhaustion.
    pub fn reserve_transient(&mut self, amount: u64) -> Result<(), PlanningPreparationError> {
        self.check()?;
        let raw = self
            .raw_transient_bytes
            .checked_add(amount)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        self.add_transient(amount)?;
        self.raw_transient_bytes = raw;
        Ok(())
    }

    pub(crate) fn reserve_transient_owned(
        &mut self,
        amount: u64,
    ) -> Result<TransientReservation, PlanningPreparationError> {
        self.check()?;
        self.add_transient(amount)?;
        Ok(TransientReservation {
            counter: Arc::clone(&self.transient_bytes),
            bytes: amount,
        })
    }

    /// Replaces the inherited control view without resetting cumulative accounting.
    pub fn replace_control(&mut self, control: OperationControl) -> OperationControl {
        std::mem::replace(&mut self.control, control)
    }

    /// Restores an earlier inherited control view without resetting cumulative accounting.
    pub fn restore_control(&mut self, control: OperationControl) {
        self.control = control;
    }

    /// Checks inherited cancellation/deadline state without changing accounting.
    ///
    /// # Errors
    /// Returns inherited cancellation or deadline exhaustion.
    pub fn check(&self) -> Result<(), PlanningPreparationError> {
        match self.control.check() {
            Ok(()) => Ok(()),
            Err(OperationInterruption::Cancelled) => Err(PlanningPreparationError::Cancelled),
            Err(OperationInterruption::DeadlineExceeded) => {
                Err(PlanningPreparationError::DeadlineExceeded)
            }
        }
    }
}

impl PlanningPreparationBudget {
    /// Creates a finite, non-cancellable budget for APIs that predate operation control.
    #[must_use]
    pub fn uncontrolled(limits: crate::PlanningIrLimitsV1) -> Self {
        Self::new(
            OperationControl::Cancellation(CancellationToken::new()),
            limits,
        )
    }

    /// Reserves cumulative finite work before performing it.
    ///
    /// # Errors
    /// Returns interruption, checked arithmetic failure, or finite-work exhaustion.
    pub fn reserve_work(&mut self, amount: u64) -> Result<(), PlanningPreparationError> {
        self.check()?;
        let next = self
            .work
            .checked_add(amount)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        if next > self.max_work {
            return Err(PlanningPreparationError::WorkLimitExceeded);
        }
        self.work = next;
        Ok(())
    }

    fn add_transient(&mut self, amount: u64) -> Result<(), PlanningPreparationError> {
        let mut live = self.transient_bytes.load(Ordering::Relaxed);
        loop {
            let next = live
                .checked_add(amount)
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
            if next > self.max_transient_bytes {
                return Err(PlanningPreparationError::TransientLimitExceeded);
            }
            match self.transient_bytes.compare_exchange_weak(
                live,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.peak_transient_bytes = self.peak_transient_bytes.max(next);
                    return Ok(());
                }
                Err(actual) => live = actual,
            }
        }
    }

    /// Releases a completed raw transient reservation.
    ///
    /// # Errors
    /// Returns arithmetic failure when the release exceeds the outstanding raw reservation.
    pub fn release_transient(&mut self, amount: u64) -> Result<(), PlanningPreparationError> {
        let raw = self
            .raw_transient_bytes
            .checked_sub(amount)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
        let mut live = self.transient_bytes.load(Ordering::Relaxed);
        loop {
            let next = live
                .checked_sub(amount)
                .ok_or(PlanningPreparationError::ArithmeticOverflow)?;
            match self.transient_bytes.compare_exchange_weak(
                live,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.raw_transient_bytes = raw;
                    return Ok(());
                }
                Err(actual) => live = actual,
            }
        }
    }

    /// Computes the conservative scratch reservation associated with encoded bytes.
    ///
    /// # Errors
    /// Returns arithmetic failure when the conservative reservation is not representable.
    pub fn serialized_scratch_bytes(bytes: u64) -> Result<u64, PlanningPreparationError> {
        bytes
            .checked_mul(2)
            .ok_or(PlanningPreparationError::ArithmeticOverflow)
    }

    /// Precharges a borrowed serialized value without retaining its encoded bytes.
    ///
    /// The exact encoded length is counted through a controlled writer before any caller-owned
    /// output buffer can be allocated.  The caller reserves and releases scratch using
    /// [`Self::serialized_scratch_bytes`] around the live preparation phase.
    ///
    /// # Errors
    /// Returns interruption, encoding failure, or serialized-byte/work admission failure.
    pub fn precharge_serialized<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
        max_bytes: u64,
    ) -> Result<u64, PlanningPreparationError> {
        self.check()?;
        let mut writer = CountingWriter {
            budget: self,
            bytes: 0,
            max_bytes,
            failure: None,
        };
        let serialized = serde_json::to_writer(&mut writer, value);
        let bytes = writer.bytes;
        let failure = writer.failure;
        if let Some(error) = failure {
            return Err(error);
        }
        if serialized.is_err() {
            return Err(PlanningPreparationError::Serialization);
        }
        Ok(bytes)
    }

    /// Returns cumulative logical work charged so far.
    #[must_use]
    pub const fn charged_work(&self) -> u64 {
        self.work
    }

    /// Returns peak live transient capacity charged so far.
    #[must_use]
    pub const fn charged_transient_bytes(&self) -> u64 {
        self.peak_transient_bytes
    }
}

struct CountingWriter<'a> {
    budget: &'a mut PlanningPreparationBudget,
    bytes: u64,
    max_bytes: u64,
    failure: Option<PlanningPreparationError>,
}

impl Write for CountingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("serialized length overflow"))?;
        let next = self
            .bytes
            .checked_add(length)
            .ok_or_else(|| io::Error::other("serialized length overflow"))?;
        if next > self.max_bytes {
            self.failure = Some(PlanningPreparationError::SerializedLimitExceeded);
            return Err(io::Error::other(
                "planning IR serialized-byte limit exceeded",
            ));
        }
        let work = length / 64 + 1;
        if let Err(error) = self.budget.reserve_work(work) {
            self.failure = Some(error);
            return Err(io::Error::other("planning preparation admission failed"));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.budget
            .check()
            .map_err(|_| io::Error::other("planning preparation interrupted"))
    }
}
