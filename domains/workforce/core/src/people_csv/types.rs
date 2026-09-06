use crate::ids::{AssignmentTypeId, LocationId, TeamId};
use crate::model::{
    ActiveRange, PersonDisplay, QualificationGrant, WorkloadTarget, WorkloadWeight,
    deserialize_present,
};
use eutheto_types::{EntityId, PersonId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

pub const CSV_LIMITS_VERSION: u32 = 1;
pub const CSV_PARSER_VERSION: &str = "csv-core-0.1.13";
pub const MAX_CSV_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CSV_DATA_RECORDS: u32 = 10_000;
pub const MAX_CSV_LOGICAL_RECORDS: u32 = MAX_CSV_DATA_RECORDS + 1;
pub const MAX_CSV_COLUMNS: usize = 64;
pub const MAX_CSV_CELL_BYTES: usize = 16 * 1024;
pub const MAX_CSV_RECORD_BYTES: usize = 256 * 1024;
pub const MAX_CSV_MAPPING_BYTES: usize = 1024 * 1024;
pub const MAX_CSV_DECISIONS: usize = 10_000;
pub const MAX_CSV_DECISION_BYTES: usize = 1024 * 1024;
pub const MAX_CSV_REVIEW_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CSV_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CSV_REJECTED_ROWS: usize = 200;
pub const MAX_CSV_REJECTED_REPORT_BYTES: usize = 64 * 1024;
pub const MAX_CSV_DETECTION_BYTES: usize = 64 * 1024;
pub const MAX_CSV_SAMPLE_RECORDS: usize = 2;
pub const MAX_CSV_SAMPLE_CELL_BYTES: usize = 64;

/// A reviewed delimiter choice, never an automatically selected fallback.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CsvDialect {
    Comma,
    Semicolon,
    Tab,
}

impl CsvDialect {
    pub(crate) const fn delimiter(self) -> u8 {
        match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
        }
    }
}

/// Applies only to an exactly empty decoded cell, never trimmed whitespace.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BlankPolicy {
    Preserve,
    Clear,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonField {
    Name,
    ExternalId,
    ActiveRange,
    QualificationGrants,
    EligibleAssignmentTypeIds,
    HomeLocationId,
    WorkloadWeight,
    WorkloadTarget,
    Tags,
    TeamIds,
}

impl PersonField {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::ExternalId => "externalId",
            Self::ActiveRange => "activeRange",
            Self::QualificationGrants => "qualificationGrants",
            Self::EligibleAssignmentTypeIds => "eligibleAssignmentTypeIds",
            Self::HomeLocationId => "homeLocationId",
            Self::WorkloadWeight => "workloadWeight",
            Self::WorkloadTarget => "workloadTarget",
            Self::Tags => "tags",
            Self::TeamIds => "teamIds",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColumnMapping {
    /// Zero-based column position. Header names are presentation only.
    pub index: u8,
    pub field: PersonField,
    pub blank: BlankPolicy,
}

/// Explicit creation policy for fields absent from a row. No domain defaults are inferred.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewPersonDefaults {
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
pub struct PeopleCsvMapping {
    pub dialect: CsvDialect,
    pub has_header: bool,
    pub expected_columns: u8,
    pub columns: Vec<ColumnMapping>,
    pub new_person_defaults: NewPersonDefaults,
    /// Exact user-declared tokens only. UUID-shaped keys cannot shadow direct stable IDs.
    pub reference_tokens: BTreeMap<String, EntityId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum IdentityDecision {
    Add { person_id: PersonId },
    Update { person_id: PersonId },
    Skip,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowDecision {
    /// One-based logical record number, including the header when present.
    pub record: u32,
    pub decision: IdentityDecision,
}

/// Stable bounded failure codes; never includes submitted cell or operating-system error text.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CsvErrorCode {
    Io,
    Cancelled,
    UnsupportedEncoding,
    InvalidUtf8,
    BinaryControl,
    SourceByteLimit,
    CellLimit,
    RecordByteLimit,
    ColumnLimit,
    LogicalRecordLimit,
    ParserNoProgress,
    DetectionLimit,
    InvalidMapping,
    MappingLimit,
    HeaderMismatch,
    DataRecordLimit,
    InvalidDecision,
    DecisionLimit,
    MutationLimit,
    RejectedReportLimit,
    PreviewLimit,
    ReviewLimit,
    UnsupportedVersion,
    InvalidCurrentDocument,
    InvalidReview,
    StaleReview,
    InvalidBatch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsvError {
    pub code: CsvErrorCode,
    /// Absent for source-global errors that cannot safely identify a logical record.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub record: Option<u32>,
}

impl CsvError {
    pub(crate) const fn source(code: CsvErrorCode) -> Self {
        Self { code, record: None }
    }

    pub(crate) const fn at(code: CsvErrorCode, record: u32) -> Self {
        Self {
            code,
            record: Some(record),
        }
    }
}

impl fmt::Display for CsvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "people CSV rejected: {:?}", self.code)?;
        if let Some(record) = self.record {
            write!(formatter, " at logical record {record}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CsvError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsvSource {
    /// Exact original bytes, including the initial BOM and ignored physical empty lines.
    pub raw_bytes: u64,
    pub blake3: String,
    pub logical_records: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsvSampleCell {
    pub text: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CsvSampleRecord {
    pub record: u32,
    pub cells: Vec<CsvSampleCell>,
}

/// Candidate-local decoded limits do not rule out a different explicit dialect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CsvDialectDetection {
    Candidate {
        dialect: CsvDialect,
        source: CsvSource,
        consistent_columns: Option<u8>,
        samples: Vec<CsvSampleRecord>,
    },
    Rejected {
        dialect: CsvDialect,
        error: CsvError,
    },
}

/// Bounded suggestions only; no chosen header, identity or mutation authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvDetection {
    pub schema_version: u32,
    pub dialects: Vec<CsvDialectDetection>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowRejectionCode {
    ColumnCount,
    InvalidCell,
    InvalidReference,
    MissingName,
    InvalidPerson,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RejectedRow {
    pub record: u32,
    pub code: RowRejectionCode,
}

pub(crate) struct CsvRecord<'a> {
    pub number: u32,
    pub cells: &'a [&'a str],
}

/// Raw mapped values survive until generated candidate-schema validation.
pub(crate) enum FieldEdit {
    Set(Value),
    Remove,
}

pub(crate) struct PersonPatch {
    pub fields: BTreeMap<PersonField, FieldEdit>,
}
