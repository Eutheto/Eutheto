use super::{PairingMode, PreferencePriority, Scope, TimeWindow, Weekday};
use crate::ids::{
    AssignmentTypeId, AvailabilityId, CoverageRequirementId, LocationId, WorkCalendarId,
    WorkloadPolicyId,
};
use eutheto_types::{EntityId, PersonId, RuleId};
use serde::{Deserialize, Serialize};

/// Explicit preference data; no stored variant silently becomes a required rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkforcePreference {
    Time {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        direction: PreferenceDirection,
        time_window: TimeWindow,
    },
    AssignmentType {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        direction: PreferenceDirection,
        assignment_type_ids: Vec<AssignmentTypeId>,
    },
    Location {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        direction: PreferenceDirection,
        location_ids: Vec<LocationId>,
    },
    RequestedTimeOff {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        availability_ids: Vec<AvailabilityId>,
    },
    AssignmentTarget {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        calendar_id: WorkCalendarId,
        target_count: u32,
    },
    WorkloadBalance {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        workload_policy_id: WorkloadPolicyId,
    },
    ConsecutiveWork {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        mode: ConsecutiveWorkMode,
        assignment_type_ids: Vec<AssignmentTypeId>,
        threshold: u32,
    },
    Adjacency {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        direction: PreferenceDirection,
        base_schedule_id: EntityId,
        maximum_gap_minutes: u32,
        mode: AdjacencyMode,
    },
    BaseStability {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        base_schedule_id: EntityId,
    },
    TogetherSeparate {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        direction: GroupDirection,
        person_ids: Vec<PersonId>,
        mode: PairingMode,
    },
    PreferredCoverage {
        id: RuleId,
        active: bool,
        scope: Scope,
        priority: PreferencePriority,
        weight: u32,
        coverage_requirement_ids: Vec<CoverageRequirementId>,
    },
}

impl WorkforcePreference {
    /// Returns identity, activation, scope, priority and weight without copying parameters.
    #[must_use]
    pub const fn header(&self) -> (RuleId, bool, &Scope, PreferencePriority, u32) {
        match self {
            Self::Time {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::AssignmentType {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::Location {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::RequestedTimeOff {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::AssignmentTarget {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::WorkloadBalance {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::ConsecutiveWork {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::Adjacency {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::BaseStability {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::TogetherSeparate {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            }
            | Self::PreferredCoverage {
                id,
                active,
                scope,
                priority,
                weight,
                ..
            } => (*id, *active, scope, *priority, *weight),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreferenceDirection {
    Prefer,
    Avoid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupDirection {
    Together,
    Separate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConsecutiveWorkMode {
    ReportingDays {},
    Weekends {
        calendar_id: WorkCalendarId,
        weekdays: Vec<Weekday>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AdjacencyMode {
    Before,
    After,
    Either,
}
