//! Native report custody. Release retires review authority, never cancels an operation.

use crate::{ApiError, boundary_error, map_app_error};
use eutheto_core::{
    CapturedRecipientPdf, CapturedRecipientReport, CapturedScheduleExport,
    RecipientReportMetadataV1, ScheduleExportFormatV1,
};
use eutheto_types::{
    ApiErrorDto, IdGenerator, MonotonicClock, OperationId, RequestId, Revision, ScenarioId,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

const MAX_REPORTS: usize = 3;
const MAX_WINDOW_REPORTS: usize = 2;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const RESERVED_TTL: Duration = Duration::from_secs(30);
pub(crate) const READY_TTL: Duration = Duration::from_mins(10);
// Conservative allowance for registry/guards and each inline entry. Window string and
// vector capacities are charged separately. Payload/response capacity is supplied by
// the caller; allocator overhead/RSS and independently owned clock/ID sources are not.
const REGISTRY_ALLOWANCE: usize = 64 * 1024;
const ENTRY_ALLOWANCE: usize = 4 * 1024;
// Includes the shared consuming guard's Arc header and synchronous capture mutex.
const _: () = assert!(
    size_of::<Entry>()
        + size_of::<PreparingReport>()
        + size_of::<ConsumingReport>()
        + 4 * size_of::<usize>()
        <= ENTRY_ALLOWANCE
);
const MAX_ID_ATTEMPTS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureKind {
    Recipient,
    Schedule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureFormat {
    Html,
    Pdf,
    Schedule(ScheduleExportFormatV1),
}

pub(crate) enum CapturedReport {
    Recipient(CapturedRecipientReport),
    Pdf(CapturedRecipientPdf),
    Schedule(CapturedScheduleExport),
}

impl CapturedReport {
    fn kind(&self) -> CaptureKind {
        match self {
            Self::Recipient(_) | Self::Pdf(_) => CaptureKind::Recipient,
            Self::Schedule(_) => CaptureKind::Schedule,
        }
    }
    fn format(&self) -> CaptureFormat {
        match self {
            Self::Recipient(_) => CaptureFormat::Html,
            Self::Pdf(_) => CaptureFormat::Pdf,
            Self::Schedule(value) => CaptureFormat::Schedule(value.format()),
        }
    }
    pub(super) fn metadata(&self) -> &RecipientReportMetadataV1 {
        match self {
            Self::Recipient(value) => value.metadata(),
            Self::Pdf(value) => value.metadata(),
            Self::Schedule(value) => value.metadata(),
        }
    }
    pub(super) fn admit(&self) -> Result<(), eutheto_types::AppError> {
        match self {
            Self::Recipient(value) => value.admit(),
            Self::Pdf(value) => value.admit(),
            Self::Schedule(value) => value.admit(),
        }
    }
    pub fn into_recipient(self) -> Result<CapturedRecipientReport, ApiError> {
        match self {
            Self::Recipient(value) => Ok(value),
            Self::Pdf(_) | Self::Schedule(_) => Err(binding_mismatch().into()),
        }
    }
    pub fn into_pdf(self) -> Result<CapturedRecipientPdf, ApiError> {
        match self {
            Self::Pdf(value) => Ok(value),
            Self::Recipient(_) | Self::Schedule(_) => Err(binding_mismatch().into()),
        }
    }
    pub(super) fn retained_bytes(
        &self,
        control: &eutheto_types::OperationControl,
    ) -> Result<usize, eutheto_types::AppError> {
        match self {
            Self::Recipient(value) => value.retained_bytes(control),
            Self::Pdf(value) => value.retained_bytes(control),
            Self::Schedule(value) => value.retained_bytes(control),
        }
    }
    pub fn into_schedule(self) -> Result<CapturedScheduleExport, ApiError> {
        match self {
            Self::Schedule(value) => Ok(value),
            Self::Recipient(_) | Self::Pdf(_) => Err(binding_mismatch().into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Scope {
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub kind: CaptureKind,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Target {
    Preview { preview_id: RequestId },
    Operation { operation_id: OperationId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ReleaseAcknowledgement {
    Released,
    Closing,
    NotActive,
}

struct Window {
    label: String,
    closed: bool,
}

// Only three inline entries exist; boxing the one large variant would add an allocation.
#[allow(clippy::large_enum_variant)]
enum Phase {
    Reserved {
        expires: Duration,
    },
    Preparing,
    Ready {
        expires: Duration,
        capture: CapturedReport,
        response_pending: bool,
    },
    Consuming,
}

impl Phase {
    fn active(&self) -> bool {
        matches!(
            self,
            Self::Preparing
                | Self::Consuming
                | Self::Ready {
                    response_pending: true,
                    ..
                }
        )
    }

    fn expired(&self, now: Duration) -> bool {
        match self {
            Self::Reserved { expires }
            | Self::Ready {
                expires,
                response_pending: false,
                ..
            } => now >= *expires,
            Self::Preparing
            | Self::Consuming
            | Self::Ready {
                response_pending: true,
                ..
            } => false,
        }
    }
}

struct Entry {
    owner: usize,
    operation_id: OperationId,
    scope: Scope,
    preview_id: Option<RequestId>,
    phase: Phase,
    closing: bool,
    charged_bytes: usize,
}

struct State {
    windows: Vec<Window>,
    // Fixed maximum capacity: no map nodes, owner copies or secondary ID index.
    entries: Vec<Entry>,
    charged_bytes: usize,
    last_now: Duration,
    shutdown: bool,
}

impl State {
    fn owner(&self, label: &str) -> Option<usize> {
        self.windows.iter().position(|window| window.label == label)
    }

    fn require_open(&self, owner: &str) -> Result<usize, ApiError> {
        let owner = self.owner(owner).ok_or_else(unavailable)?;
        if self.shutdown || self.windows[owner].closed {
            return Err(unavailable().into());
        }
        Ok(owner)
    }

    fn remove(&mut self, index: usize) {
        let entry = self.entries.swap_remove(index);
        let charge = entry.charged_bytes;
        // Drop the actual ready capture before returning its application-owned charge.
        drop(entry);
        self.charged_bytes -= charge;
    }

    fn prune(&mut self, now: Duration) {
        for index in (0..self.entries.len()).rev() {
            if self.entries[index].phase.expired(now) {
                self.remove(index);
            }
        }
    }

    fn charge_entry(&mut self, index: usize, payload_bytes: usize) -> Result<(), ApiError> {
        let charge = ENTRY_ALLOWANCE
            .checked_add(payload_bytes)
            .ok_or_else(capacity)?;
        let total = self
            .charged_bytes
            .checked_sub(self.entries[index].charged_bytes)
            .and_then(|bytes| bytes.checked_add(charge))
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or_else(capacity)?;
        self.entries[index].charged_bytes = charge;
        self.charged_bytes = total;
        Ok(())
    }
}

pub(crate) struct ReportCustody {
    clock: Arc<dyn MonotonicClock>,
    ids: Arc<dyn IdGenerator>,
    state: Mutex<State>,
}

impl ReportCustody {
    pub fn new(
        clock: Arc<dyn MonotonicClock>,
        ids: Arc<dyn IdGenerator>,
        windows: impl IntoIterator<Item = String>,
    ) -> Self {
        let windows: Vec<_> = windows
            .into_iter()
            .map(|label| Window {
                label,
                closed: false,
            })
            .collect();
        let base = windows.iter().fold(
            windows
                .capacity()
                .checked_mul(size_of::<Window>())
                .and_then(|bytes| bytes.checked_add(REGISTRY_ALLOWANCE)),
            |total, window| total.and_then(|bytes| bytes.checked_add(window.label.capacity())),
        );
        let charged_bytes = base.unwrap_or(usize::MAX);
        Self {
            clock,
            ids,
            state: Mutex::new(State {
                windows,
                entries: Vec::with_capacity(MAX_REPORTS),
                charged_bytes,
                last_now: Duration::ZERO,
                shutdown: charged_bytes > MAX_BYTES,
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Cleanup must still retire authority after poisoning. No external source work
        // executes under this mutex; captures are immutable native-owned objects.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn now(&self, state: &mut State) -> Result<Duration, ApiError> {
        let now = self.clock.now();
        if now < state.last_now {
            return Err(clock_error().into());
        }
        state.last_now = now;
        state.prune(now);
        Ok(now)
    }

    fn cleanup_now(&self, state: &mut State) {
        // A faulty clock cannot prevent cleanup or extend any previously observed TTL.
        state.last_now = state.last_now.max(self.clock.now());
        state.prune(state.last_now);
    }

    pub fn reservation_instant(&self) -> Result<Duration, ApiError> {
        self.now(&mut self.state())
    }

    pub fn reserve(
        self: &Arc<Self>,
        owner: &str,
        operation_id: OperationId,
        scope: Scope,
        created: Duration,
    ) -> Result<(), ApiError> {
        let mut state = self.state();
        let now = self.now(&mut state)?;
        let owner = state.require_open(owner)?;
        let expires = created.checked_add(RESERVED_TTL).ok_or_else(clock_error)?;
        if created > now {
            return Err(clock_error().into());
        }
        if now >= expires
            || state
                .entries
                .iter()
                .any(|entry| entry.operation_id == operation_id)
        {
            return Err(unavailable().into());
        }
        if state.entries.len() >= MAX_REPORTS
            || state
                .entries
                .iter()
                .filter(|entry| entry.owner == owner)
                .count()
                >= MAX_WINDOW_REPORTS
        {
            return Err(capacity().into());
        }
        let total = state
            .charged_bytes
            .checked_add(ENTRY_ALLOWANCE)
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or_else(capacity)?;
        state.entries.push(Entry {
            owner,
            operation_id,
            scope,
            preview_id: None,
            phase: Phase::Reserved { expires },
            closing: false,
            charged_bytes: ENTRY_ALLOWANCE,
        });
        state.charged_bytes = total;
        Ok(())
    }

    pub fn begin(
        self: &Arc<Self>,
        owner: &str,
        operation_id: OperationId,
        scope: Scope,
    ) -> Result<PreparingReport, ApiError> {
        let mut state = self.state();
        self.now(&mut state)?;
        let owner = state.require_open(owner)?;
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.owner == owner && entry.operation_id == operation_id)
            .ok_or_else(unavailable)?;
        if entry.scope != scope {
            return Err(binding_mismatch().into());
        }
        if entry.closing || !matches!(entry.phase, Phase::Reserved { .. }) {
            return Err(unavailable().into());
        }
        entry.phase = Phase::Preparing;
        Ok(PreparingReport {
            custody: Arc::clone(self),
            operation_id,
            scope,
            capture: None,
            armed: true,
        })
    }

    pub fn release(
        &self,
        owner: &str,
        scenario_id: ScenarioId,
        kind: CaptureKind,
        target: Target,
    ) -> ReleaseAcknowledgement {
        let mut state = self.state();
        self.cleanup_now(&mut state);
        let Some(owner) = state.owner(owner) else {
            return ReleaseAcknowledgement::NotActive;
        };
        let index = state.entries.iter().position(|entry| {
            entry.owner == owner
                && entry.scope.scenario_id == scenario_id
                && entry.scope.kind == kind
                && match target {
                    Target::Preview { preview_id } => entry.preview_id == Some(preview_id),
                    Target::Operation { operation_id } => entry.operation_id == operation_id,
                }
        });
        let Some(index) = index else {
            return ReleaseAcknowledgement::NotActive;
        };
        if state.entries[index].phase.active() {
            state.entries[index].closing = true;
            ReleaseAcknowledgement::Closing
        } else {
            state.remove(index);
            ReleaseAcknowledgement::Released
        }
    }

    pub fn release_reserved(&self, owner: &str, operation_id: OperationId) {
        let mut state = self.state();
        self.cleanup_now(&mut state);
        let Some(owner) = state.owner(owner) else {
            return;
        };
        if let Some(index) = state.entries.iter().position(|entry| {
            entry.owner == owner
                && entry.operation_id == operation_id
                && matches!(entry.phase, Phase::Reserved { .. })
        }) {
            state.remove(index);
        }
    }

    pub fn close_window(&self, owner: &str) {
        let mut state = self.state();
        let Some(owner) = state.owner(owner) else {
            return;
        };
        state.windows[owner].closed = true;
        for index in (0..state.entries.len()).rev() {
            if state.entries[index].owner == owner {
                if state.entries[index].phase.active() {
                    state.entries[index].closing = true;
                } else {
                    state.remove(index);
                }
            }
        }
    }

    pub fn shutdown(&self) {
        let mut state = self.state();
        state.shutdown = true;
        for index in (0..state.entries.len()).rev() {
            if state.entries[index].phase.active() {
                state.entries[index].closing = true;
            } else {
                state.remove(index);
            }
        }
    }

    pub fn consume(
        self: &Arc<Self>,
        owner: &str,
        preview_id: RequestId,
        scope: Scope,
        format: CaptureFormat,
    ) -> Result<ConsumingReport, ApiError> {
        let mut state = self.state();
        let admission_clock = self.now(&mut state);
        let owner = state.require_open(owner)?;
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.owner == owner && entry.preview_id == Some(preview_id))
            .ok_or_else(unavailable)?;
        if entry.scope != scope {
            return Err(binding_mismatch().into());
        }
        let Phase::Ready { capture, .. } = &entry.phase else {
            return Err(unavailable().into());
        };
        if capture.format() != format {
            return Err(binding_mismatch().into());
        }
        // Only an exact retained source may outrank a failed admission clock.
        capture
            .admit()
            .map_err(|error| Box::new(map_app_error(error)))?;
        admission_clock?;
        if entry.closing
            || !matches!(
                entry.phase,
                Phase::Ready {
                    response_pending: false,
                    ..
                }
            )
        {
            return Err(unavailable().into());
        }
        let Phase::Ready { capture, .. } = std::mem::replace(&mut entry.phase, Phase::Consuming)
        else {
            return Err(unavailable().into());
        };
        Ok(ConsumingReport {
            custody: Arc::clone(self),
            operation_id: entry.operation_id,
            capture: Mutex::new(Some(capture)),
        })
    }

    fn settle(&self, operation_id: OperationId) {
        let mut state = self.state();
        if let Some(index) = state
            .entries
            .iter()
            .position(|entry| entry.operation_id == operation_id)
        {
            state.remove(index);
        }
    }
}

pub(crate) struct PreparingReport {
    custody: Arc<ReportCustody>,
    operation_id: OperationId,
    scope: Scope,
    capture: Option<CapturedReport>,
    armed: bool,
}

impl PreparingReport {
    /// Charge bounded conversion growth before starting the PDF helper. Release and
    /// window closure keep this charge until the owned preparation task settles.
    pub fn reserve_preparation(&mut self, charged_bytes: usize) -> Result<(), ApiError> {
        let mut state = self.custody.state();
        self.custody.now(&mut state)?;
        let index = state
            .entries
            .iter()
            .position(|entry| entry.operation_id == self.operation_id)
            .ok_or_else(unavailable)?;
        let entry = &state.entries[index];
        if entry.closing || !matches!(entry.phase, Phase::Preparing) || self.capture.is_some() {
            return Err(unavailable().into());
        }
        state.charge_entry(index, charged_bytes)
    }

    pub fn stage(
        &mut self,
        capture: CapturedReport,
        charged_bytes: usize,
    ) -> Result<RequestId, ApiError> {
        if capture.metadata().source.scenario_id != self.scope.scenario_id
            || capture.metadata().current_revision != self.scope.expected_revision
            || capture.kind() != self.scope.kind
        {
            return Err(binding_mismatch().into());
        }
        capture
            .admit()
            .map_err(|error| Box::new(map_app_error(error)))?;
        let result = (|| {
            let mut state = self.custody.state();
            self.custody.now(&mut state)?;
            let index = state
                .entries
                .iter()
                .position(|entry| entry.operation_id == self.operation_id)
                .ok_or_else(unavailable)?;
            let entry = &state.entries[index];
            if entry.closing || !matches!(entry.phase, Phase::Preparing) || self.capture.is_some() {
                return Err(unavailable().into());
            }
            let mut preview_id = None;
            for _ in 0..MAX_ID_ATTEMPTS {
                let candidate = RequestId::new(self.custody.ids.as_ref()).map_err(|_| {
                    Box::new(boundary_error(
                        "identity.generation_failed",
                        "A unique report preview identity could not be generated.",
                        None,
                    ))
                })?;
                if !state
                    .entries
                    .iter()
                    .any(|entry| entry.preview_id == Some(candidate))
                {
                    preview_id = Some(candidate);
                    break;
                }
            }
            let preview_id = preview_id.ok_or_else(unavailable)?;
            state.charge_entry(index, charged_bytes)?;
            state.entries[index].preview_id = Some(preview_id);
            Ok(preview_id)
        })();
        match result {
            Ok(preview_id) => {
                self.capture = Some(capture);
                Ok(preview_id)
            }
            Err(error) => {
                // A known source alarm outranks capacity, closing and clock errors.
                capture
                    .admit()
                    .map_err(|error| Box::new(map_app_error(error)))?;
                Err(error)
            }
        }
    }

    pub fn admit(&self) -> Result<(), ApiError> {
        self.capture
            .as_ref()
            .ok_or_else(unavailable)?
            .admit()
            .map_err(|error| Box::new(map_app_error(error)))
    }

    /// Call after dropping the typed response model. The guard remains armed through
    /// encoded-response ownership, including JoinHandle/handler-return abandonment.
    /// On error drop the encoded response/writer before dropping this guard.
    pub fn ready(&mut self) -> Result<(), ApiError> {
        self.admit()?;
        let result = (|| {
            let mut state = self.custody.state();
            let now = self.custody.now(&mut state)?;
            let expires = now.checked_add(READY_TTL).ok_or_else(clock_error)?;
            let entry = state
                .entries
                .iter_mut()
                .find(|entry| entry.operation_id == self.operation_id)
                .ok_or_else(unavailable)?;
            if entry.closing
                || !matches!(entry.phase, Phase::Preparing)
                || entry.preview_id.is_none()
            {
                return Err(unavailable().into());
            }
            let capture = self.capture.take().ok_or_else(unavailable)?;
            entry.phase = Phase::Ready {
                expires,
                capture,
                response_pending: true,
            };
            Ok(())
        })();
        if let Err(error) = result {
            self.admit()?;
            return Err(error);
        }
        Ok(())
    }

    /// Nonfallible settlement after `Response::body` transfers its bytes into Tauri's
    /// `InvokeResponseBody`. Before that transfer an abandoned response must be dropped
    /// before this guard, so application-owned response capacity is never refunded early.
    pub fn complete_response(mut self) {
        let mut state = self.custody.state();
        let Some(index) = state
            .entries
            .iter()
            .position(|entry| entry.operation_id == self.operation_id)
        else {
            return;
        };
        if !matches!(
            state.entries[index].phase,
            Phase::Ready {
                response_pending: true,
                ..
            }
        ) {
            return;
        }
        if state.entries[index].closing {
            state.remove(index);
        } else if let Phase::Ready {
            response_pending, ..
        } = &mut state.entries[index].phase
        {
            *response_pending = false;
        }
        self.armed = false;
    }
}

impl Drop for PreparingReport {
    fn drop(&mut self) {
        if self.armed {
            drop(self.capture.take());
            self.custody.settle(self.operation_id);
        }
    }
}

pub(crate) struct ConsumingReport {
    custody: Arc<ReportCustody>,
    operation_id: OperationId,
    capture: Mutex<Option<CapturedReport>>,
}

impl ConsumingReport {
    pub fn take_capture(&self) -> Result<CapturedReport, ApiError> {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| unavailable().into())
    }

    pub fn source_revision(&self) -> Result<Revision, ApiError> {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|capture| capture.metadata().source.scenario_revision)
            .ok_or_else(|| unavailable().into())
    }

    /// After `take_capture`, Core publication owns source readmission instead.
    pub fn admit(&self) -> Result<(), ApiError> {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .ok_or_else(unavailable)?
            .admit()
            .map_err(|error| Box::new(map_app_error(error)))
    }

    /// Admission can fail before owned work starts. Once taken, Core owns every
    /// publication error; an absent capture must not invent a postcommit refusal.
    pub fn admit_if_retained(&self) -> Result<(), ApiError> {
        let capture = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match capture.as_ref() {
            Some(capture) => capture
                .admit()
                .map_err(|error| Box::new(map_app_error(error))),
            None => Ok(()),
        }
    }
}

impl Drop for ConsumingReport {
    fn drop(&mut self) {
        drop(
            self.capture
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
        // The owned create closure must retain this guard until its moved capture,
        // picker/publication and response work have all settled.
        self.custody.settle(self.operation_id);
    }
}

fn unavailable() -> ApiErrorDto {
    boundary_error(
        "recipient.preview_not_active",
        "The recipient report preview is not active for this window.",
        None,
    )
}

fn capacity() -> ApiErrorDto {
    boundary_error(
        "recipient.preview_capacity",
        "Close an existing recipient report preview before creating another.",
        None,
    )
}

fn binding_mismatch() -> ApiErrorDto {
    boundary_error(
        "recipient.preview_binding_mismatch",
        "The recipient report request does not match the retained scenario revision.",
        None,
    )
}

fn clock_error() -> ApiErrorDto {
    boundary_error(
        "recipient.clock_invalid",
        "Native monotonic report time is unavailable.",
        None,
    )
}

#[cfg(test)]
#[path = "custody_tests.rs"]
mod tests;
