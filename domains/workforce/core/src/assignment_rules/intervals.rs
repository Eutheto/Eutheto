use super::{
    AssignmentRuleError, InstantInterval,
    budget::{OperationBudget, count},
};
use crate::{
    model::{Availability, DateRange, TimeWindow},
    temporal::{
        TemporalIssue, TemporalIssueKind, potentially_intersects, resolve_interval, weekday,
    },
};
use eutheto_types::{EntityId, Horizon, ScenarioSettings};
use jiff::{
    Span,
    civil::{Date, Time},
    tz::{Offset, TimeZone},
};

/// Original temporal normalization only. Callers independently decide rule/record applicability
/// and unavailable/available-only satisfaction; this value contains no candidate decision.
pub(crate) struct AvailabilityIntervals {
    pub query: Option<InstantInterval>,
    pub intervals: Vec<InstantInterval>,
}

pub(crate) fn date_range(
    range: DateRange,
    settings: &ScenarioSettings,
    owner: EntityId,
) -> Result<InstantInterval, AssignmentRuleError> {
    let interval = resolve_interval(
        range.start_date.to_datetime(Time::MIN),
        range.end_date_exclusive.to_datetime(Time::MIN),
        settings,
        owner,
    )?;
    Ok(InstantInterval {
        start: interval.starts_at.instant,
        end: interval.ends_at.instant,
    })
}

pub(crate) fn availability_intervals(
    availability: &Availability,
    shift: InstantInterval,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<AvailabilityIntervals, AssignmentRuleError> {
    budget.step()?;
    let owner = availability.id.as_entity_id();
    let zone = TimeZone::get(settings.time_zone.as_str()).map_err(|_| overflow(owner, None))?;
    // As in calendar queries, reject ambiguity only in a potentially relevant interval.
    if !potentially_intersects(
        availability
            .effective_range
            .start_date
            .to_datetime(Time::MIN),
        availability
            .effective_range
            .end_date_exclusive
            .to_datetime(Time::MIN),
        &zone,
        Horizon {
            start: shift.start,
            end: shift.end,
        },
        owner,
    )? {
        return Ok(AvailabilityIntervals {
            query: None,
            intervals: Vec::new(),
        });
    }
    let effective = date_range(availability.effective_range, settings, owner)?;
    let Some(query) = shift.intersection(effective) else {
        return Ok(AvailabilityIntervals {
            query: None,
            intervals: Vec::new(),
        });
    };
    let mut intervals = Vec::new();
    match &availability.time_window {
        TimeWindow::Instant { starts_at, ends_at } => {
            budget.expanded_interval()?;
            if let Some(interval) = query.intersection(InstantInterval {
                start: *starts_at,
                end: *ends_at,
            }) {
                retain_interval(&mut intervals, interval, budget)?;
            }
        }
        TimeWindow::Weekly { windows } => {
            for window in windows {
                budget.step()?;
                let (first, last) = weekly_start_bounds(query, window.end_day_offset, owner)?;
                let mut date = first;
                loop {
                    budget.step()?;
                    budget.steps(count(window.weekdays.len())?)?;
                    if window.weekdays.contains(&weekday(date)) {
                        let end_date = date
                            .checked_add(Span::new().days(i64::from(window.end_day_offset)))
                            .map_err(|_| overflow(owner, Some(date)))?;
                        let start = date.to_datetime(window.start_time);
                        let end = end_date.to_datetime(window.end_time);
                        if potentially_intersects(
                            start,
                            end,
                            &zone,
                            Horizon {
                                start: query.start,
                                end: query.end,
                            },
                            owner,
                        )? {
                            budget.expanded_interval()?;
                            let resolved = resolve_interval(start, end, settings, owner)?;
                            if let Some(interval) = query.intersection(InstantInterval {
                                start: resolved.starts_at.instant,
                                end: resolved.ends_at.instant,
                            }) {
                                retain_interval(&mut intervals, interval, budget)?;
                            }
                        }
                    }
                    if date == last {
                        break;
                    }
                    date = date
                        .checked_add(Span::new().days(1))
                        .map_err(|_| overflow(owner, Some(date)))?;
                }
            }
        }
    }
    budget.sort_work(intervals.len())?;
    intervals.sort_unstable();
    intervals.dedup();
    budget.check()?;
    Ok(AvailabilityIntervals {
        query: Some(query),
        intervals,
    })
}

/// Resolved endpoints equal intended civil time minus an offset within these bounds,
/// including compatible gap movement and whole-day transitions.
fn weekly_start_bounds(
    query: InstantInterval,
    end_day_offset: u8,
    owner: EntityId,
) -> Result<(Date, Date), AssignmentRuleError> {
    let first = Offset::UTC
        .to_datetime(query.start.as_timestamp())
        .checked_add(Span::new().seconds(i64::from(Offset::MIN.seconds())))
        .map_err(|_| overflow(owner, None))?
        .date()
        .checked_sub(Span::new().days(i64::from(end_day_offset)))
        .map_err(|_| overflow(owner, None))?;
    let last = Offset::UTC
        .to_datetime(query.end.as_timestamp())
        .checked_add(Span::new().seconds(i64::from(Offset::MAX.seconds())))
        .map_err(|_| overflow(owner, None))?
        .date();
    Ok((first, last))
}

fn retain_interval(
    intervals: &mut Vec<InstantInterval>,
    interval: InstantInterval,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let bytes = budget.measure(&(interval.start, interval.end))?;
    budget.reserve(1, 2, bytes)?;
    intervals.push(interval);
    Ok(())
}

fn overflow(owner: EntityId, date: Option<Date>) -> AssignmentRuleError {
    AssignmentRuleError::Temporal(TemporalIssue {
        kind: TemporalIssueKind::DateOverflow,
        entity_id: Some(owner),
        local_date: date,
    })
}
