use super::budget::OperationBudget;
use super::{AssignmentRuleError, InstantInterval};
use crate::model::Weekday;
use crate::temporal::{ResolvedShift, resolve_interval, weekday};
use eutheto_types::ScenarioSettings;
use jiff::{
    Span,
    civil::{Date, Time},
};

/// One local Saturday 00:00 through Monday 00:00 interval that positively overlaps a
/// source-resolved shift. The interval is retained in elapsed instant space so DST changes do
/// not turn a positive half-open overlap into a civil-time approximation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct WorkedWeekend {
    pub date: Date,
    pub interval: InstantInterval,
}

/// Candidate membership for one distinct local weekend.
pub(super) struct WeekendPopulation {
    pub date: Date,
    pub population: Vec<usize>,
}

/// One person's source-derived weekend cardinality family.
pub(super) struct WeekendPlan {
    pub maximum: u32,
    pub weekends: Vec<WeekendPopulation>,
}

/// Derive all weekend windows touched by one source-owned shift. The two-day look-back covers a
/// Sunday-start shift whose owning Saturday is not itself in the shift's local date range; output
/// is still source-bound because windows with no positive elapsed overlap are discarded.
pub(super) fn for_shift(
    shift: &ResolvedShift,
    settings: &ScenarioSettings,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<WorkedWeekend>, AssignmentRuleError> {
    let start_date = shift.interval.starts_at.local.as_datetime().date();
    let end_date = shift.interval.ends_at.local.as_datetime().date();
    let mut date = start_date
        .checked_sub(Span::new().days(2))
        .or_else(|_| start_date.checked_sub(Span::new().days(1)))
        .unwrap_or(start_date);
    let shift_interval = InstantInterval {
        start: shift.interval.starts_at.instant,
        end: shift.interval.ends_at.instant,
    };
    let mut result = Vec::new();
    while date <= end_date {
        budget.step()?;
        if matches!(weekday(date), Weekday::Saturday) {
            let end = date
                .checked_add(Span::new().days(2))
                .map_err(|_| overflow(shift, date))?;
            let resolved = resolve_interval(
                date.to_datetime(Time::MIN),
                end.to_datetime(Time::MIN),
                settings,
                shift.id.as_entity_id(),
            )?;
            let interval = InstantInterval {
                start: resolved.starts_at.instant,
                end: resolved.ends_at.instant,
            };
            if shift_interval.intersection(interval).is_some() {
                let bytes = budget.measure(&(date, interval.start, interval.end))?;
                budget.reserve(1, 1, bytes)?;
                result.push(WorkedWeekend { date, interval });
            }
        }
        if date == end_date {
            break;
        }
        date = date
            .checked_add(Span::new().days(1))
            .map_err(|_| overflow(shift, date))?;
    }
    Ok(result)
}

fn overflow(shift: &ResolvedShift, date: Date) -> AssignmentRuleError {
    AssignmentRuleError::Temporal(crate::temporal::TemporalIssue {
        kind: crate::temporal::TemporalIssueKind::DateOverflow,
        entity_id: Some(shift.id.as_entity_id()),
        local_date: Some(date),
        endpoint: None,
        occurrence_date: None,
        origin: None,
    })
}
