//! Bounded native imports of availability and time-off records.

mod review;

use crate::{
    ids::AvailabilityId,
    people_csv::{BlankPolicy, CsvDialect, CsvSource, PeopleImportDisposition},
};
use eutheto_domain_api::DomainBatchCommand;
use eutheto_types::{EntityId, ResolvedLocalTime, Revision, ScenarioId, ValidationIssue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub use review::{
    decode_availability_csv_review, preview_availability_csv, rebuild_availability_csv_review,
};

pub const MAX_AVAILABILITY_CSV_REPORT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityCsvField {
    PersonId,
    AvailabilityKind,
    StartsAt,
    EndsAt,
    WeeklyWindows,
    EffectiveStartDate,
    EffectiveEndDateExclusive,
    AssignmentTypeIds,
    LocationIds,
    Source,
    Note,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityCsvColumn {
    pub index: u16,
    pub field: AvailabilityCsvField,
    pub blank: BlankPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityCsvMapping {
    pub dialect: CsvDialect,
    pub has_header: bool,
    pub expected_columns: u16,
    pub identity_column: Option<u16>,
    pub columns: Vec<AvailabilityCsvColumn>,
    pub person_mappings: BTreeMap<String, EntityId>,
    pub assignment_type_mappings: BTreeMap<String, EntityId>,
    pub location_mappings: BTreeMap<String, EntityId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AvailabilityCsvDecision {
    Add { availability_id: AvailabilityId },
    Update { availability_id: AvailabilityId },
    Skip {},
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityCsvRowDecision {
    pub record: u32,
    pub decision: AvailabilityCsvDecision,
}

/// Source identity interpreted independently of a selected row decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AvailabilityCsvIdentity {
    Absent {},
    Supplied { availability_id: AvailabilityId },
    Invalid {},
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityCsvRowStatus {
    Added,
    Updated,
    Unchanged,
    Skipped,
    Blocked,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityCsvRowIssue {
    ColumnCount,
    InvalidIdentity,
    ReservedIdentity,
    UnresolvedIdentity,
    DuplicateTarget,
    InvalidCell,
    InvalidReference,
    InvalidLocalTime,
    UnresolvableLocalTime,
    NonpositiveInterval,
    ConflictingTimeWindow,
    MissingRequiredField,
    InvalidAvailability,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityImportRow {
    pub record: u32,
    pub status: AvailabilityCsvRowStatus,
    pub identity: AvailabilityCsvIdentity,
    pub availability_id: Option<AvailabilityId>,
    /// Bounded validated raw stored record, including native-accepted spelling.
    pub before: Option<Value>,
    /// Changed/new record only; Unchanged retains its meaning in `before`.
    pub after: Option<Value>,
    /// Native facts for mapped local endpoints, also retained for equivalent edits.
    pub resolved_starts_at: Option<ResolvedLocalTime>,
    pub resolved_ends_at: Option<ResolvedLocalTime>,
    pub issue: Option<AvailabilityCsvRowIssue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityCsvExcludedRow {
    pub record: u32,
    pub skipped: bool,
    pub issue: Option<AvailabilityCsvRowIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityCsvReview {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    /// Binds the document and native-owned additional wrapper identity census.
    pub scenario_blake3: String,
    pub source: CsvSource,
    pub parser_version: String,
    pub limits_version: u32,
    pub mapping: AvailabilityCsvMapping,
    pub decisions: Vec<AvailabilityCsvRowDecision>,
    pub rows_blake3: String,
    pub changes_blake3: String,
    pub excluded_blake3: String,
    pub validation_blake3: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityImportPreview {
    pub schema_version: u32,
    pub disposition: PeopleImportDisposition,
    pub source: CsvSource,
    pub rows: Vec<AvailabilityImportRow>,
    pub excluded_rows: Vec<AvailabilityCsvExcludedRow>,
    pub validation_issues: Vec<ValidationIssue>,
    pub batch: Option<DomainBatchCommand>,
    pub review: Option<AvailabilityCsvReview>,
    pub approval_digest: Option<String>,
}
