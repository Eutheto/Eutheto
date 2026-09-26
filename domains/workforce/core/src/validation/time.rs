use super::{
    common::{Result, bounded, invalid, prefix, require, unique},
    context::Context,
};
use crate::model::{CalendarPeriod, DateRange, Recurrence, ShiftTiming, TimeWindow};
use eutheto_types::{GapPolicy, ResolvedLocalTime, resolve_local_time};
use jiff::{civil::Time, tz::AmbiguousOffset};

pub(super) fn date_range(range: DateRange) -> Result {
    require(
        range.start_date < range.end_date_exclusive,
        "endDateExclusive",
        "date range must be increasing and nonempty",
    )
}

fn local_window(start: Time, end: Time, offset: u8, path: &str) -> Result {
    require(
        offset > 0 || start < end,
        path,
        "local interval must have positive duration",
    )
}

pub(super) fn timing(value: &ShiftTiming) -> Result {
    match *value {
        ShiftTiming::LocalWindow {
            start_time,
            end_time,
            end_day_offset,
        } => local_window(start_time, end_time, end_day_offset, "endTime"),
        ShiftTiming::ElapsedDuration {
            duration_minutes, ..
        } => require(
            duration_minutes > 0,
            "durationMinutes",
            "duration must be positive",
        ),
    }
}

pub(super) fn recurrence(value: &Recurrence) -> Result {
    prefix(date_range(value.effective_range), "effectiveRange")?;
    unique(&value.weekdays, true, "weekdays")?;
    unique(&value.excluded_dates, false, "excludedDates")
}

pub(super) fn calendar(value: &CalendarPeriod) -> Result {
    match value {
        CalendarPeriod::Day { .. } | CalendarPeriod::Week { .. } => Ok(()),
        CalendarPeriod::PayPeriod { length_days, .. } => require(
            *length_days > 0,
            "lengthDays",
            "period length must be positive",
        ),
        CalendarPeriod::Custom { intervals } => {
            bounded(intervals, true, "intervals")?;
            require(
                intervals.iter().all(|interval| {
                    interval.starts_at.as_datetime() < interval.ends_at.as_datetime()
                }),
                "intervals",
                "calendar intervals must be increasing and nonempty",
            )?;
            require(
                intervals
                    .windows(2)
                    .all(|pair| pair[0].ends_at.as_datetime() <= pair[1].starts_at.as_datetime()),
                "intervals",
                "calendar intervals must be ordered and nonoverlapping",
            )
        }
    }
}

pub(super) fn time_window(value: &TimeWindow) -> Result {
    match value {
        TimeWindow::Instant { starts_at, ends_at } => require(
            starts_at < ends_at,
            "endsAt",
            "instant interval must be increasing and nonempty",
        ),
        TimeWindow::Weekly { windows } => {
            bounded(windows, true, "windows")?;
            for (idx, window) in windows.iter().enumerate() {
                prefix(
                    unique(&window.weekdays, true, "weekdays"),
                    format_args!("windows.{idx}"),
                )?;
                prefix(
                    local_window(
                        window.start_time,
                        window.end_time,
                        window.end_day_offset,
                        "endTime",
                    ),
                    format_args!("windows.{idx}"),
                )?;
            }
            Ok(())
        }
    }
}

impl Context<'_> {
    pub(super) fn resolved_time(&self, value: &ResolvedLocalTime) -> Result {
        resolved_time_inner(value, self.settings, &self.zone, "")
    }
}

/// Public wrapper preserving the `resolvedTime.` path prefix for settings consumers.
pub(crate) fn validate_resolved_time(
    value: &ResolvedLocalTime,
    settings: &eutheto_types::ScenarioSettings,
    zone: &jiff::tz::TimeZone,
) -> Result {
    resolved_time_inner(value, settings, zone, "resolvedTime")
}

fn resolved_time_inner(
    value: &ResolvedLocalTime,
    settings: &eutheto_types::ScenarioSettings,
    zone: &jiff::tz::TimeZone,
    path_prefix: &str,
) -> Result {
    let actual = value.instant.as_timestamp().to_zoned(zone.clone());
    let result: Result = (|| {
        require(
            actual.offset().seconds() == value.offset_seconds,
            "offsetSeconds",
            "offset does not match the scenario-zone instant",
        )?;
        if actual.datetime() == value.local.as_datetime() {
            return Ok(());
        }
        require(
            settings.gap_policy == GapPolicy::MoveForward
                && matches!(
                    zone.to_ambiguous_zoned(value.local.as_datetime()).offset(),
                    AmbiguousOffset::Gap { .. }
                ),
            "local",
            "local intent does not match the scenario-zone instant",
        )?;
        let resolved = resolve_local_time(
            value.local,
            &settings.time_zone,
            settings.gap_policy,
            settings.overlap_policy,
        )
        .map_err(|_| {
            invalid(
                "local",
                "local intent cannot be resolved under the scenario policy",
            )
        })?;
        require(
            resolved == *value,
            "",
            "resolved gap differs from the explicit scenario resolver",
        )
    })();
    if path_prefix.is_empty() {
        result
    } else {
        prefix(result, path_prefix)
    }
}
