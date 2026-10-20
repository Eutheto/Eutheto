//! Alternatives observations share operation ownership; terminal metadata retains no task or source.

use super::{
    ApiError, MAX_RESERVATIONS, OperationClaim, OperationContextV1, OperationPurposeV1,
    OperationRegistry, Reservation, boundary_error, not_active, resource_limit,
};
use eutheto_core::{WorkforceAlternativesRequest, WorkforceAlternativesSnapshotV1};
use eutheto_types::{ApiErrorDto, CancellationToken, OperationId, RequestId, Revision, ScenarioId};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlternativesGroupDtoV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub scenario_revision: Revision,
    pub baseline_source: eutheto_core::StoredBaselineSourceV1,
    pub cancel_operation_id: Option<OperationId>,
    pub settled: bool,
    pub snapshot: Option<WorkforceAlternativesSnapshotV1>,
}

pub(super) struct AlternativesReservation {
    request: WorkforceAlternativesRequest,
    admitted: u64,
    snapshot: Option<WorkforceAlternativesSnapshotV1>,
}

struct TerminalGroup {
    window_label: String,
    group: AlternativesReservation,
}

#[derive(Default)]
pub(super) struct AlternativesRetention {
    next_admission: u64,
    terminal: VecDeque<TerminalGroup>,
}

impl AlternativesReservation {
    fn dto(
        &self,
        cancel_operation_id: Option<OperationId>,
        settled: bool,
    ) -> AlternativesGroupDtoV1 {
        AlternativesGroupDtoV1 {
            schema_version: 1,
            request_id: self.request.request_id,
            scenario_id: self.request.scenario_id,
            scenario_revision: self.request.expected_revision,
            baseline_source: self.request.baseline_source,
            cancel_operation_id,
            settled,
            snapshot: self.snapshot.clone(),
        }
    }
}

impl OperationRegistry {
    /// Atomic group identity admission happens before asynchronous heavy admission.
    /// A repeated exact request observes its existing owner; it does not start a second task.
    pub fn register_alternatives(
        &self,
        window_label: &str,
        claim: &OperationClaim,
        request: WorkforceAlternativesRequest,
    ) -> Result<Option<AlternativesGroupDtoV1>, ApiError> {
        let mut entries = self.entries()?;
        let reserved = entries
            .get(&claim.operation_id)
            .filter(|entry| entry.window_label == window_label)
            .ok_or_else(not_active)?;
        let context = OperationContextV1::Scenario {
            scenario_id: request.scenario_id,
            expected_revision: Some(request.expected_revision),
        };
        if reserved.purpose != Some(OperationPurposeV1::SolveAlternatives)
            || claim.purpose != reserved.purpose
            || claim.context != context
            || reserved.context != context
            || claim.request_id != request.request_id
            || reserved.signal.cancellation.is_cancelled()
        {
            return Err(boundary_error(
                "operation.claim_mismatch",
                "The Alternatives owner does not match its native reservation.",
                Some("/operationId"),
            )
            .into());
        }
        for (operation_id, entry) in &*entries {
            if entry.window_label != window_label {
                continue;
            }
            let Some(group) = &entry.alternatives else {
                continue;
            };
            if group.request.request_id == request.request_id {
                if group.request != request {
                    return Err(group_request_conflict().into());
                }
                return Ok(Some(group.dto(
                    (!entry.signal.cancellation.is_cancelled()).then_some(*operation_id),
                    false,
                )));
            }
            if group.request.scenario_id == request.scenario_id {
                return Err(boundary_error("solve.alternatives_active", "An Alternatives group is already active for this scenario in this window. Wait for it to settle or cancel its owner.", None).into());
            }
        }
        let mut retention = self.alternatives.lock().map_err(|_| resource_limit())?;
        if let Some(retained) = retention.terminal.iter().find(|entry| {
            entry.window_label == window_label
                && entry.group.request.request_id == request.request_id
        }) {
            if retained.group.request != request {
                return Err(group_request_conflict().into());
            }
            return Ok(Some(retained.group.dto(None, true)));
        }
        let entry = entries
            .get_mut(&claim.operation_id)
            .ok_or_else(not_active)?;
        if entry.claimed {
            return Err(group_request_conflict().into());
        }
        entry.solve_request_id = Some(request.request_id);
        retention.next_admission = retention
            .next_admission
            .checked_add(1)
            .ok_or_else(resource_limit)?;
        entry.alternatives = Some(AlternativesReservation {
            request,
            admitted: retention.next_admission,
            snapshot: None,
        });
        Ok(None)
    }

