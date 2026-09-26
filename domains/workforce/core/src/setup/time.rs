use super::{
    contracts::{LocalTimeResolutionParametersV1, WorkforcePositionV1, WorkforceSetupViewDataV1},
    paging::{ProjectionBudget, Result, invalid},
};
use crate::temporal::{
    TemporalError, TemporalIssue, TemporalIssueKind,
    diagnostics::{TemporalDiagnostic, issue_detail, issue_report},
};
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
            origin: None,
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
