use super::{
    CalendarWindow, MAX_CALENDAR_WINDOWS, ResolvedInterval, TemporalError, TemporalIssueKind,
    check_cancelled, issue, resolve_interval,
};
use crate::{
    ids::WorkCalendarId,
    model::{CalendarPeriod, LocalInterval, WorkCalendar, WorkforceEntity},
    validation::validate_document,
};
use eutheto_types::{
    CancellationToken, Horizon, ScenarioDocument, ScenarioSettings, TimeResolutionFailureKind,
};
use jiff::{
    SignedDuration, Span,
    civil::{Date, DateTime, Time},
    tz::{Offset, TimeZone},
};

/// Resolves whole calendar windows intersecting the explicit half-open instant range.
/// The scenario horizon does not clip or otherwise constrain this query.
///
/// # Errors
/// Rejects invalid documents, unknown/wrong-kind calendars, empty queries, unresolved
/// boundaries, nonpositive, overlapping or reordered custom windows, overflow,
/// limits and cancellation.
pub fn resolve_calendar(
    document: &ScenarioDocument,
    calendar_id: WorkCalendarId,
    range: Horizon,
    cancellation: &CancellationToken,
) -> Result<Vec<CalendarWindow>, TemporalError> {
    let calendar = validated_calendar(document, calendar_id, cancellation)?;
    if range.start >= range.end {
        return Err(issue(
            TemporalIssueKind::InvalidQuery,
            Some(calendar_id.into()),
            None,
        ));
    }
    if let CalendarPeriod::Custom { intervals } = &calendar.period {
        return custom_windows(
            intervals,
            calendar_id,
            &document.settings,
            Some(range),
            cancellation,
        );
    }
    let period = RegularPeriod::from_calendar(&calendar);
    let zone = TimeZone::get(document.settings.time_zone.as_str()).map_err(|_| {
        issue(
            TemporalIssueKind::Resolution(TimeResolutionFailureKind::InvalidTimeZone),
            Some(calendar_id.into()),
            None,
        )
    })?;
    // Every supported offset, including the pre-transition offset used by a moved
    // gap boundary, lies within these bounds. Use civil arithmetic so no machine
    // zone or timestamp saturation participates in selecting candidate periods.
    let lower = range
        .start
        .as_timestamp()
        .to_zoned(TimeZone::UTC)
        .datetime()
        .checked_sub(SignedDuration::from_secs(i64::from(Offset::MAX.seconds())))
        .map_err(|_| overflow(calendar_id, None))?;
    let upper = range
        .end
        .as_timestamp()
        .to_zoned(TimeZone::UTC)
        .datetime()
        .checked_sub(SignedDuration::from_secs(i64::from(Offset::MIN.seconds())))
        .map_err(|_| overflow(calendar_id, None))?;
    let mut date = period.start_date(lower.date(), calendar_id)?;
    if date.to_datetime(period.time) > lower {
        date = add_days(date, -period.days, calendar_id)?;
    }
    let mut windows = Vec::new();
    while date.to_datetime(period.time) < upper {
        check_cancelled(cancellation)?;
        let next = add_days(date, period.days, calendar_id)?;
        let start = date.to_datetime(period.time);
        let end = next.to_datetime(period.time);
        // Earlier/later here bound *possible* instants, not a resolution policy.
        // This avoids rejecting unrelated DST boundaries outside the query. Every
        // retained endpoint is resolved below solely by the explicit host policies.
        if potentially_intersects(start, end, &zone, range, calendar_id)? {
            let window = CalendarWindow {
                calendar_id,
                interval: resolve_interval(start, end, &document.settings, calendar_id.into())?,
            };
            if intersects(window, range) {
                push_window(&mut windows, window)?;
            }
        }
        date = next;
    }
    check_cancelled(cancellation)?;
    Ok(windows)
}

/// Finds the window owning a civil reporting date, independently of its clock boundary.
/// Regular periods own 1/7/lengthDays dates beginning at their anchor occurrence.
/// Custom windows own touched local dates, excluding a midnight end date.
///
/// # Errors
/// In addition to calendar resolution errors, rejects multiple custom owners of
/// the reporting date rather than choosing one or attributing a workload twice.
pub fn reporting_window(
    document: &ScenarioDocument,
    calendar_id: WorkCalendarId,
    date: Date,
    cancellation: &CancellationToken,
) -> Result<Option<CalendarWindow>, TemporalError> {
    let calendar = validated_calendar(document, calendar_id, cancellation)?;
    if let CalendarPeriod::Custom { intervals } = &calendar.period {
        let windows = custom_windows(
            intervals,
            calendar_id,
            &document.settings,
            None,
            cancellation,
        )?;
        let mut owner = None;
        for window in windows {
            check_cancelled(cancellation)?;
            let start = window.interval.starts_at.local.as_datetime();
            let end = window.interval.ends_at.local.as_datetime();
            if start.date() <= date
                && (date < end.date() || (date == end.date() && end.time() != Time::MIN))
            {
                if owner.is_some() {
                    return Err(issue(
                        TemporalIssueKind::AmbiguousReportingDate,
                        Some(calendar_id.into()),
                        Some(date),
                    ));
                }
                owner = Some(window);
            }
        }
        return Ok(owner);
    }
    let period = RegularPeriod::from_calendar(&calendar);
    let start = period.start_date(date, calendar_id)?;
    let end = add_days(start, period.days, calendar_id)?;
    let interval = resolve_interval(
        start.to_datetime(period.time),
        end.to_datetime(period.time),
        &document.settings,
        calendar_id.into(),
    )?;
    check_cancelled(cancellation)?;
    Ok(Some(CalendarWindow {
        calendar_id,
        interval,
    }))
}

