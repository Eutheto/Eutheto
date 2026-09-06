use crate::ids::{ShiftId, ShiftTemplateId, WorkCalendarId};
use crate::validation::{MAX_DOCUMENT_OCCURRENCES, MAX_REFERENCE_ITEMS};
use eutheto_domain_api::{DomainBatchCommand, DomainPackError};
use eutheto_types::{EntityId, ResolvedLocalTime, TimeResolutionFailureKind};
use jiff::{SignedDuration, civil::Date};
use std::fmt;

/// Upper bound for ledger/detached occurrences plus stored manual shifts.
pub const MAX_RESOLVED_SHIFTS: usize = MAX_DOCUMENT_OCCURRENCES + MAX_REFERENCE_ITEMS;
pub const MAX_SHIFT_CHANGES: usize = 2 * MAX_RESOLVED_SHIFTS;
pub const MAX_CALENDAR_WINDOWS: usize = MAX_DOCUMENT_OCCURRENCES;

/// Exact endpoints retain intended wall time independently of elapsed time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedInterval {
    pub starts_at: ResolvedLocalTime,
    pub ends_at: ResolvedLocalTime,
}

impl ResolvedInterval {
    #[must_use]
    pub fn elapsed_duration(self) -> SignedDuration {
        self.starts_at
            .instant
            .as_timestamp()
            .duration_until(self.ends_at.instant.as_timestamp())
    }

    /// Signed intent, including a negative span across explicitly chosen fold occurrences.
    #[must_use]
    pub fn scheduled_duration(self) -> SignedDuration {
        self.starts_at
            .local
            .as_datetime()
            .duration_until(self.ends_at.local.as_datetime())
    }

    /// Touching half-open intervals do not overlap.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        self.starts_at.instant < other.ends_at.instant
            && other.starts_at.instant < self.ends_at.instant
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedShiftOrigin {
    Generated {
        template_id: ShiftTemplateId,
        occurrence_date: Date,
    },
    Detached {
        template_id: ShiftTemplateId,
        occurrence_date: Date,
    },
    Manual,
}

/// Compact operation-local timing evidence, not a stored instance or solver authority.
/// Coverage, tags and other metadata remain in the original source record, owned once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedShift {
    pub id: ShiftId,
    pub origin: ResolvedShiftOrigin,
    pub interval: ResolvedInterval,
    pub reporting_date: Date,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CalendarWindow {
    pub calendar_id: WorkCalendarId,
    pub interval: ResolvedInterval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalIssueKind {
    Resolution(TimeResolutionFailureKind),
    DateOverflow,
    InvalidInterval,
    OutsideHorizon,
    UnreconciledIdentity,
    IdentityCollision,
    IdentityTransition,
    OccurrenceLimit,
    OutputLimit,
    CalendarLimit,
    CalendarOverlap,
    CalendarOrder,
    AmbiguousReportingDate,
    UnknownCalendar,
    InvalidQuery,
    DifferentScenario,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemporalIssue {
    pub kind: TemporalIssueKind,
    pub entity_id: Option<EntityId>,
    pub local_date: Option<Date>,
}

#[derive(Debug)]
pub enum TemporalError {
    InvalidDocument(DomainPackError),
    Issue(TemporalIssue),
    Cancelled,
}

impl From<DomainPackError> for TemporalError {
    fn from(error: DomainPackError) -> Self {
        match error {
            DomainPackError::Cancelled => Self::Cancelled,
            other => Self::InvalidDocument(other),
        }
    }
}

impl fmt::Display for TemporalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument(error) => error.fmt(f),
            Self::Issue(issue) => write!(f, "Workforce temporal review required: {:?}", issue.kind),
            Self::Cancelled => f.write_str("Workforce temporal operation cancelled"),
        }
    }
}

impl std::error::Error for TemporalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidDocument(error) => Some(error),
            Self::Issue(_) | Self::Cancelled => None,
        }
    }
}

/// A prior unresolved occurrence may be repaired without inventing a prior instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PriorShift {
    Resolved(ResolvedShift),
    Unresolved {
        id: ShiftId,
        origin: ResolvedShiftOrigin,
        issue: TemporalIssueKind,
    },
}

impl PriorShift {
    #[must_use]
    pub const fn id(self) -> ShiftId {
        match self {
            Self::Resolved(shift) => shift.id,
            Self::Unresolved { id, .. } => id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShiftChangeKind {
    Added,
    Changed,
    Removed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShiftChange {
    pub id: ShiftId,
    pub kind: ShiftChangeKind,
}

/// Pure review data. The batch applies only to the exact prospective document hash.
/// It does not apply the edits that produced that document or authorize their persistence.
/// `None` means no identity reconciliation is necessary; a blocked preview is an error.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationPreview {
    pub prospective_hash: [u8; 32],
    pub before: Vec<PriorShift>,
    pub after: Vec<ResolvedShift>,
    pub changes: Vec<ShiftChange>,
    pub reconciliation: Option<DomainBatchCommand>,
}
