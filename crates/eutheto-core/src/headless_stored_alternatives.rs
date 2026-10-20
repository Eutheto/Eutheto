//! One captured source, one parent deadline, and four ordinary independently accepted solves.

use super::{
    EuthetoApp, StoredSolveFailure, StoredSolveRequest, known_alarm_from_error,
    operation_interrupted, solution_contract_error, solve_error, store_error,
};
use eutheto_domain_ir::{ObjectiveProfileId, RunObjectiveOperation, RunTerminalOutcomeV1};
use eutheto_solver_api::{OutputError, ProgressSink, SolveProgressEvent};
use eutheto_types::{
    AppError, KnownVerificationAlarmV1, OperationControl, OperationInterruption, ParentSolveBudget,
    RequestId, Revision, Rfc3339Timestamp, ScenarioId, ScenarioSnapshotId, SolveOptions,
    SolvePhase, SolveRunId, SolveStatus,
};
use eutheto_workforce::assignment_rules::{WorkforceAdmissionBudget, WorkforceAlternativeOutcomes};
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Fixed Phase07 objective profiles, not user-supplied solver parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkforceAlternativeProfileV1 {
    Balanced,
    PreferenceFocused,
    MinimalChanges,
    FairnessFirst,
}

impl WorkforceAlternativeProfileV1 {
    pub const ORDER: [Self; 4] = [
        Self::Balanced,
        Self::PreferenceFocused,
        Self::MinimalChanges,
        Self::FairnessFirst,
    ];

    /// Recorded domain objective identity for this profile.
    #[must_use]
    pub const fn objective_id(self) -> &'static str {
        match self {
            Self::Balanced => "official.workforce.objective.balanced",
            Self::PreferenceFocused => "official.workforce.objective.preference-focused",
            Self::MinimalChanges => "official.workforce.objective.minimal-changes",
            Self::FairnessFirst => "official.workforce.objective.fairness-first",
        }
    }
}

/// Caller identity and immutable source expectation for one native group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkforceAlternativesRequest {
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub baseline_source: super::StoredBaselineSourceV1,
    pub options: SolveOptions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkforceAlternativesStatusV1 {
    Preparing,
    Running,
    Completed,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkforceAlternativeMemberStatusV1 {
    Queued,
    Preparing,
    Running,
    Finished,
    Failed,
    Skipped,
    OutcomeUnknown,
}

/// This classifies suggestion suppression, never score ordering or acceptance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkforceAlternativeEquivalenceV1 {
    NotSuppressed,
    Equivalent {
        profile: WorkforceAlternativeProfileV1,
    },
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkforceAlternativesStopV1 {
    Cancelled,
    TimeLimit,
    SourceChanged,
    Failed,
    VerificationAlarm,
    FinalizationUnknown,
    SummaryUnavailable,
    Interrupted,
}

/// Compact descriptive state; accepted identities still require ordinary native read admission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkforceAlternativeMemberV1 {
    pub profile: WorkforceAlternativeProfileV1,
    pub request_id: Option<RequestId>,
    pub run_id: Option<SolveRunId>,
    pub status: WorkforceAlternativeMemberStatusV1,
    pub phase: Option<SolvePhase>,
    pub outcome: Option<RunTerminalOutcomeV1>,
    pub equivalence: Option<WorkforceAlternativeEquivalenceV1>,
    pub warning_code: Option<String>,
    pub known_alarm: Option<KnownVerificationAlarmV1>,
    pub unconfirmed_result_id: Option<eutheto_types::SolutionId>,
    pub finalization_unknown: bool,
}

/// Bounded live/terminal group observation. This is not a persisted restart/resume record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkforceAlternativesSnapshotV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub started_at: Rfc3339Timestamp,
    pub capture_id: Option<ScenarioSnapshotId>,
    pub status: WorkforceAlternativesStatusV1,
    pub stop_reason: Option<WorkforceAlternativesStopV1>,
    pub diagnostic_code: Option<String>,
    pub known_alarm: Option<KnownVerificationAlarmV1>,
    pub members: [WorkforceAlternativeMemberV1; 4],
}

