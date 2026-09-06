mod support;

use eutheto_types::{
    CancellationToken, GapPolicy, Horizon, OverlapPolicy, ScenarioDocument,
    TimeResolutionFailureKind,
};
use eutheto_workforce::{
    ids::WorkCalendarId,
    temporal::{
        CalendarWindow, MAX_CALENDAR_WINDOWS, TemporalError, TemporalIssueKind, reporting_window,
        resolve_calendar,
    },
};
use jiff::{Span, civil::Date};
use serde_json::{Value, json};
use std::error::Error;
use support::{fixture, id};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn calendar_id() -> Result<WorkCalendarId> {
    Ok(id(2).parse()?)
}

fn document(period: Value) -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    let key = calendar_id()?.as_entity_id();
    document.domain.entities.retain(|id, _| *id == key);
    document.domain.locked_assignments.clear();
    document
        .domain
        .entities
        .get_mut(&key)
        .ok_or("missing calendar")?["period"] = period;
    Ok(document)
}

fn utc_document(period: Value) -> Result<ScenarioDocument> {
    let mut document = document(period)?;
    document.settings.time_zone = "UTC".parse()?;
    document.settings.horizon = range("2026-11-01T00:00:00Z", "2026-11-02T00:00:00Z")?;
    Ok(document)
}

fn range(start: &str, end: &str) -> Result<Horizon> {
    Ok(Horizon::new(start.parse()?, end.parse()?)?)
}

fn query(document: &ScenarioDocument, start: &str, end: &str) -> Result<Vec<CalendarWindow>> {
    Ok(resolve_calendar(
        document,
        calendar_id()?,
        range(start, end)?,
        &CancellationToken::new(),
    )?)
}

fn owner(document: &ScenarioDocument, date: &str) -> Result<CalendarWindow> {
    reporting_window(
        document,
        calendar_id()?,
        date.parse()?,
        &CancellationToken::new(),
    )?
    .ok_or_else(|| "reporting date has no owner".into())
}

fn assert_kind<T>(result: std::result::Result<T, TemporalError>, expected: TemporalIssueKind) {
    let actual = result.err().and_then(|error| match error {
        TemporalError::Issue(issue) => Some(issue.kind),
        TemporalError::InvalidDocument(_) | TemporalError::Cancelled => None,
    });
    assert_eq!(actual, Some(expected));
}

#[test]
fn non_midnight_days_preserve_whole_boundaries_and_half_open_edges() -> Result {
    let document = utc_document(json!({"kind":"day","startTime":"06:00:00"}))?;
    let windows = query(&document, "2026-11-01T06:00:00Z", "2026-11-02T06:00:00Z")?;
    assert_eq!(windows.len(), 1);
    assert_eq!(
        windows[0].interval.starts_at.instant,
        "2026-11-01T06:00:00Z".parse()?
    );
    assert_eq!(
        windows[0].interval.ends_at.instant,
        "2026-11-02T06:00:00Z".parse()?
    );
    let before = query(
        &document,
        "2026-11-01T05:59:59.999999999Z",
        "2026-11-01T06:00:00Z",
    )?;
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0].interval.starts_at.local.as_datetime(),
        "2026-10-31T06:00:00".parse()?
    );
    assert_eq!(
        before[0].interval.ends_at.instant,
        windows[0].interval.starts_at.instant
    );
    let crossing = query(&document, "2026-11-01T05:59:59Z", "2026-11-01T06:00:01Z")?;
    assert_eq!(crossing, vec![before[0], windows[0]]);
    // The reporting date belongs to its date-anchored period, not the period
    // containing midnight or the instant at which work happens to start.
    assert_eq!(owner(&document, "2026-11-01")?, windows[0]);
    Ok(())
}

