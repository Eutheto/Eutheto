use super::{
    ActiveRange, DateRange, Recurrence, ReportingAttribution, ShiftOrigin, ShiftScope, ShiftTiming,
    TimeWindow, WindowMembership, deserialize_present,
};
use crate::ids::{
    AssignmentTypeId, AvailabilityId, CoverageRequirementId, LocationId, QualificationId, ShiftId,
    ShiftTemplateId, TeamId, WorkCalendarId, WorkloadBucketId,
};
use eutheto_types::{
    AssignmentId, EntityId, PersonId, ResolvedLocalTime, Revision, Rfc3339Timestamp, SolutionId,
};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Positive bounded rational participation weight, without a locale-sensitive decimal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadWeight {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadTarget {
    pub bucket_id: WorkloadBucketId,
    pub calendar_id: WorkCalendarId,
    pub membership: WindowMembership,
    pub target: u32,
}

/// Display-only metadata. Avatars are bounded initials, not files, URLs, or embedded images.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonDisplay {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub color: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub avatar_initials: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Person {
    pub id: PersonId,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub external_id: Option<String>,
    pub active_range: ActiveRange,
    pub qualification_grants: Vec<QualificationGrant>,
    pub eligible_assignment_type_ids: Vec<AssignmentTypeId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub home_location_id: Option<LocationId>,
    pub workload_weight: WorkloadWeight,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub workload_target: Option<WorkloadTarget>,
    pub tags: Vec<String>,
    pub team_ids: Vec<TeamId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub display: Option<PersonDisplay>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Qualification {
    pub id: QualificationId,
    pub name: String,
    pub description: String,
}

/// A grant covers an inclusive start and exclusive expiry; absent endpoints are unbounded.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualificationGrant {
    pub qualification_id: QualificationId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub effective_from: Option<Rfc3339Timestamp>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub expires_at: Option<Rfc3339Timestamp>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Location {
    pub id: LocationId,
    pub name: String,
    /// Unique destinations; supplied order is preserved but has no transition meaning.
    pub transitions: Vec<LocationTransition>,
}

/// Directed destination reference; a UUID-keyed numeric map would not be safely remappable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocationTransition {
    pub location_id: LocationId,
    pub minutes: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkloadMeasurement {
    AssignmentCount,
    ElapsedMinutes,
    ScheduledMinutes,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlappingContribution {
    Sum,
    Union,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadBucket {
    pub id: WorkloadBucketId,
    pub name: String,
    pub measurement: WorkloadMeasurement,
    pub overlapping_contribution: OverlappingContribution,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentType {
    pub id: AssignmentTypeId,
    pub name: String,
    pub category: String,
    pub time_behavior: TimeBehavior,
    pub default_duration_minutes: u32,
    pub qualifications: QualificationExpression,
    pub location_behavior: LocationBehavior,
    pub workload_bucket_ids: Vec<WorkloadBucketId>,
}

/// Creation defaults only; explicit template timing and stored endpoints remain authoritative.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TimeBehavior {
    LocalWallClock,
    Elapsed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LocationBehavior {
    None {},
    Optional {},
    Required {},
    Fixed { location_id: LocationId },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum QualificationExpression {
    Unconstrained {},
    Matches(QualificationMatch),
}

/// Both lists are sets. Every all-reference and at least one nonempty any-reference must hold.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualificationMatch {
    pub all_qualification_ids: Vec<QualificationId>,
    pub any_qualification_ids: Vec<QualificationId>,
}

/// Independent minimum, not a distinct staffed position; dual-qualified people may count in each.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualificationMinimum {
    pub qualifications: QualificationMatch,
    pub minimum: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Coverage {
    Exact {
        count: u16,
        qualification_minimums: Vec<QualificationMinimum>,
    },
    AtLeast {
        minimum: u16,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        preferred_count: Option<u16>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        maximum_count: Option<u16>,
        qualification_minimums: Vec<QualificationMinimum>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftTemplate {
    pub id: ShiftTemplateId,
    pub name: String,
    pub assignment_type_id: AssignmentTypeId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub location_id: Option<LocationId>,
    pub recurrence: Recurrence,
    pub timing: ShiftTiming,
    pub coverage: Coverage,
    pub tags: Vec<String>,
    pub reporting_attribution: ReportingAttribution,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub occurrence_identities: BTreeMap<ShiftId, OccurrenceIdentity>,
}

/// Owns only stable identity and local start date, not a generated interval cache.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OccurrenceIdentity {
    pub id: ShiftId,
    pub local_start_date: Date,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftInstance {
    pub id: ShiftId,
    pub assignment_type_id: AssignmentTypeId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub location_id: Option<LocationId>,
    pub starts_at: ResolvedLocalTime,
    pub ends_at: ResolvedLocalTime,
    pub coverage: Coverage,
    pub tags: Vec<String>,
    pub reporting_attribution: ReportingAttribution,
    pub origin: ShiftOrigin,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageRequirement {
    pub id: CoverageRequirementId,
    pub active: bool,
    pub scope: ShiftScope,
    pub coverage: Coverage,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityKind {
    Unavailable,
    AvailableOnly,
    ApprovedTimeOff,
    RequestedTimeOff,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Availability {
    pub id: AvailabilityId,
    pub person_id: PersonId,
    pub availability_kind: AvailabilityKind,
    pub time_window: TimeWindow,
    pub effective_range: DateRange,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub assignment_type_ids: Option<Vec<AssignmentTypeId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub location_ids: Option<Vec<LocationId>>,
    pub source: String,
    pub note: String,
}

/// Data for future repair; this record cannot confer accepted-result authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BaseSchedule {
    pub id: EntityId,
    pub source_solution_id: SolutionId,
    pub source_revision: Revision,
    pub assignments: Vec<AssignmentPair>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentPair {
    pub person_id: PersonId,
    pub shift_id: ShiftId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentLock {
    pub id: AssignmentId,
    pub person_id: PersonId,
    pub shift_id: ShiftId,
    pub state: LockState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LockState {
    Hard {},
    Soft { stability_weight: u32 },
    Unlocked {},
}