impl WorkforceAlternativesSnapshotV1 {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            WorkforceAlternativesStatusV1::Completed | WorkforceAlternativesStatusV1::Stopped
        )
    }

    /// A lost native task cannot leave a retained terminal snapshot claiming live work.
    pub fn mark_interrupted(&mut self) {
        if self.is_terminal() {
            return;
        }
        self.stop(
            WorkforceAlternativesStopV1::Interrupted,
            Some("operation.interrupted".to_owned()),
        );
    }

    fn stop(&mut self, reason: WorkforceAlternativesStopV1, diagnostic_code: Option<String>) {
        self.status = WorkforceAlternativesStatusV1::Stopped;
        self.stop_reason = Some(reason);
        self.diagnostic_code = diagnostic_code;
        for member in &mut self.members {
            if member.status == WorkforceAlternativeMemberStatusV1::Queued {
                member.status = WorkforceAlternativeMemberStatusV1::Skipped;
            } else if matches!(
                member.status,
                WorkforceAlternativeMemberStatusV1::Preparing
                    | WorkforceAlternativeMemberStatusV1::Running
            ) {
                member.status = WorkforceAlternativeMemberStatusV1::OutcomeUnknown;
                member.finalization_unknown = true;
            }
        }
    }
}

/// Retains the exact leaf failure, including unknown commit and known-alarm authority.
#[derive(Debug)]
pub struct WorkforceAlternativesFailure {
    pub snapshot: WorkforceAlternativesSnapshotV1,
    pub failure: StoredSolveFailure,
}

pub type WorkforceAlternativesObserver =
    Arc<dyn Fn(&WorkforceAlternativesSnapshotV1) + Send + Sync>;

type SharedState = Arc<Mutex<WorkforceAlternativesSnapshotV1>>;

