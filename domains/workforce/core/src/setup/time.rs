use super::{
    contracts::{LocalTimeResolutionParametersV1, WorkforcePositionV1, WorkforceSetupViewDataV1},
    paging::{ProjectionBudget, Result, invalid},
};
use crate::temporal::{TemporalEndpoint, TemporalError, TemporalIssue, TemporalIssueKind};
use eutheto_domain_api::DomainPackError;
use eutheto_types::{
    LocalWallTime, ScenarioDocument, TimeResolutionFailureKind, ValidationIssue,
    ValidationSeverity, resolve_local_time,
};
use jiff::{civil::Date, fmt::temporal::DateTimeParser};

pub(super) fn validation_error(
    code: &'static str,
    path: &str,
    message: &'static str,
) -> DomainPackError {
    DomainPackError::SetupValidation(Box::new(ValidationIssue {
        code: code.to_owned(),
        severity: ValidationSeverity::Error,
        message: message.to_owned(),
        field_path: Some(path.to_owned()),
        resource: None,
    }))
}

/// Shared by thrown findings and unresolved rows without allocating an unused issue code.
pub(super) struct TemporalDiagnostic {
    code: &'static str,
    pub field_path: Option<String>,
    pub message: String,
}

impl TemporalDiagnostic {
    fn into_error(self) -> DomainPackError {
        DomainPackError::SetupValidation(Box::new(ValidationIssue {
            code: self.code.to_owned(),
            severity: ValidationSeverity::Error,
            message: self.message,
            field_path: self.field_path,
            resource: None,
        }))
    }
}

pub(super) fn local_time_resolution(
    document: &ScenarioDocument,
    parameters: &LocalTimeResolutionParametersV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "local-time resolution does not accept continuation",
        ));
    }
    budget.visit()?;
    let invalid_local = || {
        validation_error(
            "workforce.time.invalid_local",
            "/query/parameters/local",
            "Enter a local date and time without an offset or timezone annotation.",
        )
    };
    // Jiff's civil parser accepts and discards numeric offsets and zone annotations.
    // Inspect the borrowed pieces once instead; this view accepts local intent only.
    let pieces = DateTimeParser::new()
        .parse_pieces(&parameters.local)
        .map_err(|_| invalid_local())?;
    if pieces.offset().is_some() || pieces.time_zone_annotation().is_some() {
        return Err(invalid_local());
    }
    let time = pieces.time().ok_or_else(invalid_local)?;
    let local = LocalWallTime::from_datetime(pieces.date().to_datetime(time));
    let resolved = resolve_local_time(
        local,
        &document.settings.time_zone,
        document.settings.gap_policy,
        document.settings.overlap_policy,
    )
    .map_err(|error| {
        resolution_error(
            error.kind,
            if error.kind == TimeResolutionFailureKind::InvalidTimeZone {
                "/settings/timeZone"
            } else {
                "/query/parameters/local"
            },
            local.as_datetime().date(),
        )
    })?;
    Ok(WorkforceSetupViewDataV1::LocalTimeResolution(resolved))
}

pub(super) fn resolution_error(
    kind: TimeResolutionFailureKind,
    path: &str,
    date: Date,
) -> DomainPackError {
    issue_detail(
        TemporalIssue {
            kind: TemporalIssueKind::Resolution(kind),
            entity_id: None,
            local_date: Some(date),
            endpoint: None,
            occurrence_date: None,
        },
        Some(path.to_owned()),
    )
    .into_error()
}

pub(super) fn temporal_error(document: &ScenarioDocument, error: TemporalError) -> DomainPackError {
    match error {
        TemporalError::InvalidDocument(error) => error,
        TemporalError::Cancelled => DomainPackError::Cancelled,
        TemporalError::Issue(issue) => match issue.kind {
            TemporalIssueKind::OccurrenceLimit
            | TemporalIssueKind::OutputLimit
            | TemporalIssueKind::CalendarLimit => DomainPackError::ResourceLimitExceeded,
            _ => issue_report(document, issue).into_error(),
        },
    }
}

