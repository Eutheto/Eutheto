use crate::{
    ids::{AssignmentTypeId, AvailabilityId},
    model::AssignmentPair,
    temporal::{TemporalError, TemporalIssue},
};
use eutheto_domain_api::{DomainPackError, DomainValidationReport};
use eutheto_domain_ir::RuleEvaluation;
use eutheto_planning_ir::{BoolVariable, ConstraintRecord, ProvenanceRecord};
use eutheto_types::{Rfc3339Timestamp, RuleId, ValidationIssue, ValidationSeverity};
use std::fmt;

/// An operation-local half-open instant interval, not another stored time format.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InstantInterval {
    pub start: Rfc3339Timestamp,
    pub end: Rfc3339Timestamp,
}

impl InstantInterval {
    #[must_use]
    pub fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start < end).then_some(Self { start, end })
    }
}

/// One failed original-source predicate. Expressions remain in the hash-bound document.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RejectionCause {
    OutsideActiveRange {
        allowed: InstantInterval,
        outside: InstantInterval,
    },
    AssignmentTypeNotAllowed {
        assignment_type_id: AssignmentTypeId,
    },
    QualificationExpression {
        assignment_type_id: AssignmentTypeId,
    },
    Unavailable {
        availability_id: AvailabilityId,
        overlap: InstantInterval,
    },
    OutsideAvailableOnly {
        availability_id: AvailabilityId,
        uncovered: InstantInterval,
    },
    ApprovedTimeOff {
        availability_id: AvailabilityId,
        overlap: InstantInterval,
    },
}

/// Distinct rule/fact owners retain distinct rejection causes for the same pair.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PairRejection {
    pub pair: AssignmentPair,
    pub binding_id: RuleId,
    pub cause: RejectionCause,
}

/// Sorted, unique, disjoint lists partitioning the complete original required-obligation set.
/// A contribution with remaining obligations is not a complete pack verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiredRulePartition {
    pub handled: Vec<RuleId>,
    pub remaining: Vec<RuleId>,
}

/// Exact counts for this rule contribution, not backend timing or a complete planning model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssignmentModelEstimate {
    pub inspected_pairs: u64,
    pub after_activity_pruning: u64,
    pub after_assignment_type_pruning: u64,
    pub after_qualification_pruning: u64,
    pub after_availability_pruning: u64,
    pub rejection_facts: u64,
    pub variables: u64,
    pub constraints: u64,
    pub provenance_records: u64,
    pub references: u64,
}

/// Bounded candidate and readiness analysis without solve or acceptance authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentAnalysis {
    pub source_document_hash: String,
    pub candidates: Vec<AssignmentPair>,
    pub rejections: Vec<PairRejection>,
    pub estimate: AssignmentModelEstimate,
    pub validation: DomainValidationReport,
    pub obligations: RequiredRulePartition,
}

/// Explicit pair-to-Boolean association; callers never parse identifiers or infer identity by index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentVariable {
    pub pair: AssignmentPair,
    pub variable: BoolVariable,
}

/// Mathematics for the four assignment rule families, not a complete PlanningProblem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentRuleCompilation {
    pub source_document_hash: String,
    pub variables: Vec<AssignmentVariable>,
    pub constraints: Vec<ConstraintRecord>,
    pub provenance: Vec<ProvenanceRecord>,
    pub rejections: Vec<PairRejection>,
    pub estimate: AssignmentModelEstimate,
    pub validation: DomainValidationReport,
    pub obligations: RequiredRulePartition,
}