impl EuthetoApp {
    /// Executes the four fixed profiles under one native source and unchanged parent deadline.
    ///
    /// The caller owns active-request exclusivity and observation retention. A durable first
    /// request without that owner is history-only, never permission to replay a group.
    /// # Errors
    /// Preserves source refusal, cancellation, correctness alarms and uncertain durable outcomes.
    #[allow(clippy::too_many_lines)]
    pub async fn solve_workforce_alternatives(
        &self,
        request: WorkforceAlternativesRequest,
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
        observer: WorkforceAlternativesObserver,
    ) -> Result<WorkforceAlternativesSnapshotV1, WorkforceAlternativesFailure> {
        let state = Arc::new(Mutex::new(WorkforceAlternativesSnapshotV1 {
            schema_version: 1,
            request_id: request.request_id,
            scenario_id: request.scenario_id,
            scenario_revision: request.expected_revision,
            started_at,
            capture_id: None,
            status: WorkforceAlternativesStatusV1::Preparing,
            stop_reason: None,
            diagnostic_code: None,
            known_alarm: None,
            members: WorkforceAlternativeProfileV1::ORDER.map(|profile| {
                WorkforceAlternativeMemberV1 {
                    profile,
                    request_id: None,
                    run_id: None,
                    status: WorkforceAlternativeMemberStatusV1::Queued,
                    phase: None,
                    outcome: None,
                    equivalence: None,
                    warning_code: None,
                    known_alarm: None,
                    finalization_unknown: false,
                    unconfirmed_result_id: None,
                }
            }),
        }));
        publish(&state, &observer);
        let control = OperationControl::Solve(budget.phase_view());
        let result = async {
            request.options.validate().map_err(|_| StoredSolveFailure::BeforeStart(solve_error(
                "solve.options_invalid", "Solve options are invalid.",
            )))?;
            control.check().map_err(|error| StoredSolveFailure::BeforeStart(operation_interrupted(error)))?;
            if self.store.load_solve_run_by_request(request.request_id).await
                .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?.is_some()
            {
                return Err(StoredSolveFailure::BeforeStart(solve_error(
                    "solve.alternatives_summary_unavailable",
                    "This request already has saved work. Read History rather than replaying its Alternatives group.",
                )));
            }
            let capture = self.capture_objective_alternatives_source(
                request.scenario_id, Some(request.expected_revision), request.baseline_source, control.clone(), started_at,
            ).await.map_err(StoredSolveFailure::BeforeStart)?;
            let mut member_requests = [request.request_id; 4];
            for id in &mut member_requests[1..] {
                *id = RequestId::new(self.ids.as_ref())
                    .map_err(|_| StoredSolveFailure::BeforeStart(solution_contract_error()))?;
            }
            {
                let mut snapshot = lock_state(&state);
                snapshot.capture_id = Some(capture.snapshot_id());
                for (member, id) in snapshot.members.iter_mut().zip(member_requests) {
                    member.request_id = Some(id);
                }
            }
            publish(&state, &observer);
            let mut facts: [Option<WorkforceAlternativeOutcomes>; 4] = std::array::from_fn(|_| None);
            let mut admission = WorkforceAdmissionBudget::new(Some(&control));
            for (index, profile) in WorkforceAlternativeProfileV1::ORDER.into_iter().enumerate() {
                if let Err(reason) = control.check() {
                    lock_state(&state).stop(interruption_reason(reason), None);
                    break;
                }
                {
                    let mut snapshot = lock_state(&state);
                    snapshot.status = WorkforceAlternativesStatusV1::Running;
                    snapshot.members[index].status = WorkforceAlternativeMemberStatusV1::Preparing;
                    snapshot.members[index].phase = Some(SolvePhase::Compiling);
                }
                publish(&state, &observer);
                let member_request = StoredSolveRequest {
                    request_id: member_requests[index],
                    scenario_id: request.scenario_id,
                    expected_revision: Some(request.expected_revision),
                    options: request.options.clone(),
                    objective_selection: Some(capture.selection(
                        ObjectiveProfileId::new(profile.objective_id())
                            .map_err(|_| StoredSolveFailure::BeforeStart(solution_contract_error()))?,
                        RunObjectiveOperation::Optimize,
                    )),
                };
                let bound_state = Arc::clone(&state);
                let bound_observer = Arc::clone(&observer);
                let bound = Arc::new(move |run_id| {
                    {
                        let mut snapshot = lock_state(&bound_state);
                        snapshot.members[index].run_id = Some(run_id);
                        snapshot.members[index].status = WorkforceAlternativeMemberStatusV1::Running;
                    }
                    publish(&bound_state, &bound_observer);
                });
                let mut progress = AlternativeProgress {
                    state: Arc::clone(&state), observer: Arc::clone(&observer), index,
                };
                let outcome = match self.solve_stored_with_budget_observer_in_capture(
                    member_request, &mut progress, budget.clone(), started_at, bound, &capture,
                    u8::try_from(4 - index).map_err(|_| StoredSolveFailure::BeforeStart(solution_contract_error()))?,
                ).await {
                    Ok(outcome) => outcome,
                    Err(failure) => {
                        record_failure(&state, index, &failure, &control);
                        return Err(failure);
                    }
                };
                // A competing owner may have won the durable first request after our initial read.
                // Reused work never authorizes three newly allocated member requests.
                if outcome.reused {
                    return Err(StoredSolveFailure::BeforeStart(solve_error(
                        "solve.alternatives_summary_unavailable",
                        "The retained member belongs to earlier work. Read History; this group was not replayed.",
                    )));
                }
                let stored = outcome.state();
                let terminal = stored.manifest.as_ref().ok_or_else(|| StoredSolveFailure::BeforeStart(solution_contract_error()))?;
                {
                    let mut snapshot = lock_state(&state);
                    let member = &mut snapshot.members[index];
                    member.run_id = Some(stored.input.run_id);
                    member.status = WorkforceAlternativeMemberStatusV1::Finished;
                    member.outcome = Some(terminal.outcome.clone());
                    member.warning_code = outcome.warning.as_ref().map(error_code);
                    if outcome.accepted_result_id().is_some() {
                        member.equivalence = Some(WorkforceAlternativeEquivalenceV1::Unavailable);
                    }
                }
                if let RunTerminalOutcomeV1::Accepted { status, solution_id, .. } = &terminal.outcome {
                    self.publish_solve_completed(member_requests[index], request.scenario_id, request.expected_revision,
                        stored.input.run_id, *status, Some(*solution_id));
                } else if let RunTerminalOutcomeV1::NoResult { status } = &terminal.outcome {
                    self.publish_solve_completed(member_requests[index], request.scenario_id, request.expected_revision,
                        stored.input.run_id, *status, None);
                }
                if let Some(portable) = outcome.portable_result()
                    && let Some(context) = portable.run_input.objective_context.as_ref()
                {
                    let extracted = admission.alternative_outcomes(
                        &capture.snapshot.document, &portable.accepted_result, context,
                    );
                    if let Ok(current) = extracted {
                        let mut equivalent = None;
                        let mut comparable = true;
                        for (previous, prior) in facts[..index].iter().enumerate() {
                            if let Some(prior) = prior {
                                match admission.same_alternative_outcomes(prior, &current) {
                                    Ok(true) => { equivalent = Some(WorkforceAlternativeProfileV1::ORDER[previous]); break; }
                                    Ok(false) => {}
                                    Err(_) => { comparable = false; break; }
                                }
                            }
                        }
                        if comparable {
                            lock_state(&state).members[index].equivalence = Some(if let Some(profile) = equivalent {
                                WorkforceAlternativeEquivalenceV1::Equivalent { profile }
                            } else {
                                facts[index] = Some(current);
                                WorkforceAlternativeEquivalenceV1::NotSuppressed
                            });
                        }
                    }
                }
                if let Some(reason) = terminal_stop(&terminal.outcome) {
                    lock_state(&state).stop(reason, None);
                } else if let Err(reason) = control.check() {
                    lock_state(&state).stop(interruption_reason(reason), None);
                }
                publish(&state, &observer);
                if lock_state(&state).is_terminal() {
                    break;
                }
            }
            let mut snapshot = lock_state(&state);
            if !snapshot.is_terminal() {
                snapshot.status = WorkforceAlternativesStatusV1::Completed;
            }
            Ok(())
        }.await;
        if let Err(failure) = result {
            let snapshot = {
                let mut snapshot = lock_state(&state);
                if !snapshot.is_terminal() {
                    let error = failure_error(&failure);
                    snapshot.known_alarm = known_alarm_from_error(error);
                    let reason = if snapshot.known_alarm.is_some() {
                        WorkforceAlternativesStopV1::VerificationAlarm
                    } else if error_code(error) == "solve.alternatives_summary_unavailable" {
                        WorkforceAlternativesStopV1::SummaryUnavailable
                    } else if matches!(error, AppError::Conflict { .. }) {
                        WorkforceAlternativesStopV1::SourceChanged
                    } else if let Err(reason) = control.check() {
                        interruption_reason(reason)
                    } else {
                        WorkforceAlternativesStopV1::Failed
                    };
                    snapshot.stop(reason, Some(error_code(error)));
                }
                snapshot.clone()
            };
            observer(&snapshot);
            return Err(WorkforceAlternativesFailure { snapshot, failure });
        }
        let snapshot = lock_state(&state).clone();
        observer(&snapshot);
        Ok(snapshot)
    }
}

