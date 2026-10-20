//! Bounded sparse edits to configured eligibility, never computed assignment feasibility.

mod matrix;
mod review;

use crate::{
    ids::AssignmentTypeId,
    people_csv::{CsvDialect, CsvSampleRecord, CsvSource, PeopleImportDisposition},
};
use eutheto_domain_api::DomainBatchCommand;
use eutheto_types::{PersonId, Revision, ScenarioId, ValidationIssue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use matrix::{
    decode_eligibility_matrix_review, detect_eligibility_matrix, preview_eligibility_matrix,
    rebuild_eligibility_matrix_review, sample_eligibility_matrix_record,
};

pub use review::{
    decode_eligibility_csv_review, preview_eligibility_csv, rebuild_eligibility_csv_review,
};

pub const MAX_ELIGIBILITY_CSV_REPORT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityCsvMapping {
    pub dialect: CsvDialect,
    pub has_header: bool,
    pub expected_columns: u16,
    pub person_column: u16,
    pub assignment_type_column: u16,
    pub eligible_column: u16,
    pub person_mappings: BTreeMap<String, PersonId>,
    pub assignment_type_mappings: BTreeMap<String, AssignmentTypeId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EligibilityRowStatus {
    Changed,
    Unchanged,
    Skipped,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EligibilityRowIssue {
    ColumnCount,
    InvalidToken,
    UnresolvedPerson,
    UnresolvedAssignmentType,
    InvalidMembership,
    DuplicatePair,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityImportRow {
    pub record: u32,
    pub status: EligibilityRowStatus,
    pub person_id: Option<PersonId>,
    pub assignment_type_id: Option<AssignmentTypeId>,
    pub before: Option<bool>,
    pub after: Option<bool>,
    pub issue: Option<EligibilityRowIssue>,
}

/// Complete omitted-row metadata, including explicit skips of otherwise valid rows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityExcludedRow {
    pub record: u32,
    pub skipped: bool,
    pub issue: Option<EligibilityRowIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityCsvReview {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub scenario_blake3: String,
    pub source: CsvSource,
    pub parser_version: String,
    pub limits_version: u32,
    pub mapping: EligibilityCsvMapping,
    pub skipped_records: Vec<u32>,
    pub rows_blake3: String,
    pub changes_blake3: String,
    pub excluded_blake3: String,
    pub validation_blake3: String,
}

/// Native custody separately owns approval and the ordinary atomic command.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityImportPreview {
    pub schema_version: u32,
    pub disposition: PeopleImportDisposition,
    pub source: CsvSource,
    pub rows: Vec<EligibilityImportRow>,
    pub excluded_rows: Vec<EligibilityExcludedRow>,
    pub validation_issues: Vec<ValidationIssue>,
    pub batch: Option<DomainBatchCommand>,
    pub review: Option<EligibilityCsvReview>,
    pub approval_digest: Option<String>,
}

pub const ELIGIBILITY_MATRIX_PARSER_VERSION: &str = "csv-core-0.1.13/eligibility-matrix-1";
pub const ELIGIBILITY_MATRIX_LIMITS_VERSION: u32 = 1;
pub const MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_ELIGIBILITY_MATRIX_CELLS: u32 = 10_000;
pub const MAX_ELIGIBILITY_MATRIX_REPORT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixMapping {
    pub person_mappings: BTreeMap<String, PersonId>,
    pub assignment_type_mappings: BTreeMap<String, AssignmentTypeId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixCellAddress {
    /// One-based logical record, including the header.
    pub record: u32,
    /// Zero-based column; column zero contains the person reference.
    pub column: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixDetection {
    pub schema_version: u32,
    pub source: CsvSource,
    pub columns: u16,
    pub cells: u32,
    pub samples: Vec<CsvSampleRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixCell {
    pub cell: EligibilityMatrixCellAddress,
    pub status: EligibilityRowStatus,
    pub person_id: Option<PersonId>,
    pub assignment_type_id: Option<AssignmentTypeId>,
    pub before: Option<bool>,
    pub after: Option<bool>,
    pub issue: Option<EligibilityRowIssue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixExcludedCell {
    pub cell: EligibilityMatrixCellAddress,
    pub skipped: bool,
    pub issue: Option<EligibilityRowIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixReview {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub scenario_blake3: String,
    pub source: CsvSource,
    pub parser_version: String,
    pub limits_version: u32,
    pub columns: u16,
    pub cells: u32,
    pub mapping: EligibilityMatrixMapping,
    pub skipped_cells: Vec<EligibilityMatrixCellAddress>,
    pub rows_blake3: String,
    pub changes_blake3: String,
    pub excluded_blake3: String,
    pub validation_blake3: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibilityMatrixPreview {
    pub schema_version: u32,
    pub disposition: PeopleImportDisposition,
    pub source: CsvSource,
    pub columns: u16,
    pub cells: u32,
    pub rows: Vec<EligibilityMatrixCell>,
    pub excluded_cells: Vec<EligibilityMatrixExcludedCell>,
    pub validation_issues: Vec<ValidationIssue>,
    pub batch: Option<DomainBatchCommand>,
    pub review: Option<EligibilityMatrixReview>,
    pub approval_digest: Option<String>,
}