    /// Native context discovery includes terminal groups, so response loss cannot hide settlement.
    pub fn alternatives_group(
        &self,
        window_label: &str,
        scenario_id: ScenarioId,
        revision: Revision,
        request_id: Option<RequestId>,
    ) -> Result<Option<AlternativesGroupDtoV1>, ApiError> {
        if self
            .windows
            .get(window_label)
            .is_none_or(CancellationToken::is_cancelled)
        {
            return Err(not_active().into());
        }
        let entries = self.entries()?;
        let matches = |group: &AlternativesReservation| {
            group.request.scenario_id == scenario_id
                && group.request.expected_revision == revision
                && request_id.is_none_or(|id| group.request.request_id == id)
        };
        if let Some((operation_id, entry, group)) = entries.iter().find_map(|(id, entry)| {
            let group = entry.alternatives.as_ref()?;
            (entry.window_label == window_label && matches(group)).then_some((id, entry, group))
        }) {
            return Ok(Some(group.dto(
                (!entry.signal.cancellation.is_cancelled()).then_some(*operation_id),
                false,
            )));
        }
        let retention = self.alternatives.lock().map_err(|_| resource_limit())?;
        Ok(retention
            .terminal
            .iter()
            .filter(|entry| entry.window_label == window_label && matches(&entry.group))
            .max_by_key(|entry| entry.group.admitted)
            .map(|entry| entry.group.dto(None, true)))
    }

    /// Only a native producer possessing this operation's exact identities may update observation.
    pub fn observe_alternatives(
        &self,
        window_label: &str,
        operation_id: OperationId,
        snapshot: &WorkforceAlternativesSnapshotV1,
    ) -> Result<(), ApiError> {
        let mut entries = self.entries()?;
        let entry = entries
            .get_mut(&operation_id)
            .filter(|entry| entry.window_label == window_label && entry.claimed)
            .ok_or_else(not_active)?;
        let group = entry.alternatives.as_mut().ok_or_else(not_active)?;
        if group.request.request_id != snapshot.request_id
            || group.request.scenario_id != snapshot.scenario_id
            || group.request.expected_revision != snapshot.scenario_revision
        {
            return Err(group_request_conflict().into());
        }
        group.snapshot = Some(snapshot.clone());
        Ok(())
    }

    // Caller holds reservations throughout removal/retention, matching the query lock order.
    pub(super) fn retain_alternatives_terminal(&self, entry: Reservation) {
        let Some(mut group) = entry.alternatives else {
            return;
        };
        if self
            .windows
            .get(&entry.window_label)
            .is_none_or(CancellationToken::is_cancelled)
        {
            return;
        }
        if let Some(snapshot) = &mut group.snapshot {
            snapshot.mark_interrupted();
        }
        if let Ok(mut retention) = self.alternatives.lock() {
            if retention.terminal.len() == MAX_RESERVATIONS {
                retention.terminal.pop_front();
            }
            retention.terminal.push_back(TerminalGroup {
                window_label: entry.window_label,
                group,
            });
        }
    }

    pub(super) fn forget_alternatives_window(&self, window_label: &str) {
        if let Ok(mut retention) = self.alternatives.lock() {
            retention
                .terminal
                .retain(|entry| entry.window_label != window_label);
        }
    }
}

fn group_request_conflict() -> ApiErrorDto {
    boundary_error(
        "solve.alternatives_request_conflict",
        "The Alternatives request identity is already bound to different native work.",
        Some("/requestId"),
    )
}
