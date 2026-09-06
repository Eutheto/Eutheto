use super::{
    AssignmentLock, AssignmentType, Availability, BaseSchedule, CoverageRequirement, Location,
    Person, Qualification, ShiftInstance, ShiftTemplate, Team, WorkCalendar, WorkforcePreference,
    WorkforceRule, WorkforceScorePolicy, WorkloadBucket,
};
use eutheto_types::{AssignmentId, EntityId, RuleId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Typed body of Workforce internal schema v1. The version lives in the host's
/// domain-pack reference; settings and nonsemantic extensions remain host fields.
/// Decoding these records is not a substitute for bounded document/reference validation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkforceDomainV1 {
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub entities: BTreeMap<EntityId, WorkforceEntity>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub rules: BTreeMap<RuleId, WorkforceRule>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub preferences: BTreeMap<RuleId, WorkforcePreference>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub locked_assignments: BTreeMap<AssignmentId, AssignmentLock>,
}

/// Each record's identity must equal its host entity-map key, across every kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkforceEntity {
    Person(Person),
    Qualification(Qualification),
    Team(Team),
    Location(Location),
    WorkloadBucket(WorkloadBucket),
    Calendar(WorkCalendar),
    AssignmentType(AssignmentType),
    ShiftTemplate(ShiftTemplate),
    ShiftInstance(ShiftInstance),
    Availability(Availability),
    CoverageRequirement(CoverageRequirement),
    BaseSchedule(BaseSchedule),
    ScorePolicy(WorkforceScorePolicy),
}

impl WorkforceEntity {
    #[must_use]
    pub const fn id(&self) -> EntityId {
        match self {
            Self::Person(record) => EntityId::from_uuid(record.id.as_uuid()),
            Self::Qualification(record) => record.id.as_entity_id(),
            Self::Team(record) => record.id.as_entity_id(),
            Self::Location(record) => record.id.as_entity_id(),
            Self::WorkloadBucket(record) => record.id.as_entity_id(),
            Self::Calendar(record) => record.id.as_entity_id(),
            Self::AssignmentType(record) => record.id.as_entity_id(),
            Self::ShiftTemplate(record) => record.id.as_entity_id(),
            Self::ShiftInstance(record) => record.id.as_entity_id(),
            Self::Availability(record) => record.id.as_entity_id(),
            Self::CoverageRequirement(record) => record.id.as_entity_id(),
            Self::BaseSchedule(record) => record.id,
            Self::ScorePolicy(record) => record.id,
        }
    }
}
