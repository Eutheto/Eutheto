use super::{PeerGroup, WindowMembership};
use crate::ids::{WorkCalendarId, WorkloadBucketId, WorkloadPolicyId};
use eutheto_types::{EntityId, PersonId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreferencePriority {
    Low,
    Normal,
    High,
    VeryHigh,
}

/// Explicit policy data, not a claim that fairness or repair solving is available.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkforceScorePolicy {
    pub id: EntityId,
    pub profile_key: String,
    pub levels: Vec<ScoreLevel>,
    pub priority_mapping: Vec<PriorityMapping>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub workload_policies: BTreeMap<WorkloadPolicyId, WorkloadPolicy>,
    pub tie_break: TieBreakPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreLevel {
    pub level_key: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PriorityMapping {
    pub priority: PreferencePriority,
    pub level_key: String,
    pub scale: u32,
}

/// A final minimized rank sum, not a guarantee that equal-score assignments are unique.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TieBreakPolicy {
    StableAssignmentRank,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadPolicy {
    pub id: WorkloadPolicyId,
    pub bucket_id: WorkloadBucketId,
    pub calendar_id: WorkCalendarId,
    pub membership: WindowMembership,
    pub peer_group: PeerGroup,
    pub target_mode: WorkloadTargetMode,
    pub penalty: DeviationPenalty,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkloadTargetMode {
    Explicit { targets: Vec<PersonTarget> },
    PersonTargets {},
    WeightedEqualShare {},
}

/// Unique-person reference records; supplied order is preserved but has no target meaning.
/// UUID-keyed numeric values would evade identity remapping.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonTarget {
    pub person_id: PersonId,
    pub target: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DeviationPenalty {
    Absolute {},
    Piecewise { breakpoints: Vec<PenaltyBreakpoint> },
}

/// Inclusive segment start and integer cost per deviation unit. The first start is zero;
/// subsequent starts increase, bounded nonnegative slopes may vary, and segment costs accumulate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PenaltyBreakpoint {
    pub from_deviation: u32,
    pub slope: u32,
}