pub(super) fn issue_report(
    document: &ScenarioDocument,
    issue: TemporalIssue,
) -> TemporalDiagnostic {
    let field_path = if issue.kind
        == TemporalIssueKind::Resolution(TimeResolutionFailureKind::InvalidTimeZone)
    {
        Some("/settings/timeZone".to_owned())
    } else {
        issue.entity_id.map(|id| {
            // Only the validated record discriminator selects a fixed suffix. No raw field
            // content or arbitrary parser message enters the user-facing error.
            let kind = document
                .domain
                .entities
                .get(&id)
                .and_then(|record| record["kind"].as_str());
            let suffix = match (kind, issue.endpoint) {
                (Some("shiftTemplate"), Some(TemporalEndpoint::Start)) => "/timing/startTime",
                (Some("shiftTemplate"), Some(TemporalEndpoint::End)) => "/timing/endTime",
                (Some("shiftTemplate"), None)
                    if matches!(
                        issue.kind,
                        TemporalIssueKind::DateOverflow | TemporalIssueKind::InvalidInterval
                    ) =>
                {
                    "/timing"
                }
                (Some("shiftInstance"), Some(TemporalEndpoint::Start)) => "/startsAt",
                (Some("shiftInstance"), Some(TemporalEndpoint::End)) => "/endsAt",
                (Some("calendar"), _) => "/period",
                _ => "",
            };
            format!("/domain/entities/{id}{suffix}")
        })
    };
    issue_detail(issue, field_path)
}

fn issue_detail(issue: TemporalIssue, field_path: Option<String>) -> TemporalDiagnostic {
    let (code, summary) = match issue.kind {
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::Gap) => (
            "workforce.time.gap",
            "The local time does not exist in the selected time zone.",
        ),
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::Overlap) => (
            "workforce.time.overlap",
            "The local time occurs twice in the selected time zone.",
        ),
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::PackResolutionRequired) => (
            "workforce.time.pack_resolution_required",
            "The local time requires explicit domain-pack resolution.",
        ),
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::InvalidTimeZone) => (
            "workforce.time.invalid_zone",
            "The time zone could not be resolved.",
        ),
        TemporalIssueKind::DateOverflow => (
            "workforce.temporal.date_overflow",
            "The temporal boundary exceeds its supported range.",
        ),
        TemporalIssueKind::InvalidInterval => (
            "workforce.temporal.invalid_interval",
            "The interval must end after it starts.",
        ),
        TemporalIssueKind::OutsideHorizon => (
            "workforce.temporal.outside_horizon",
            "The shift is outside the active planning horizon.",
        ),
        TemporalIssueKind::UnreconciledIdentity => (
            "workforce.temporal.unreconciled_identity",
            "Review generation before using this occurrence.",
        ),
        TemporalIssueKind::IdentityCollision => (
            "workforce.temporal.identity_collision",
            "Generated occurrences have conflicting identities.",
        ),
        TemporalIssueKind::IdentityTransition => (
            "workforce.temporal.identity_transition",
            "The occurrence ownership change requires explicit review.",
        ),
        TemporalIssueKind::OccurrenceLimit
        | TemporalIssueKind::OutputLimit
        | TemporalIssueKind::CalendarLimit => (
            "workforce.temporal.resource_limit",
            "The temporal operation exceeded its resource limit.",
        ),
        TemporalIssueKind::CalendarOverlap => (
            "workforce.temporal.calendar_overlap",
            "Calendar intervals must not overlap.",
        ),
        TemporalIssueKind::CalendarOrder => (
            "workforce.temporal.calendar_order",
            "Calendar intervals must be ordered.",
        ),
        TemporalIssueKind::AmbiguousReportingDate => (
            "workforce.temporal.ambiguous_reporting_date",
            "The reporting date requires explicit resolution.",
        ),
        TemporalIssueKind::UnknownCalendar => (
            "workforce.temporal.unknown_calendar",
            "The selected calendar is unavailable.",
        ),
        TemporalIssueKind::InvalidQuery => (
            "workforce.temporal.invalid_query",
            "The temporal request is not valid.",
        ),
        TemporalIssueKind::DifferentScenario => (
            "workforce.temporal.different_scenario",
            "The temporal comparison requires the same scenario.",
        ),
    };
    let message = match (issue.occurrence_date, issue.local_date) {
        (Some(occurrence), Some(local)) => {
            format!("{summary} Occurrence date: {occurrence}; local date: {local}.")
        }
        (Some(occurrence), None) => format!("{summary} Occurrence date: {occurrence}."),
        (None, Some(local)) => format!("{summary} Local date: {local}."),
        (None, None) => summary.to_owned(),
    };
    TemporalDiagnostic {
        code,
        field_path,
        message,
    }
}