// A poisoned observation lock retains its last native facts; it does not cancel or undo a solve.
fn lock_state(state: &SharedState) -> std::sync::MutexGuard<'_, WorkforceAlternativesSnapshotV1> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn publish(state: &SharedState, observer: &WorkforceAlternativesObserver) {
    let snapshot = lock_state(state).clone();
    observer(&snapshot);
}

fn interruption_reason(reason: OperationInterruption) -> WorkforceAlternativesStopV1 {
    match reason {
        OperationInterruption::Cancelled => WorkforceAlternativesStopV1::Cancelled,
        OperationInterruption::DeadlineExceeded => WorkforceAlternativesStopV1::TimeLimit,
    }
}

fn terminal_stop(outcome: &RunTerminalOutcomeV1) -> Option<WorkforceAlternativesStopV1> {
    match outcome {
        RunTerminalOutcomeV1::VerificationAlarm { .. } => {
            Some(WorkforceAlternativesStopV1::VerificationAlarm)
        }
        RunTerminalOutcomeV1::Interrupted => Some(WorkforceAlternativesStopV1::Interrupted),
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::Cancelled,
        } => Some(WorkforceAlternativesStopV1::Cancelled),
        RunTerminalOutcomeV1::NoResult {
            status:
                SolveStatus::InvalidModel
                | SolveStatus::BackendUnavailable
                | SolveStatus::BackendFailed
                | SolveStatus::Unbounded,
        } => Some(WorkforceAlternativesStopV1::Failed),
        _ => None,
    }
}

