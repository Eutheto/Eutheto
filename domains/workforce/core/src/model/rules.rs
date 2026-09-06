use super::{QualificationMinimum, Scope, WorkWindow};
use crate::ids::{LocationId, ShiftId, WorkCalendarId, WorkloadBucketId};
use eutheto_types::{PersonId, RuleId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RequiredStrength {
    Required,
}

/// Complete stored rule vocabulary. Serialization does not confer backend support.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkforceRule {
    Eligibility {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
    },
    Availability {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
    },
    Coverage {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
    },
    NoOverlap {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        compatible_category_pairs: Vec<CategoryPair>,
    },
    MinimumRest(Box<MinimumRestRule>),
    MaximumHours {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        bucket_id: WorkloadBucketId,
        window: WorkWindow,
        maximum_minutes: u32,
    },
    MaximumConsecutive {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        mode: ConsecutiveMode,
        maximum: u32,
    },
    MaximumAssignmentCount {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        calendar_id: WorkCalendarId,
        maximum: u32,
    },
    RequiredSkillMix {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        qualification_minimums: Vec<QualificationMinimum>,
    },
    FixedAssignment {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        person_id: PersonId,
        shift_id: ShiftId,
    },
    MutualAssignmentRestriction {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        person_ids: Vec<PersonId>,
        mode: PairingMode,
    },
    TransitionTime {
        id: RuleId,
        active: bool,
        strength: RequiredStrength,
        scope: Scope,
        location_ids: Vec<LocationId>,
    },
}

impl WorkforceRule {
    /// Returns shared identity, activation, and scope without copying rule parameters.
    #[must_use]
    pub const fn header(&self) -> (RuleId, bool, &Scope) {
        match self {
            Self::Eligibility {
                id, active, scope, ..
            }
            | Self::Availability {
                id, active, scope, ..
            }
            | Self::Coverage {
                id, active, scope, ..
            }
            | Self::NoOverlap {
                id, active, scope, ..
            }
            | Self::MaximumHours {
                id, active, scope, ..
            }
            | Self::MaximumConsecutive {
                id, active, scope, ..
            }
            | Self::MaximumAssignmentCount {
                id, active, scope, ..
            }
            | Self::RequiredSkillMix {
                id, active, scope, ..
            }
            | Self::FixedAssignment {
                id, active, scope, ..
            }
            | Self::MutualAssignmentRestriction {
                id, active, scope, ..
            }
            | Self::TransitionTime {
                id, active, scope, ..
            } => (*id, *active, scope),
            Self::MinimumRest(rule) => (rule.id, rule.active, &rule.scope),
        }
    }
}

/// Boxed in the rule enum so every other rule does not reserve space for three scopes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MinimumRestRule {
    pub id: RuleId,
    pub active: bool,
    pub strength: RequiredStrength,
    pub scope: Scope,
    pub after_scope: Scope,
    pub before_scope: Scope,
    pub minimum_minutes: u32,
}

/// Symmetric compatibility stored in canonical lexical category order.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoryPair {
    pub first_category: String,
    pub second_category: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConsecutiveMode {
    WorkedDays {},
    /// Matching chronological assignments stay in one run when gap is at most this duration.
    Assignments {
        break_minutes: u32,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingMode {
    SameShift,
    OverlappingShifts,
}
