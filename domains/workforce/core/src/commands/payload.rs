use crate::model::{AssignmentLock, WorkforceEntity, WorkforcePreference, WorkforceRule};
use eutheto_types::{AssignmentId, EntityId, RuleId};
use serde::{Deserialize, Serialize};

pub use crate::generated_workforce_pack_contract::{
    ADD_ENTITY, ADD_LOCK, ADD_PREFERENCE, ADD_RULE, REMOVE_ENTITY, REMOVE_LOCK, REMOVE_PREFERENCE,
    REMOVE_RULE, UPDATE_ENTITY, UPDATE_LOCK, UPDATE_PREFERENCE, UPDATE_RULE,
};

/// Complete typed record for an add or update; its identity selects the target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityPayload {
    pub entity: WorkforceEntity,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityTarget {
    pub entity_id: EntityId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulePayload {
    pub rule: WorkforceRule,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleTarget {
    pub rule_id: RuleId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencePayload {
    pub preference: WorkforcePreference,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferenceTarget {
    pub preference_id: RuleId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockPayload {
    pub lock: AssignmentLock,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockTarget {
    pub assignment_id: AssignmentId,
}
