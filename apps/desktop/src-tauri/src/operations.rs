//! Window-owned finite operations; caller abandonment signals rather than aborts their work.

use super::{ApiError, boundary_error, map_app_error};
use eutheto_core::{EuthetoApp, SetupQuerySource};
use eutheto_types::{
    ApiErrorDto, CancellationToken, Clock, DurationMillis, IdGenerator, MonotonicClock,
    OperationId, ParentSolveBudget, RequestId, Revision, Rfc3339Timestamp, ScenarioId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

#[path = "operations_alternatives.rs"]
mod alternatives;
pub(super) use alternatives::AlternativesGroupDtoV1;
use alternatives::{AlternativesReservation, AlternativesRetention};

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
    SolveStart,
    SolveAlternatives,
    SolveEstimate,
    SolutionExplain,
    SolutionDiagnosticCopy,
    SolutionDiagnosticExport,
    SharePreview,
    ShareCreate,
    ExportPreview,
    ExportCreate,
    ManualRepairRead,
    ManualRepairReview,
    ManualRepairApply,
    CsvSourceOpen,
    CsvDetect,
    CsvSample,
    CsvPreview,
    CsvApply,
    CsvReportSave,
    SettingsImportPreview,
    SettingsImportApply,
    SettingsExport,
    ProjectImportPreview,
    ProjectImportApply,
    ProjectExportPreview,
    ProjectExportCreate,
    ProjectBackupPreview,
    ProjectBackupCreate,
    ProjectRestorePreview,
    ProjectRestoreApply,
    ProjectUnopenedBundleInspect,
    ProjectUnopenedBundleReexport,
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
    solve_request_id: Option<RequestId>,
    context: OperationContextV1,
    created: Duration,
    claimed: bool,
    signal: Arc<Signal>,
    alternatives: Option<AlternativesReservation>,
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
    alternatives: Mutex<AlternativesRetention>,
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
            alternatives: Mutex::default(),
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
                | OperationPurposeV1::ProjectImportPreview
                | OperationPurposeV1::ProjectImportApply
                | OperationPurposeV1::ProjectBackupPreview
                | OperationPurposeV1::ProjectBackupCreate
                | OperationPurposeV1::ProjectRestorePreview
                | OperationPurposeV1::ProjectRestoreApply
                | OperationPurposeV1::ProjectUnopenedBundleInspect
                | OperationPurposeV1::ProjectUnopenedBundleReexport
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
                let revision_required = matches!(
                    request.purpose,
                    OperationPurposeV1::SettingsImportApply
                        | OperationPurposeV1::ProjectImportApply
                        | OperationPurposeV1::ProjectBackupCreate
                        | OperationPurposeV1::ProjectRestoreApply
                );
                if revision_required != expected_library_revision.is_some() {
                    return Err(boundary_error(
                        "operation.library_revision_mismatch",
                        "The library revision context does not match this operation's purpose.",
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
                    solve_request_id: None,
                    alternatives: None,
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
        self.cancel_matching(window_label, request, |purpose| {
            !matches!(
                purpose,
                Some(OperationPurposeV1::SolveStart | OperationPurposeV1::SolveAlternatives)
            )
        })
    }

    pub fn cancel_export(
        &self,
        window_label: &str,
        request: &OperationControlRequestV1,
    ) -> Result<OperationCancelledV1, ApiError> {
        self.cancel_matching(window_label, request, |purpose| {
            matches!(
                purpose,
                Some(OperationPurposeV1::ExportPreview | OperationPurposeV1::ExportCreate)
            )
        })
    }

    fn cancel_matching(
        &self,
        window_label: &str,
        request: &OperationControlRequestV1,
        accepts: impl FnOnce(Option<&OperationPurposeV1>) -> bool,
    ) -> Result<OperationCancelledV1, ApiError> {
        require_version(request.schema_version)?;
        let entries = self.entries()?;
        let acknowledgement = entries
            .get(&request.operation_id)
            .filter(|entry| entry.window_label == window_label && accepts(entry.purpose.as_ref()))
            .map_or(CancellationAcknowledgementV1::NotActive, |entry| {
                entry.signal.cancel();
                CancellationAcknowledgementV1::CancellationRequested
            });
        Ok(OperationCancelledV1 {
            schema_version: 1,
            acknowledgement,
        })
    }

    /// Returns a live solve handle only for its original request, scenario revision, and
    /// native window. Durable job reads can reattach after the presentation observer leaves.
    pub fn active_solve_operation(
        &self,
        window_label: &str,
        request_id: RequestId,
        context: &OperationContextV1,
    ) -> Result<Option<OperationId>, ApiError> {
        let entries = self.entries()?;
        Ok(entries.iter().find_map(|(operation_id, entry)| {
            (entry.window_label == window_label
                && entry.claimed
                && entry.solve_request_id == Some(request_id)
                && entry.context == *context
                && matches!(entry.purpose, Some(OperationPurposeV1::SolveStart))
                && !entry.signal.cancellation.is_cancelled())
            .then_some(*operation_id)
        }))
    }

    /// Cancels only a live solve reservation owned by this exact window and
    /// immutable scenario context.
    // The native command boundary supplies this context by value; keep that
    // boundary stable while comparing the immutable reservation context.
    #[allow(clippy::needless_pass_by_value)]
    pub fn cancel_solve(
        &self,
        window_label: &str,
        request: &OperationControlRequestV1,
        context: OperationContextV1,
    ) -> Result<OperationCancelledV1, ApiError> {
        require_version(request.schema_version)?;
        let entries = self.entries()?;
        let acknowledgement = entries
            .get(&request.operation_id)
            .filter(|entry| {
                entry.window_label == window_label
                    && entry.context == context
                    && matches!(
                        entry.purpose,
                        Some(
                            OperationPurposeV1::SolveStart | OperationPurposeV1::SolveAlternatives
                        )
                    )
            })
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
                if matches!(
                    entry.purpose,
                    Some(
                        OperationPurposeV1::SolveStart
                            | OperationPurposeV1::SolveAlternatives
                            | OperationPurposeV1::SharePreview
                            | OperationPurposeV1::ShareCreate
                            | OperationPurposeV1::ExportPreview
                            | OperationPurposeV1::ExportCreate
                    )
                ) {
                    // Detaching observation never cancels these owned operations.
                    // Explicit cancellation remains reachable until settlement.
                    ReleaseAcknowledgementV1::Released
                } else {
                    entry.signal.cancel();
                    ReleaseAcknowledgementV1::CancellationRequested
                }
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
        self.forget_alternatives_window(window_label);
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
        if matches!(entry.purpose, Some(OperationPurposeV1::SolveAlternatives))
            && (entry.alternatives.is_none() || entry.solve_request_id != Some(claim.request_id))
        {
            return Err(boundary_error(
                "operation.claim_mismatch",
                "The Alternatives group was not registered for this native request.",
                Some("/requestId"),
            )
            .into());
        }
        entry.claimed = true;
        if matches!(entry.purpose, Some(OperationPurposeV1::SolveStart)) {
            entry.solve_request_id = Some(claim.request_id);
        }
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
        self.run_inner(
            window_label,
            claim,
            progress,
            phase,
            (input, preflight),
            None,
            true,
            CompletionPolicy::AwaitSettlement,
            work,
        )
        .await
    }

    /// Runs a bounded ordinary operation whose parent deadline starts before heavy admission.
    /// Unlike a durable solve, caller abandonment, release, and generic cancellation all cancel
    /// the live work.
    // Keep the established native operation call shape; callers construct these
    // values at the IPC boundary.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_bounded_budget<T, R, F, Fut>(
        self: &Arc<Self>,
        window_label: &str,
        claim: OperationClaim,
        progress: Option<ProgressSink>,
        phase: OperationPhaseV1,
        (input, preflight): (T, Option<OperationPreflight<T>>),
        time_limit: DurationMillis,
        work: F,
    ) -> Result<R, ApiError>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, OperationExecution) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, ApiError>> + Send + 'static,
    {
        self.run_inner(
            window_label,
            claim,
            progress,
            phase,
            (input, preflight),
            Some(time_limit),
            true,
            CompletionPolicy::BoundedRead,
            work,
        )
        .await
    }

    /// Runs a solve with its parent deadline created before heavy-permit
    /// admission. Observer release remains separate from this live owner.
    // Keep the established native operation call shape; callers construct these
    // values at the IPC boundary.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_solve_budget<T, R, F, Fut>(
        self: &Arc<Self>,
        window_label: &str,
        claim: OperationClaim,
        progress: Option<ProgressSink>,
        phase: OperationPhaseV1,
        (input, preflight): (T, Option<OperationPreflight<T>>),
        time_limit: DurationMillis,
        work: F,
    ) -> Result<R, ApiError>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, OperationExecution) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, ApiError>> + Send + 'static,
    {
        self.run_inner(
            window_label,
            claim,
            progress,
            phase,
            (input, preflight),
            Some(time_limit),
            false,
            CompletionPolicy::AwaitSettlement,
            work,
        )
        .await
    }

    /// Bounds mutation admission and precommit work without discarding a committed receipt.
    /// The caller must propagate the supplied budget through its final transaction check.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_with_mutation_budget<T, R, F, Fut>(
        self: &Arc<Self>,
        window_label: &str,
        claim: OperationClaim,
        progress: Option<ProgressSink>,
        phase: OperationPhaseV1,
        (input, preflight): (T, Option<OperationPreflight<T>>),
        time_limit: DurationMillis,
        work: F,
    ) -> Result<R, ApiError>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, OperationExecution) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, ApiError>> + Send + 'static,
    {
        self.run_inner(
            window_label,
            claim,
            progress,
            phase,
            (input, preflight),
            Some(time_limit),
            true,
            CompletionPolicy::AwaitSettlement,
            work,
        )
        .await
    }

    // Admission, cancellation, and deadline sequencing are intentionally kept
    // together so the native scope invariant remains auditable.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_inner<T, R, F, Fut>(
        self: &Arc<Self>,
        window_label: &str,
        claim: OperationClaim,
        progress: Option<ProgressSink>,
        phase: OperationPhaseV1,
        (input, preflight): (T, Option<OperationPreflight<T>>),
        solve_limit: Option<DurationMillis>,
        cancel_on_drop: bool,
        completion_policy: CompletionPolicy,
        work: F,
    ) -> Result<R, ApiError>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, OperationExecution) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, ApiError>> + Send + 'static,
    {
        let active = self.claim(window_label, &claim)?;
        // Ordinary callers, including bounded model estimates, cancel on abandonment; a durable
        // solve is cancelled only by explicit cancel, window destroy, or application shutdown.
        let _cancel_caller_drop =
            cancel_on_drop.then(|| CancelCallerDrop(Arc::clone(&active.signal)));
        let solve_admission = solve_limit
            .map(|time_limit| {
                let cancellation = active.signal.cancellation.child();
                let budget = ParentSolveBudget::new(
                    time_limit,
                    Arc::clone(&self.monotonic_clock),
                    cancellation,
                )
                .map_err(|_| {
                    boundary_error(
                        "solve.budget_invalid",
                        "The solve deadline could not be created safely.",
                        Some("/timeLimitMilliseconds"),
                    )
                })?;
                Ok::<_, ApiError>((budget, self.clock.now()))
            })
            .transpose()?;
        let mut execution = OperationExecution {
            signal: Arc::clone(&active.signal),
            solve_admission,
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
        let bounded = if matches!(completion_policy, CompletionPolicy::BoundedRead) {
            execution
                .solve_admission
                .as_ref()
                .map(|(budget, _)| (budget.clone(), Arc::clone(&execution.signal)))
        } else {
            None
        };
        let heavy = Arc::clone(&self.heavy);
        let job = tauri::async_runtime::spawn(async move {
            let job = async move {
                check_solve_admission(&execution)?;
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
                check_solve_admission(&execution)?;
                // These guards live in the owned task, not in the invoke caller. A deadline
                // response must not release the reservation or heavy permit before `work`
                // returns and all retained capture/render/serialization state is dropped.
                let _active = active;
                execution.report(OperationPhaseV1::WaitingForAdmission);
                let _permit = {
                    let notified = execution.signal.wake.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    check_solve_admission(&execution)?;
                    if let Some((budget, _)) = execution.solve_admission.as_ref() {
                        let deadline = tokio::time::sleep(budget.remaining_duration());
                        tokio::pin!(deadline);
                        tokio::select! {
                            biased;
                            () = &mut notified => return Err(cancelled().into()),
                            () = &mut deadline => return Err(deadline_exceeded().into()),
                            permit = heavy.acquire_owned() => permit.map_err(|_| resource_limit())?,
                        }
                    } else {
                        tokio::select! {
                            biased;
                            () = &mut notified => return Err(cancelled().into()),
                            permit = heavy.acquire_owned() => permit.map_err(|_| resource_limit())?,
                        }
                    }
                };
                check_solve_admission(&execution)?;
                execution.report(phase);
                work(input, execution).await
            };
            job.await
        });
        // Timing out the join detaches the owned job; its reservation and heavy permit remain
        // charged until cooperative native work has actually released its retained resources.
        let completion = if let Some((budget, signal)) = bounded {
            let result = tokio::time::timeout(budget.remaining_duration(), job).await;
            if budget.is_expired() {
                signal.cancel();
                return Err(deadline_exceeded().into());
            }
            let Ok(result) = result else {
                signal.cancel();
                return Err(deadline_exceeded().into());
            };
            result
        } else {
            job.await
        };
        completion.map_err(|_| {
            boundary_error(
                "operation.execution_failed",
                "The native operation could not finish safely.",
                None,
            )
        })?
    }
}

