use super::{DateRange, Weekday, deserialize_present};
use crate::ids::{AssignmentTypeId, LocationId, ShiftId, TeamId};
use eutheto_types::PersonId;
use serde::{Deserialize, Serialize};

/// Tags match exactly. Alternatives within `anyTags` are disjunctive; `allTags` are conjunctive.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PersonSelection {
    All {},
    Selected {
        person_ids: Vec<PersonId>,
    },
    Filter {
        all_tags: Vec<String>,
        any_tags: Vec<String>,
    },
}

/// Different filters intersect; identifiers within a filter are alternatives.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub people: PersonSelection,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub team_ids: Option<Vec<TeamId>>,
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
    pub categories: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub weekdays: Option<Vec<Weekday>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub location_ids: Option<Vec<LocationId>>,
}

/// The sole population definition for a workload policy, without shift filters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerGroup {
    pub people: PersonSelection,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub team_ids: Option<Vec<TeamId>>,
}

/// Coverage dates select local shift-start dates, independently of reporting attribution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShiftScope {
    All {},
    Selected {
        shift_ids: Vec<ShiftId>,
    },
    Filter {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        assignment_type_ids: Option<Vec<AssignmentTypeId>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        start_date_range: Option<DateRange>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        location_ids: Option<Vec<LocationId>>,
    },
}
