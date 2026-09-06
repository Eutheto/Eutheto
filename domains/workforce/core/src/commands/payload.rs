use crate::ids::{ShiftId, ShiftTemplateId};
use crate::model::{
    AssignmentLock, OccurrenceIdentity, WorkforceEntity, WorkforcePreference, WorkforceRule,
};
use eutheto_types::{AssignmentId, EntityId, RuleId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use crate::generated_workforce_pack_contract::{
    ADD_ENTITY, ADD_LOCK, ADD_OCCURRENCE_IDENTITIES, ADD_PREFERENCE, ADD_RULE, DETACH_SHIFT,
    REATTACH_SHIFT, REMOVE_ENTITY, REMOVE_LOCK, REMOVE_OCCURRENCE_IDENTITIES, REMOVE_PREFERENCE,
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

/// Raw input maps are retained separately by mutation code to preserve UUID spelling.
/// Reattach requires exactly one definition; bulk addition uses the existing ledger caps.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateOccurrenceIdentities {
    pub template_id: ShiftTemplateId,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    pub occurrence_identities: BTreeMap<ShiftId, OccurrenceIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddOccurrenceIdentitiesPayload {
    pub templates: Vec<TemplateOccurrenceIdentities>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateOccurrenceTargets {
    pub template_id: ShiftTemplateId,
    pub shift_ids: Vec<ShiftId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveOccurrenceIdentitiesPayload {
    pub templates: Vec<TemplateOccurrenceTargets>,
}

/// The generated schema and runtime both restrict `instance` to the shift-instance variant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DetachShiftPayload {
    pub template_id: ShiftTemplateId,
    pub instance: WorkforceEntity,
}