fn failure_error(failure: &StoredSolveFailure) -> &AppError {
    match failure {
        StoredSolveFailure::BeforeStart(error)
        | StoredSolveFailure::AfterStart { error, .. }
        | StoredSolveFailure::FinalizationUnconfirmed { error, .. } => error,
        StoredSolveFailure::Preparation(failure) => &failure.error,
    }
}

fn error_code(error: &AppError) -> String {
    let code = match error {
        AppError::Solver(error) => error.code.as_str(),
        AppError::Verification(error) => error.code.as_str(),
        AppError::Storage(error) => error.code.as_str(),
        AppError::Protocol(error) => error.code.as_str(),
        AppError::Ai(error) => error.code.as_str(),
        AppError::Validation(_) => "validation.failed",
        AppError::Conflict { .. } => "revision.conflict",
        AppError::NotFound(_) => "resource.not_found",
        AppError::Unsupported(_) => "operation.unsupported",
        AppError::Internal { .. } => "operation.internal_error",
    };
    if code.len() <= 128 {
        code.to_owned()
    } else {
        "operation.failed".to_owned()
    }
}

fn record_failure(
    state: &SharedState,
    index: usize,
    failure: &StoredSolveFailure,
    control: &OperationControl,
) {
    let error = failure_error(failure);
    let snapshot = &mut *lock_state(state);
    let member = &mut snapshot.members[index];
    member.status = WorkforceAlternativeMemberStatusV1::Failed;
    member.warning_code = Some(error_code(error));
    member.known_alarm = known_alarm_from_error(error);
    match failure {
        StoredSolveFailure::AfterStart {
            run_id,
            cleanup,
            known_alarm,
            ..
        } => {
            member.run_id = Some(*run_id);
            if let Ok(manifest) = cleanup {
                member.outcome = Some(manifest.outcome.clone());
            }
            if known_alarm.is_some() {
                member.known_alarm.clone_from(known_alarm);
            }
        }
        StoredSolveFailure::FinalizationUnconfirmed {
            run_id,
            result_id,
            known_alarm,
            ..
        } => {
            member.run_id = Some(*run_id);
            member.status = WorkforceAlternativeMemberStatusV1::OutcomeUnknown;
            member.finalization_unknown = true;
            member.unconfirmed_result_id = *result_id;
            if known_alarm.is_some() {
                member.known_alarm.clone_from(known_alarm);
            }
        }
        _ => {}
    }
    let reason = if member.known_alarm.is_some()
        || matches!(
            member.outcome,
            Some(RunTerminalOutcomeV1::VerificationAlarm { .. })
        ) {
        WorkforceAlternativesStopV1::VerificationAlarm
    } else if member.finalization_unknown {
        WorkforceAlternativesStopV1::FinalizationUnknown
    } else if matches!(error, AppError::Conflict { .. }) {
        WorkforceAlternativesStopV1::SourceChanged
    } else if let Some(reason) = member.outcome.as_ref().and_then(terminal_stop) {
        reason
    } else if let Err(reason) = control.check() {
        interruption_reason(reason)
    } else {
        WorkforceAlternativesStopV1::Failed
    };
    snapshot.known_alarm.clone_from(&member.known_alarm);
    snapshot.stop(reason, Some(error_code(error)));
}

struct AlternativeProgress {
    state: SharedState,
    observer: WorkforceAlternativesObserver,
    index: usize,
}

impl ProgressSink for AlternativeProgress {
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        let phase = match event {
            SolveProgressEvent::Queued => SolvePhase::Queued,
            SolveProgressEvent::Compiling { .. } => SolvePhase::Compiling,
            SolveProgressEvent::BackendStarted { .. } => SolvePhase::Solving,
            SolveProgressEvent::Verifying => SolvePhase::Verifying,
            SolveProgressEvent::Explaining => SolvePhase::Explaining,
            _ => return Ok(()),
        };
        {
            let mut state = lock_state(&self.state);
            if state.members[self.index].phase == Some(phase) {
                return Ok(());
            }
            state.members[self.index].phase = Some(phase);
        }
        publish(&self.state, &self.observer);
        Ok(())
    }
}
