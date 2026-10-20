//! Bounded imports of stored manual/detached shifts, never generated occurrences.

mod review;

use crate::{
    ids::ShiftId,
    people_csv::{BlankPolicy, CsvDialect, CsvSource, PeopleImportDisposition},
};
use eutheto_domain_api::DomainBatchCommand;
use eutheto_types::{EntityId, Revision, ScenarioId, ValidationIssue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub use review::{decode_shift_csv_review, preview_shift_csv, rebuild_shift_csv_review};

pub const MAX_SHIFT_CSV_REPORT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShiftCsvField {
    AssignmentTypeId,
    LocationId,
    StartsAt,
    EndsAt,
    Coverage,
    Tags,
    ReportingAttribution,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftCsvColumn {
    pub index: u16,
    pub field: ShiftCsvField,
    pub blank: BlankPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftCsvMapping {
    pub dialect: CsvDialect,
    pub has_header: bool,
    pub expected_columns: u16,
    pub identity_column: Option<u16>,
    pub columns: Vec<ShiftCsvColumn>,
    pub reference_mappings: BTreeMap<String, EntityId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShiftCsvDecision {
    Add { shift_id: ShiftId },
    Update { shift_id: ShiftId },
    Skip {},
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftCsvRowDecision {
    pub record: u32,
    pub decision: ShiftCsvDecision,
}

/// Source identity interpreted natively, independently of a selected row decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShiftCsvIdentity {
    Absent {},
    Supplied { shift_id: ShiftId },
    Invalid {},
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShiftCsvRowStatus {
    Added,
    Updated,
    Unchanged,
    Skipped,
    Blocked,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShiftCsvRowIssue {
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
    MissingRequiredField,
    InvalidShift,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftImportRow {
    pub record: u32,
    pub status: ShiftCsvRowStatus,
    pub identity: ShiftCsvIdentity,
    pub shift_id: Option<ShiftId>,
    /// Bounded validated raw stored record; preserves native-accepted spelling.
    pub before: Option<Value>,
    /// Changed/new record only; Unchanged retains its meaning in `before`.
    pub after: Option<Value>,
    pub issue: Option<ShiftCsvRowIssue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftCsvExcludedRow {
    pub record: u32,
    pub skipped: bool,
    pub issue: Option<ShiftCsvRowIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftCsvReview {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    /// Binds the document and native-owned additional wrapper identity census.
    pub scenario_blake3: String,
    pub source: CsvSource,
    pub parser_version: String,
    pub limits_version: u32,
    pub mapping: ShiftCsvMapping,
    pub decisions: Vec<ShiftCsvRowDecision>,
    pub rows_blake3: String,
    pub changes_blake3: String,
    pub excluded_blake3: String,
    pub validation_blake3: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShiftImportPreview {
    pub schema_version: u32,
    pub disposition: PeopleImportDisposition,
    pub source: CsvSource,
    pub rows: Vec<ShiftImportRow>,
    pub excluded_rows: Vec<ShiftCsvExcludedRow>,
    pub validation_issues: Vec<ValidationIssue>,
    pub batch: Option<DomainBatchCommand>,
    pub review: Option<ShiftCsvReview>,
    pub approval_digest: Option<String>,
}