#[test]
fn anchored_weeks_and_pay_periods_use_euclidean_arithmetic_before_anchor() -> Result {
    for (period, expected_start, expected_end) in [
        (
            json!({"kind":"week","anchorDate":"2026-11-09","startTime":"18:00:00"}),
            "2026-10-26",
            "2026-11-02",
        ),
        (
            json!({"kind":"payPeriod","anchorDate":"2026-11-09","startTime":"18:00:00","lengthDays":14}),
            "2026-10-26",
            "2026-11-09",
        ),
    ] {
        let document = utc_document(period)?;
        let window = owner(&document, "2026-11-01")?;
        assert_eq!(
            window.interval.starts_at.local.as_datetime(),
            format!("{expected_start}T18:00:00").parse()?
        );
        assert_eq!(
            window.interval.ends_at.local.as_datetime(),
            format!("{expected_end}T18:00:00").parse()?
        );
        let windows = query(&document, "2026-11-01T12:00:00Z", "2026-11-01T13:00:00Z")?;
        assert_eq!(windows, vec![window]);
        // Reporting ownership changes on the ending civil date, before 18:00.
        let next = owner(&document, expected_end)?;
        assert_eq!(
            next.interval.starts_at.local.as_datetime().date(),
            expected_end.parse::<Date>()?
        );
    }
    Ok(())
}

#[test]
fn moved_spring_boundary_after_query_start_keeps_the_previous_window() -> Result {
    let mut document = document(json!({"kind":"day","startTime":"02:30:00"}))?;
    document.settings.gap_policy = GapPolicy::MoveForward;
    let previous = query(&document, "2026-03-08T07:10:00Z", "2026-03-08T07:20:00Z")?;
    assert_eq!(previous.len(), 1);
    assert_eq!(
        previous[0].interval.starts_at.local.as_datetime(),
        "2026-03-07T02:30:00".parse()?
    );
    assert_eq!(
        previous[0].interval.ends_at.local.as_datetime(),
        "2026-03-08T02:30:00".parse()?
    );
    assert_eq!(
        previous[0].interval.ends_at.instant,
        "2026-03-08T07:30:00Z".parse()?
    );
    let current = query(&document, "2026-03-08T07:30:00Z", "2026-03-08T07:31:00Z")?;
    assert_eq!(current.len(), 1);
    assert_eq!(current[0], owner(&document, "2026-03-08")?);
    assert_eq!(current[0].interval.elapsed_duration().as_secs(), 23 * 3600);
    assert_eq!(
        current[0].interval.scheduled_duration().as_secs(),
        24 * 3600
    );
    for (policy, expected) in [
        (GapPolicy::Reject, TimeResolutionFailureKind::Gap),
        (
            GapPolicy::PackDefined,
            TimeResolutionFailureKind::PackResolutionRequired,
        ),
    ] {
        document.settings.gap_policy = policy;
        assert_kind(
            reporting_window(
                &document,
                calendar_id()?,
                "2026-03-08".parse()?,
                &CancellationToken::new(),
            ),
            TemporalIssueKind::Resolution(expected),
        );
    }
    // Candidate search must not reject a gap belonging to an unrelated earlier day.
    let later = query(&document, "2026-03-10T12:00:00Z", "2026-03-10T13:00:00Z")?;
    assert_eq!(later.len(), 1);
    assert_eq!(
        later[0].interval.starts_at.local.as_datetime(),
        "2026-03-10T02:30:00".parse()?
    );
    Ok(())
}