/// One aggregate evaluation per handled binding, not a VerificationReport or score.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentRuleEvaluation {
    pub source_document_hash: String,
    pub evaluations: Vec<RuleEvaluation>,
    pub obligations: RequiredRulePartition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionIssueKind {
    DuplicatePair,
    MissingPerson,
    WrongKindPerson,
    UnresolvedShift,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentConstructionIssue {
    ArithmeticOverflow,
    InvalidIdentifier,
    InvalidRecord,
    IdentityCollision,
    SourceHash,
}

/// Fixed finite-work/resource failures are distinct from an observed wall-clock deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentRuleLimit {
    InspectedPairs,
    WorkSteps,
    ExpandedIntervals,
    SelectedPairs,
    Variables,
    Constraints,
    ProvenanceRecords,
    Records,
    References,
    Bytes,
    PerRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignmentRuleError {
    InvalidDocument(DomainPackError),
    Temporal(TemporalIssue),
    InvalidSelection {
        pair: AssignmentPair,
        kind: SelectionIssueKind,
    },
    LimitExceeded(AssignmentRuleLimit),
    InvalidConstruction(AssignmentConstructionIssue),
    Cancelled,
}

impl AssignmentRuleError {
    /// Stable safe classification, with no submitted names, notes or values.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDocument(_) => "official.workforce.invalid_document",
            Self::Temporal(_) => "official.workforce.temporal_review",
            Self::Cancelled => "official.workforce.cancelled",
            Self::InvalidSelection { kind, .. } => match kind {
                SelectionIssueKind::DuplicatePair => "official.workforce.selection.duplicate_pair",
                SelectionIssueKind::MissingPerson => "official.workforce.selection.missing_person",
                SelectionIssueKind::WrongKindPerson => {
                    "official.workforce.selection.wrong_kind_person"
                }
                SelectionIssueKind::UnresolvedShift => {
                    "official.workforce.selection.unresolved_shift"
                }
            },
            Self::LimitExceeded(limit) => match limit {
                AssignmentRuleLimit::InspectedPairs => "official.workforce.limit.inspected_pairs",
                AssignmentRuleLimit::WorkSteps => "official.workforce.limit.work_steps",
                AssignmentRuleLimit::ExpandedIntervals => {
                    "official.workforce.limit.expanded_intervals"
                }
                AssignmentRuleLimit::SelectedPairs => "official.workforce.limit.selected_pairs",
                AssignmentRuleLimit::Variables => "official.workforce.limit.variables",
                AssignmentRuleLimit::Constraints => "official.workforce.limit.constraints",
                AssignmentRuleLimit::ProvenanceRecords => {
                    "official.workforce.limit.provenance_records"
                }
                AssignmentRuleLimit::Records => "official.workforce.limit.records",
                AssignmentRuleLimit::References => "official.workforce.limit.references",
                AssignmentRuleLimit::Bytes => "official.workforce.limit.bytes",
                AssignmentRuleLimit::PerRecord => "official.workforce.limit.per_record",
            },
            Self::InvalidConstruction(issue) => match issue {
                AssignmentConstructionIssue::ArithmeticOverflow => {
                    "official.workforce.construction.arithmetic_overflow"
                }
                AssignmentConstructionIssue::InvalidIdentifier => {
                    "official.workforce.construction.invalid_identifier"
                }
                AssignmentConstructionIssue::InvalidRecord => {
                    "official.workforce.construction.invalid_record"
                }
                AssignmentConstructionIssue::IdentityCollision => {
                    "official.workforce.construction.identity_collision"
                }
                AssignmentConstructionIssue::SourceHash => {
                    "official.workforce.construction.source_hash"
                }
            },
        }
    }

    /// Converts operation failure into a blocking finding, never partial validity or infeasibility.
    #[must_use]
    pub fn validation_issue(&self) -> ValidationIssue {
        let field_path = match self {
            Self::InvalidDocument(DomainPackError::InvalidPayload { path, .. }) => {
                Some(path.clone())
            }
            Self::Temporal(issue) => issue.entity_id.map(|id| format!("domain.entities.{id}")),
            Self::InvalidSelection { .. } => Some("selectedPairs".to_owned()),
            _ => None,
        };
        ValidationIssue {
            code: self.code().to_owned(),
            severity: ValidationSeverity::Error,
            message: self.to_string(),
            field_path,
            resource: None,
        }
    }
}

impl fmt::Display for AssignmentRuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for AssignmentRuleError {}

impl From<DomainPackError> for AssignmentRuleError {
    fn from(error: DomainPackError) -> Self {
        match error {
            DomainPackError::Cancelled => Self::Cancelled,
            other => Self::InvalidDocument(other),
        }
    }
}

impl From<TemporalError> for AssignmentRuleError {
    fn from(error: TemporalError) -> Self {
        match error {
            TemporalError::InvalidDocument(error) => error.into(),
            TemporalError::Issue(issue) => Self::Temporal(issue),
            TemporalError::Cancelled => Self::Cancelled,
        }
    }
}

impl From<AssignmentRuleError> for DomainPackError {
    fn from(error: AssignmentRuleError) -> Self {
        match error {
            AssignmentRuleError::InvalidDocument(error) => error,
            AssignmentRuleError::Cancelled => Self::Cancelled,
            AssignmentRuleError::Temporal(issue) => Self::InvalidPayload {
                path: issue.entity_id.map_or_else(
                    || "settings".to_owned(),
                    |id| format!("domain.entities.{id}"),
                ),
                message: "official.workforce.temporal_review".to_owned(),
            },
            other => Self::Contract(other.code().to_owned()),
        }
    }
}