fn validated_calendar(
    document: &ScenarioDocument,
    calendar_id: WorkCalendarId,
    cancellation: &CancellationToken,
) -> Result<WorkCalendar, TemporalError> {
    check_cancelled(cancellation)?;
    let mut domain = validate_document(document)?;
    check_cancelled(cancellation)?;
    match domain.entities.remove(&calendar_id.into()) {
        Some(WorkforceEntity::Calendar(calendar)) => Ok(calendar),
        _ => Err(issue(
            TemporalIssueKind::UnknownCalendar,
            Some(calendar_id.into()),
            None,
        )),
    }
}

struct RegularPeriod {
    anchor: Date,
    time: Time,
    days: i64,
}

impl RegularPeriod {
    fn from_calendar(calendar: &WorkCalendar) -> Self {
        match calendar.period {
            CalendarPeriod::Day { start_time } => Self {
                anchor: Date::MIN,
                time: start_time,
                days: 1,
            },
            CalendarPeriod::Week {
                anchor_date,
                start_time,
            } => Self {
                anchor: anchor_date,
                time: start_time,
                days: 7,
            },
            CalendarPeriod::PayPeriod {
                anchor_date,
                start_time,
                length_days,
            } => Self {
                anchor: anchor_date,
                time: start_time,
                days: i64::from(length_days),
            },
            CalendarPeriod::Custom { .. } => {
                unreachable!("custom calendars are resolved separately")
            }
        }
    }

    fn start_date(&self, date: Date, id: WorkCalendarId) -> Result<Date, TemporalError> {
        let delta = self
            .anchor
            .to_datetime(Time::MIN)
            .duration_until(date.to_datetime(Time::MIN))
            .as_secs()
            / 86_400;
        // Euclidean remainder selects the containing period even before the anchor.
        add_days(date, -delta.rem_euclid(self.days), id)
    }
}

fn add_days(date: Date, days: i64, id: WorkCalendarId) -> Result<Date, TemporalError> {
    date.checked_add(Span::new().days(days))
        .map_err(|_| overflow(id, Some(date)))
}

fn overflow(id: WorkCalendarId, date: Option<Date>) -> TemporalError {
    issue(TemporalIssueKind::DateOverflow, Some(id.into()), date)
}

fn potentially_intersects(
    start: DateTime,
    end: DateTime,
    zone: &TimeZone,
    range: Horizon,
    id: WorkCalendarId,
) -> Result<bool, TemporalError> {
    let earliest_start = zone
        .to_ambiguous_zoned(start)
        .earlier()
        .map_err(|_| overflow(id, Some(start.date())))?
        .timestamp();
    let latest_end = zone
        .to_ambiguous_zoned(end)
        .later()
        .map_err(|_| overflow(id, Some(end.date())))?
        .timestamp();
    Ok(earliest_start < range.end.as_timestamp() && range.start.as_timestamp() < latest_end)
}

fn intersects(window: CalendarWindow, range: Horizon) -> bool {
    window.interval.starts_at.instant < range.end && range.start < window.interval.ends_at.instant
}

fn push_window(
    windows: &mut Vec<CalendarWindow>,
    window: CalendarWindow,
) -> Result<(), TemporalError> {
    if windows.len() == MAX_CALENDAR_WINDOWS {
        return Err(issue(
            TemporalIssueKind::CalendarLimit,
            Some(window.calendar_id.into()),
            None,
        ));
    }
    windows.push(window);
    Ok(())
}

fn custom_windows(
    intervals: &[LocalInterval],
    calendar_id: WorkCalendarId,
    settings: &ScenarioSettings,
    range: Option<Horizon>,
    cancellation: &CancellationToken,
) -> Result<Vec<CalendarWindow>, TemporalError> {
    let mut windows = Vec::new();
    let mut previous: Option<ResolvedInterval> = None;
    for local in intervals {
        check_cancelled(cancellation)?;
        let interval = resolve_interval(
            local.starts_at.as_datetime(),
            local.ends_at.as_datetime(),
            settings,
            calendar_id.into(),
        )?;
        // Check the complete configured calendar, including windows outside an
        // instant query. Stored ordering must survive resolution without repair.
        if let Some(previous) = previous {
            if previous.overlaps(interval) {
                return Err(issue(
                    TemporalIssueKind::CalendarOverlap,
                    Some(calendar_id.into()),
                    None,
                ));
            }
            if previous.starts_at.instant > interval.starts_at.instant {
                return Err(issue(
                    TemporalIssueKind::CalendarOrder,
                    Some(calendar_id.into()),
                    None,
                ));
            }
        }
        previous = Some(interval);
        let window = CalendarWindow {
            calendar_id,
            interval,
        };
        if range.is_none_or(|range| intersects(window, range)) {
            push_window(&mut windows, window)?;
        }
    }
    check_cancelled(cancellation)?;
    Ok(windows)
}