#[test]
fn repeated_boundary_obeys_both_fold_choices_and_reject() -> Result {
    let mut document = document(json!({"kind":"day","startTime":"01:30:00"}))?;
    for (policy, expected, hours) in [
        (OverlapPolicy::Earlier, "2026-11-01T05:30:00Z", 25),
        (OverlapPolicy::Later, "2026-11-01T06:30:00Z", 24),
    ] {
        document.settings.overlap_policy = policy;
        let window = owner(&document, "2026-11-01")?;
        assert_eq!(window.interval.starts_at.instant, expected.parse()?);
        assert_eq!(
            window.interval.starts_at.local.as_datetime(),
            "2026-11-01T01:30:00".parse()?
        );
        assert_eq!(window.interval.elapsed_duration().as_secs(), hours * 3600);
        assert_eq!(
            query(&document, expected, "2026-11-01T06:31:00Z")?,
            vec![window]
        );
    }
    document.settings.overlap_policy = OverlapPolicy::Reject;
    assert_kind(
        resolve_calendar(
            &document,
            calendar_id()?,
            range("2026-11-01T05:00:00Z", "2026-11-01T07:00:00Z")?,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::Resolution(TimeResolutionFailureKind::Overlap),
    );
    Ok(())
}

#[test]
fn custom_windows_reject_resolved_overlap_and_nonpositive_duration() -> Result {
    for (intervals, expected) in [
        (
            json!([
                {"startsAt":"2026-03-08T02:00:00","endsAt":"2026-03-08T02:45:00"},
                {"startsAt":"2026-03-08T03:15:00","endsAt":"2026-03-08T04:00:00"}
            ]),
            TemporalIssueKind::CalendarOverlap,
        ),
        (
            json!([{ "startsAt":"2026-03-08T02:30:00","endsAt":"2026-03-08T03:15:00" }]),
            TemporalIssueKind::InvalidInterval,
        ),
    ] {
        let mut document = document(json!({"kind":"custom","intervals":intervals}))?;
        document.settings.gap_policy = GapPolicy::MoveForward;
        assert_kind(
            resolve_calendar(
                &document,
                calendar_id()?,
                range("2026-03-08T06:00:00Z", "2026-03-08T09:00:00Z")?,
                &CancellationToken::new(),
            ),
            expected,
        );
        assert_kind(
            reporting_window(
                &document,
                calendar_id()?,
                "2026-03-08".parse()?,
                &CancellationToken::new(),
            ),
            expected,
        );
    }
    Ok(())
}

#[test]
fn disjoint_custom_windows_cannot_reverse_after_gap_resolution() -> Result {
    let mut document = document(json!({"kind":"custom","intervals":[
        {"startsAt":"2026-03-08T02:30:00","endsAt":"2026-03-08T02:40:00"},
        {"startsAt":"2026-03-08T03:00:00","endsAt":"2026-03-08T03:10:00"}
    ]}))?;
    document.settings.gap_policy = GapPolicy::MoveForward;
    assert_kind(
        resolve_calendar(
            &document,
            calendar_id()?,
            range("2026-03-08T07:00:00Z", "2026-03-08T08:00:00Z")?,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::CalendarOrder,
    );
    assert_kind(
        reporting_window(
            &document,
            calendar_id()?,
            "2026-03-08".parse()?,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::CalendarOrder,
    );
    Ok(())
}

#[test]
fn custom_reporting_ambiguity_does_not_disable_instant_queries() -> Result {
    let document = utc_document(json!({"kind":"custom","intervals":[
        {"startsAt":"2026-11-01T08:00:00","endsAt":"2026-11-01T10:00:00"},
        {"startsAt":"2026-11-01T12:00:00","endsAt":"2026-11-02T00:00:00"},
        {"startsAt":"2026-11-02T00:00:00","endsAt":"2026-11-03T01:00:00"}
    ]}))?;
    assert_kind(
        reporting_window(
            &document,
            calendar_id()?,
            "2026-11-01".parse()?,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::AmbiguousReportingDate,
    );
    let first = query(&document, "2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z")?;
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].interval.ends_at.instant,
        "2026-11-01T10:00:00Z".parse()?
    );
    assert!(query(&document, "2026-11-01T10:00:00Z", "2026-11-01T12:00:00Z")?.is_empty());
    let next = owner(&document, "2026-11-02")?;
    assert_eq!(
        next.interval.starts_at.local.as_datetime(),
        "2026-11-02T00:00:00".parse()?
    );
    assert_eq!(owner(&document, "2026-11-03")?, next);
    assert!(
        reporting_window(
            &document,
            calendar_id()?,
            "2026-11-04".parse()?,
            &CancellationToken::new()
        )?
        .is_none()
    );
    Ok(())
}

#[test]
fn calendar_output_limit_is_exact_and_date_arithmetic_does_not_wrap() -> Result {
    let document = utc_document(json!({"kind":"day","startTime":"00:00:00"}))?;
    let start: Date = "2000-01-01".parse()?;
    let end = start.checked_add(Span::new().days(i64::try_from(MAX_CALENDAR_WINDOWS)?))?;
    let start_instant = format!("{start}T00:00:00Z");
    let end_instant = format!("{end}T00:00:00Z");
    let windows = query(&document, &start_instant, &end_instant)?;
    assert_eq!(windows.len(), MAX_CALENDAR_WINDOWS);
    assert_eq!(
        windows
            .first()
            .ok_or("missing first window")?
            .interval
            .starts_at
            .instant,
        start_instant.parse()?
    );
    assert_eq!(
        windows
            .last()
            .ok_or("missing final window")?
            .interval
            .ends_at
            .instant,
        end_instant.parse()?
    );
    let excess = end.checked_add(Span::new().days(1))?;
    assert_kind(
        resolve_calendar(
            &document,
            calendar_id()?,
            range(&start_instant, &format!("{excess}T00:00:00Z"))?,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::CalendarLimit,
    );
    assert_kind(
        reporting_window(
            &document,
            calendar_id()?,
            Date::MAX,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::DateOverflow,
    );
    assert_kind(
        resolve_calendar(
            &document,
            calendar_id()?,
            range("9999-12-29T00:00:00Z", "9999-12-30T00:00:00Z")?,
            &CancellationToken::new(),
        ),
        TemporalIssueKind::DateOverflow,
    );
    Ok(())
}

#[test]
fn ingress_wrong_kind_invalid_range_and_cancellation_are_enforced() -> Result {
    let mut document = fixture()?;
    let cancellation = CancellationToken::new();
    for target in [id(1), id(999)] {
        assert_kind(
            resolve_calendar(
                &document,
                target.parse()?,
                document.settings.horizon,
                &cancellation,
            ),
            TemporalIssueKind::UnknownCalendar,
        );
        assert_kind(
            reporting_window(
                &document,
                target.parse()?,
                "2026-11-01".parse()?,
                &cancellation,
            ),
            TemporalIssueKind::UnknownCalendar,
        );
    }
    let instant = document.settings.horizon.start;
    assert_kind(
        resolve_calendar(
            &document,
            calendar_id()?,
            Horizon {
                start: instant,
                end: instant,
            },
            &cancellation,
        ),
        TemporalIssueKind::InvalidQuery,
    );
    document
        .domain
        .entities
        .get_mut(&id(2).parse()?)
        .ok_or("missing calendar")?["period"] =
        json!({"kind":"payPeriod","anchorDate":"2026-11-01","startTime":"00:00:00","lengthDays":0});
    assert!(matches!(
        resolve_calendar(
            &document,
            calendar_id()?,
            document.settings.horizon,
            &cancellation
        ),
        Err(TemporalError::InvalidDocument(_))
    ));
    assert!(matches!(
        reporting_window(
            &document,
            calendar_id()?,
            "2026-11-01".parse()?,
            &cancellation
        ),
        Err(TemporalError::InvalidDocument(_))
    ));
    let document = fixture()?;
    cancellation.cancel();
    assert!(matches!(
        resolve_calendar(
            &document,
            calendar_id()?,
            document.settings.horizon,
            &cancellation
        ),
        Err(TemporalError::Cancelled)
    ));
    assert!(matches!(
        reporting_window(
            &document,
            calendar_id()?,
            "2026-11-01".parse()?,
            &cancellation
        ),
        Err(TemporalError::Cancelled)
    ));
    Ok(())
}
