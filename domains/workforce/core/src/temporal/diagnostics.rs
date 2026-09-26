use super::{TemporalEndpoint, TemporalIssue, TemporalIssueKind, TemporalOrigin};
use eutheto_types::{ScenarioDocument, TimeResolutionFailureKind};

/// Safe field and occurrence context shared by setup and full validation.
pub(crate) struct TemporalDiagnostic {
    pub code: &'static str,
    pub field_path: Option<String>,
    pub message: String,
}

pub(crate) fn issue_report(
    document: &ScenarioDocument,
    issue: TemporalIssue,
) -> TemporalDiagnostic {
    let field_path = if issue.kind
        == TemporalIssueKind::Resolution(TimeResolutionFailureKind::InvalidTimeZone)
    {
        Some("/settings/timeZone".to_owned())
    } else {
        issue.entity_id.map(|id| {
            if let Some(origin) = issue.origin {
                let endpoint = match (origin, issue.endpoint) {
                    (
                        TemporalOrigin::PersonActiveRange
                        | TemporalOrigin::AvailabilityEffectiveRange,
                        Some(TemporalEndpoint::Start),
                    ) => "/startDate",
                    (
                        TemporalOrigin::PersonActiveRange
                        | TemporalOrigin::AvailabilityEffectiveRange,
                        Some(TemporalEndpoint::End),
                    ) => "/endDateExclusive",
                    (
                        TemporalOrigin::AvailabilityWeeklyWindow { .. },
                        Some(TemporalEndpoint::Start),
                    ) => "/startTime",
                    (
                        TemporalOrigin::AvailabilityWeeklyWindow { .. },
                        Some(TemporalEndpoint::End),
                    ) => "/endTime",
                    (_, None) => "",
                };
                return match origin {
                    TemporalOrigin::PersonActiveRange => {
                        format!("/domain/entities/{id}/activeRange{endpoint}")
                    }
                    TemporalOrigin::AvailabilityEffectiveRange => {
                        format!("/domain/entities/{id}/effectiveRange{endpoint}")
                    }
                    TemporalOrigin::AvailabilityWeeklyWindow { index } => {
                        format!("/domain/entities/{id}/timeWindow/windows/{index}{endpoint}")
                    }
                };
            }
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

pub(crate) fn issue_detail(issue: TemporalIssue, field_path: Option<String>) -> TemporalDiagnostic {
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
