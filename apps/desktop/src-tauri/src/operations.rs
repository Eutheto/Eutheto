//! Window-owned finite operations; caller abandonment signals rather than aborts their work.

use super::{ApiError, boundary_error, map_app_error};
use eutheto_core::{EuthetoApp, SetupQuerySource};
use eutheto_types::{
    ApiErrorDto, CancellationToken, Clock, IdGenerator, MonotonicClock, OperationId, RequestId,
    Revision, Rfc3339Timestamp, ScenarioId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

const MAX_RESERVATIONS: usize = 16;
const HEAVY_PERMITS: usize = 2;
const RESERVATION_TTL: Duration = Duration::from_secs(30);
const MAX_ID_ATTEMPTS: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum OperationPurposeV1 {
    ScenarioSummary,
    SetupStatus,
    SetupView { view_id: String },
    CommandPreview { view_id: String },
    FullValidation,
    ApplyReviewedGeneration,
    CsvSourceOpen,
    CsvDetect,
    CsvPreview,
    CsvApply,
    CsvReportSave,
    SettingsImportPreview,
    SettingsImportApply,
    SettingsExport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum OperationContextV1 {
    Scenario {
        scenario_id: ScenarioId,
        expected_revision: Option<Revision>,
    },
    Library {
        expected_library_revision: Option<Revision>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationPrepareRequestV1 {
    pub request_id: RequestId,
    pub schema_version: u32,
    pub purpose: OperationPurposeV1,
    pub context: OperationContextV1,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationPreparedV1 {
    pub schema_version: u32,
    pub operation_id: OperationId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationControlRequestV1 {
    pub request_id: RequestId,
    pub schema_version: u32,
    pub operation_id: OperationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CancellationAcknowledgementV1 {
    CancellationRequested,
    NotActive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ReleaseAcknowledgementV1 {
    Released,
    CancellationRequested,
    NotActive,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationCancelledV1 {
    pub schema_version: u32,
    pub acknowledgement: CancellationAcknowledgementV1,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationReleasedV1 {
    pub schema_version: u32,
    pub acknowledgement: ReleaseAcknowledgementV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum OperationPhaseV1 {
    WaitingForAdmission,
    CapturingSnapshot,
    BuildingView,
    ApplyingPreview,
    Validating,
    PreparingResponse,
    SelectingFile,
    DetectingFormat,
    PublishingReport,
    PublishingFile,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationProgressV1 {
    pub event_version: u32,
    pub timestamp: Rfc3339Timestamp,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub window_label: String,
    pub context: OperationContextV1,
    pub sequence: u32,
    pub phase: OperationPhaseV1,
}

pub(super) type ProgressSink = Arc<dyn Fn(&OperationProgressV1) + Send + Sync>;
pub(super) type OperationPreflight<T> =
    fn(&EuthetoApp, &T, &CancellationToken) -> Result<(), ApiError>;

pub(super) struct OperationClaim {
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub purpose: Option<OperationPurposeV1>,
    pub context: OperationContextV1,
}

struct Signal {
    cancellation: CancellationToken,
    wake: Notify,
}

impl Signal {
    fn cancel(&self) {
        self.cancellation.cancel();
        self.wake.notify_waiters();
    }
}

struct Reservation {
    window_label: String,
    purpose: Option<OperationPurposeV1>,
    context: OperationContextV1,
    created: Duration,
    claimed: bool,
    signal: Arc<Signal>,
}

pub(super) struct OperationRegistry {
    app: EuthetoApp,
    clock: Arc<dyn Clock>,
    monotonic_clock: Arc<dyn MonotonicClock>,
    ids: Arc<dyn IdGenerator>,
    root: CancellationToken,
    // Configured native windows are the authority. A destroyed window cannot reopen a gate
    // through a late queued invoke, and arbitrary labels never allocate owner state.
    windows: BTreeMap<String, CancellationToken>,
    reservations: Mutex<BTreeMap<OperationId, Reservation>>,
    heavy: Arc<Semaphore>,
}

impl OperationRegistry {
    pub fn new(
        app: EuthetoApp,
        clock: Arc<dyn Clock>,
        monotonic_clock: Arc<dyn MonotonicClock>,
        ids: Arc<dyn IdGenerator>,
        windows: impl IntoIterator<Item = String>,
    ) -> Self {
        let root = app.setup_cancellation();
        let windows = windows
            .into_iter()
            .map(|label| (label, root.child()))
            .collect();
        Self {
            app,
            clock,
            monotonic_clock,
            ids,
            root,
            windows,
            reservations: Mutex::default(),
            heavy: Arc::new(Semaphore::new(HEAVY_PERMITS)),
        }
    }

    fn entries(&self) -> Result<MutexGuard<'_, BTreeMap<OperationId, Reservation>>, ApiError> {
        let mut entries = self.reservations.lock().map_err(|_| {
            boundary_error(
                "operation.state_unavailable",
                "Native operation state is unavailable.",
                None,
            )
        })?;
        let now = self.monotonic_clock.now();
        entries.retain(|_, entry| {
            entry.claimed || now.saturating_sub(entry.created) < RESERVATION_TTL
        });
        Ok(entries)
    }

    pub fn prepare(
        &self,
        window_label: &str,
        request: &OperationPrepareRequestV1,
    ) -> Result<OperationPreparedV1, ApiError> {
        require_version(request.schema_version)?;
        let library_purpose = matches!(
            request.purpose,
            OperationPurposeV1::SettingsImportPreview
                | OperationPurposeV1::SettingsImportApply
                | OperationPurposeV1::SettingsExport
        );
        match request.context {
            OperationContextV1::Scenario {
                expected_revision, ..
            } if !library_purpose => {
                if !matches!(
                    request.purpose,
                    OperationPurposeV1::ScenarioSummary | OperationPurposeV1::SetupStatus
                ) && expected_revision.is_none()
                {
                    return Err(boundary_error(
                        "operation.revision_required",
                        "This operation requires an exact scenario revision.",
                        Some("/context/expectedRevision"),
                    )
                    .into());
                }
            }
            OperationContextV1::Library {
                expected_library_revision,
            } if library_purpose => {
                if matches!(request.purpose, OperationPurposeV1::SettingsImportApply)
                    != expected_library_revision.is_some()
                {
                    return Err(boundary_error(
                        "operation.library_revision_mismatch",
                        "Only a settings apply operation requires an exact library revision.",
                        Some("/context/expectedLibraryRevision"),
                    )
                    .into());
                }
            }
            _ => {
                return Err(boundary_error(
                    "operation.context_mismatch",
                    "The operation purpose does not match its revision context.",
                    Some("/context"),
                )
                .into());
            }
        }
        let source = match &request.purpose {
            OperationPurposeV1::SetupView { view_id } => Some((view_id, SetupQuerySource::Stored)),
            OperationPurposeV1::CommandPreview { view_id } => {
                Some((view_id, SetupQuerySource::CommandPreview))
            }
            _ => None,
        };
        if let Some((view_id, source)) = source {
            self.app
                .preflight_setup_source(view_id, source)
                .map_err(|error| Box::new(map_app_error(error)))?;
        }
        self.reserve(
            window_label,
            Some(request.purpose.clone()),
            request.context.clone(),
        )
    }

    fn reserve(
        &self,
        window_label: &str,
        purpose: Option<OperationPurposeV1>,
        context: OperationContextV1,
    ) -> Result<OperationPreparedV1, ApiError> {
        let mut entries = self.entries()?;
        let owner = self.windows.get(window_label).ok_or_else(not_active)?;
        if owner.is_cancelled() || self.root.is_cancelled() {
            return Err(cancelled().into());
        }
        if entries.len() == MAX_RESERVATIONS {
            return Err(resource_limit().into());
        }
        for _ in 0..MAX_ID_ATTEMPTS {
            let operation_id = OperationId::new(self.ids.as_ref()).map_err(|_| resource_limit())?;
            if entries.contains_key(&operation_id) {
                continue;
            }
            entries.insert(
                operation_id,
                Reservation {
                    window_label: window_label.to_owned(),
                    purpose,
                    context,
                    created: self.monotonic_clock.now(),
                    claimed: false,
                    signal: Arc::new(Signal {
                        cancellation: owner.child(),
                        wake: Notify::new(),
                    }),
                },
            );
            return Ok(OperationPreparedV1 {
                schema_version: 1,
                operation_id,
            });
        }
        Err(resource_limit().into())
    }

    /// Legacy accepted-result reads share capacity without adding a public prepare token.
    pub fn reserve_accepted(
        &self,
        window_label: &str,
        context: OperationContextV1,
    ) -> Result<OperationPreparedV1, ApiError> {
        self.reserve(window_label, None, context)
    }

    pub fn cancel(
        &self,
        window_label: &str,
        request: &OperationControlRequestV1,
    ) -> Result<OperationCancelledV1, ApiError> {
        require_version(request.schema_version)?;
        let entries = self.entries()?;
        let acknowledgement = entries
            .get(&request.operation_id)
            .filter(|entry| entry.window_label == window_label)
            .map_or(CancellationAcknowledgementV1::NotActive, |entry| {
                entry.signal.cancel();
                CancellationAcknowledgementV1::CancellationRequested
            });
        Ok(OperationCancelledV1 {
            schema_version: 1,
            acknowledgement,
        })
    }

    pub fn release(
        &self,
        window_label: &str,
        request: &OperationControlRequestV1,
    ) -> Result<OperationReleasedV1, ApiError> {
        require_version(request.schema_version)?;
        let mut entries = self.entries()?;
        let acknowledgement = match entries
            .get(&request.operation_id)
            .filter(|entry| entry.window_label == window_label)
        {
            None => ReleaseAcknowledgementV1::NotActive,
            Some(entry) if entry.claimed => {
                entry.signal.cancel();
                ReleaseAcknowledgementV1::CancellationRequested
            }
            Some(_) => {
                entries.remove(&request.operation_id);
                ReleaseAcknowledgementV1::Released
            }
        };
        Ok(OperationReleasedV1 {
            schema_version: 1,
            acknowledgement,
        })
    }

    pub fn cancel_window(&self, window_label: &str) {
        if let Some(owner) = self.windows.get(window_label) {
            owner.cancel();
        }
        if let Ok(entries) = self.entries() {
            for entry in entries
                .values()
                .filter(|entry| entry.window_label == window_label)
            {
                entry.signal.cancel();
            }
        }
    }

    pub fn shutdown(&self) {
        self.app.request_shutdown();
        self.root.cancel();
        if let Ok(entries) = self.entries() {
            for entry in entries.values() {
                entry.signal.cancel();
            }
        }
    }

    fn claim(
        self: &Arc<Self>,
        window_label: &str,
        claim: &OperationClaim,
    ) -> Result<ActiveReservation, ApiError> {
        let mut entries = self.entries()?;
        let entry = entries
            .get_mut(&claim.operation_id)
            .filter(|entry| entry.window_label == window_label)
            .ok_or_else(not_active)?;
        if entry.purpose != claim.purpose || entry.context != claim.context {
            return Err(boundary_error(
                "operation.claim_mismatch",
                "The operation purpose or revision context does not match its reservation.",
                Some("/operationId"),
            )
            .into());
        }
        if entry.claimed {
            return Err(boundary_error(
                "operation.already_claimed",
                "This operation has already been claimed.",
                Some("/operationId"),
            )
            .into());
        }
        if entry.signal.cancellation.is_cancelled() {
            entries.remove(&claim.operation_id);
            return Err(cancelled().into());
        }
        entry.claimed = true;
        Ok(ActiveReservation {
            registry: Arc::clone(self),
            operation_id: claim.operation_id,
            signal: Arc::clone(&entry.signal),
        })
    }

    pub async fn run<T, R, F, Fut>(
        self: &Arc<Self>,
        window_label: &str,
        claim: OperationClaim,
        progress: Option<ProgressSink>,
        phase: OperationPhaseV1,
        (input, preflight): (T, Option<OperationPreflight<T>>),
        work: F,
    ) -> Result<R, ApiError>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, OperationExecution) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, ApiError>> + Send + 'static,
    {
        let active = self.claim(window_label, &claim)?;
        let _cancel_caller_drop = CancelCallerDrop(Arc::clone(&active.signal));
        let mut execution = OperationExecution {
            signal: Arc::clone(&active.signal),
            progress: progress.map(|sink| Progress {
                sink,
                clock: Arc::clone(&self.clock),
                event: OperationProgressV1 {
                    event_version: 1,
                    timestamp: self.clock.now(),
                    operation_id: claim.operation_id,
                    request_id: claim.request_id,
                    window_label: window_label.to_owned(),
                    context: claim.context,
                    sequence: 0,
                    phase: OperationPhaseV1::WaitingForAdmission,
                },
            }),
        };
        let heavy = Arc::clone(&self.heavy);
        // Dropping the caller's JoinHandle detaches this finite owner; it does not abort it.
        // The reservation and permit stay here until the awaited core/store work really exits.
        tauri::async_runtime::spawn(async move {
            let input = if let Some(preflight) = preflight {
                let registry = Arc::clone(&active.registry);
                let cancellation = execution.cancellation();
                tauri::async_runtime::spawn_blocking(move || {
                    preflight(&registry.app, &input, &cancellation)?;
                    Ok::<T, ApiError>(input)
                })
                .await
                .map_err(|_| {
                    boundary_error(
                        "operation.execution_failed",
                        "The native operation could not finish safely.",
                        None,
                    )
                })??
            } else {
                input
            };
            let _active = active;
            execution.report(OperationPhaseV1::WaitingForAdmission);
            let _permit = {
                let notified = execution.signal.wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if execution.signal.cancellation.is_cancelled() {
                    return Err(cancelled().into());
                }
                tokio::select! {
                    biased;
                    () = &mut notified => return Err(cancelled().into()),
                    permit = heavy.acquire_owned() => permit.map_err(|_| resource_limit())?,
                }
            };
            if execution.signal.cancellation.is_cancelled() {
                return Err(cancelled().into());
            }
            execution.report(phase);
            work(input, execution).await
        })
        .await
        .map_err(|_| {
            boundary_error(
                "operation.execution_failed",
                "The native operation could not finish safely.",
                None,
            )
        })?
    }
}

struct ActiveReservation {
    registry: Arc<OperationRegistry>,
    operation_id: OperationId,
    signal: Arc<Signal>,
}

impl Drop for ActiveReservation {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.registry.entries() {
            entries.remove(&self.operation_id);
        }
    }
}

struct CancelCallerDrop(Arc<Signal>);
impl Drop for CancelCallerDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct Progress {
    sink: ProgressSink,
    clock: Arc<dyn Clock>,
    event: OperationProgressV1,
}

pub(super) struct OperationExecution {
    signal: Arc<Signal>,
    progress: Option<Progress>,
}

impl OperationExecution {
    pub fn cancellation(&self) -> CancellationToken {
        self.signal.cancellation.child()
    }

    pub fn preparing_response(&mut self) {
        self.report(OperationPhaseV1::PreparingResponse);
    }

    pub(super) fn report(&mut self, phase: OperationPhaseV1) {
        if let Some(progress) = &mut self.progress
            && let Some(sequence) = progress.event.sequence.checked_add(1)
        {
            progress.event.sequence = sequence;
            progress.event.timestamp = progress.clock.now();
            progress.event.phase = phase;
            (progress.sink)(&progress.event);
        }
    }
}

pub(super) fn require_version(version: u32) -> Result<(), ApiError> {
    if version == 1 {
        Ok(())
    } else {
        Err(boundary_error(
            "operation.version_unsupported",
            "This operation request version is not supported.",
            Some("/schemaVersion"),
        )
        .into())
    }
}

fn not_active() -> ApiErrorDto {
    boundary_error(
        "operation.not_active",
        "This operation is not active for this window.",
        Some("/operationId"),
    )
}
fn cancelled() -> ApiErrorDto {
    boundary_error("operation.cancelled", "The operation was cancelled.", None)
}
fn resource_limit() -> ApiErrorDto {
    boundary_error(
        "operation.resource_limit",
        "Native operation capacity is unavailable.",
        None,
    )
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
