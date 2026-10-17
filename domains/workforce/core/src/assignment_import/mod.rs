//! Bounded assignment CSV input for explicitly unverified starting-plan review.

mod review;
mod source;

use crate::assignments_csv::AssignmentsCsvError;
use crate::people_csv::{
    CsvDialect, CsvDialectDetection, CsvError, CsvSource, PeopleImportDisposition,
};
use crate::{ids::ShiftId, model::AssignmentPair};
use eutheto_domain_api::DomainBatchCommand;
use eutheto_types::{EntityId, PersonId, Revision, ScenarioId, ValidationIssue};
use serde::{Deserialize, Serialize};
use std::fmt;

pub use review::{
    decode_assignment_csv_review, preview_assignment_csv, rebuild_assignment_csv_review,
};
pub use source::{detect_assignment_csv, sample_assignment_csv_record};

/// Parsing choice only; neither format establishes assignment acceptance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "format", rename_all = "camelCase", deny_unknown_fields)]
pub enum AssignmentCsvReadFormat {
    CanonicalV1,
    Mapped { dialect: CsvDialect },
}

/// Native format facts. Canonical framing is never reinterpreted as mapped CSV.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "format",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssignmentCsvDetection {
    CanonicalV1 {
        schema_version: u32,
        source: CsvSource,
    },
    Mapped {
        schema_version: u32,
        dialects: Vec<CsvDialectDetection>,
    },
}

/// Closed failures contain no submitted cells, paths or operating-system error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentImportError {
    Csv(CsvError),
    Canonical(AssignmentsCsvError),
    FormatMismatch,
}

impl From<CsvError> for AssignmentImportError {
    fn from(error: CsvError) -> Self {
        Self::Csv(error)
    }
}

impl From<AssignmentsCsvError> for AssignmentImportError {
    fn from(error: AssignmentsCsvError) -> Self {
        Self::Canonical(error)
    }
}

impl fmt::Display for AssignmentImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "assignment CSV rejected: {self:?}")
    }
}

impl std::error::Error for AssignmentImportError {}

pub const MAX_ASSIGNMENT_CSV_REPORT_BYTES: usize = 1024 * 1024;

/// Mapped columns and endpoint-specific aliases never override a direct UUID.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "format",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssignmentCsvMapping {
    CanonicalV1,
    Mapped {
        dialect: CsvDialect,
        has_header: bool,
        expected_columns: u8,
        person_column: u8,
        shift_column: u8,
        person_mappings: std::collections::BTreeMap<String, PersonId>,
        shift_mappings: std::collections::BTreeMap<String, ShiftId>,
    },
}

impl AssignmentCsvMapping {
    fn read_format(&self) -> AssignmentCsvReadFormat {
        match self {
            Self::CanonicalV1 => AssignmentCsvReadFormat::CanonicalV1,
            Self::Mapped { dialect, .. } => AssignmentCsvReadFormat::Mapped { dialect: *dialect },
        }
    }

    fn framing_records(&self) -> u32 {
        match self {
            Self::CanonicalV1 => 2,
            Self::Mapped { has_header, .. } => u32::from(*has_header),
        }
    }
}

/// A complete create or replacement, never implicit removal or accepted-base adoption.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum AssignmentImportTarget {
    Create { id: EntityId, name: String },
    Replace { id: EntityId, name: String },
}

impl AssignmentImportTarget {
    fn id(&self) -> EntityId {
        match self {
            Self::Create { id, .. } | Self::Replace { id, .. } => *id,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Create { name, .. } | Self::Replace { name, .. } => name,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentRowStatus {
    Included,
    Skipped,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentRowIssue {
    ColumnCount,
    InvalidToken,
    UnresolvedPerson,
    UnresolvedShift,
    DuplicatePair,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentImportRow {
    pub record: u32,
    pub status: AssignmentRowStatus,
    pub person_id: Option<PersonId>,
    pub shift_id: Option<ShiftId>,
    pub issue: Option<AssignmentRowIssue>,
}

/// Every omitted row is retained, including explicitly skipped valid rows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentExcludedRow {
    pub record: u32,
    pub skipped: bool,
    pub issue: Option<AssignmentRowIssue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentImportChanges {
    pub previous_name: Option<String>,
    pub before_pair_count: usize,
    pub after_pair_count: usize,
    pub added_pairs: Vec<AssignmentPair>,
    pub removed_pairs: Vec<AssignmentPair>,
    pub provenance_changed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentCsvReview {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub scenario_blake3: String,
    pub source: CsvSource,
    pub parser_version: String,
    pub limits_version: u32,
    pub mapping: AssignmentCsvMapping,
    pub target: AssignmentImportTarget,
    pub skipped_records: Vec<u32>,
    pub allow_empty: bool,
    pub rows_blake3: String,
    pub changes_blake3: String,
    pub excluded_blake3: String,
    pub validation_blake3: String,
}

/// A proposal only. Native custody separately owns approval and durable apply.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentImportPreview {
    pub schema_version: u32,
    pub disposition: PeopleImportDisposition,
    pub source: CsvSource,
    pub rows: Vec<AssignmentImportRow>,
    pub excluded_rows: Vec<AssignmentExcludedRow>,
    pub changes: Option<AssignmentImportChanges>,
    pub validation_issues: Vec<ValidationIssue>,
    pub batch: Option<DomainBatchCommand>,
    pub review: Option<AssignmentCsvReview>,
    pub approval_digest: Option<String>,
}