enum CompletionPolicy {
    AwaitSettlement,
    BoundedRead,
}

fn check_solve_admission(execution: &OperationExecution) -> Result<(), ApiError> {
    if execution.signal.cancellation.is_cancelled() {
        return Err(cancelled().into());
    }
    if execution
        .solve_admission
        .as_ref()
        .is_some_and(|(budget, _)| budget.is_expired())
    {
        return Err(deadline_exceeded().into());
    }
    Ok(())
}

struct ActiveReservation {
    registry: Arc<OperationRegistry>,
    operation_id: OperationId,
    signal: Arc<Signal>,
}

impl Drop for ActiveReservation {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.registry.entries()
            && let Some(entry) = entries.remove(&self.operation_id)
        {
            self.registry.retain_alternatives_terminal(entry);
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
    solve_admission: Option<(ParentSolveBudget, Rfc3339Timestamp)>,
    progress: Option<Progress>,
}

impl OperationExecution {
    pub fn cancellation(&self) -> CancellationToken {
        self.signal.cancellation.child()
    }

    /// Takes the solve budget exactly once; it remains owned by the live solve
    /// even when the UI releases its observation.
    pub(super) fn take_solve_admission(&mut self) -> Option<(ParentSolveBudget, Rfc3339Timestamp)> {
        self.solve_admission.take()
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
pub(super) fn cancelled() -> ApiErrorDto {
    boundary_error("operation.cancelled", "The operation was cancelled.", None)
}

pub(super) fn deadline_exceeded() -> ApiErrorDto {
    boundary_error(
        "operation.deadline_exceeded",
        "The operation deadline was reached.",
        None,
    )
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
