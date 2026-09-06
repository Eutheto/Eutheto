use super::{
    common::{Result, bounded, invalid, require, unique},
    context::Context,
};
use crate::model::{CalendarPeriod, DateRange, Recurrence, ShiftTiming, TimeWindow};
use eutheto_types::{GapPolicy, ResolvedLocalTime, resolve_local_time};
use jiff::{civil::Time, tz::AmbiguousOffset};

pub(super) fn date_range(range: DateRange) -> Result {
    require(
        range.start_date < range.end_date_exclusive,
        "dateRange",
        "date range must be increasing and nonempty",
    )
}

fn local_window(start: Time, end: Time, offset: u8) -> Result {
    require(
        offset > 0 || start < end,
        "localWindow",
        "local interval must have positive duration",
    )
}

pub(super) fn timing(value: &ShiftTiming) -> Result {
    match *value {
        ShiftTiming::LocalWindow {
            start_time,
            end_time,
            end_day_offset,
        } => local_window(start_time, end_time, end_day_offset),
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
    date_range(value.effective_range)?;
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
            "timeWindow",
            "instant interval must be increasing and nonempty",
        ),
        TimeWindow::Weekly { windows } => {
            bounded(windows, true, "windows")?;
            for window in windows {
                unique(&window.weekdays, true, "weekdays")?;
                local_window(window.start_time, window.end_time, window.end_day_offset)?;
            }
            Ok(())
        }
    }
}

impl Context<'_> {
    pub(super) fn resolved_time(&self, value: &ResolvedLocalTime) -> Result {
        let actual = value.instant.as_timestamp().to_zoned(self.zone.clone());
        require(
            actual.offset().seconds() == value.offset_seconds,
            "resolvedTime.offsetSeconds",
            "offset does not match the scenario-zone instant",
        )?;
        if actual.datetime() == value.local.as_datetime() {
            return Ok(());
        }
        require(
            self.settings.gap_policy == GapPolicy::MoveForward
                && matches!(
                    self.zone
                        .to_ambiguous_zoned(value.local.as_datetime())
                        .offset(),
                    AmbiguousOffset::Gap { .. }
                ),
            "resolvedTime.local",
            "local intent does not match the scenario-zone instant",
        )?;
        let resolved = resolve_local_time(
            value.local,
            &self.settings.time_zone,
            self.settings.gap_policy,
            self.settings.overlap_policy,
        )
        .map_err(|_| {
            invalid(
                "resolvedTime.local",
                "local intent cannot be resolved under the scenario policy",
            )
        })?;
        require(
            resolved == *value,
            "resolvedTime",
            "resolved gap differs from the explicit scenario resolver",
        )
    }
}
