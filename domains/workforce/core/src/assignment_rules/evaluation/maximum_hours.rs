use super::super::{
    AssignmentRuleError, AssignmentRuleLimit, InstantInterval,
    budget::{OperationBudget, count},
    input::AssignmentInput,
    reporting::ReportingWindows,
};
use super::{HoursWitness, Selected, Summary, Witness, arithmetic, invalid, predicates};
use crate::{
    ids::{ShiftId, WorkCalendarId, WorkloadBucketId},
    model::{
        CalendarPeriod, OverlappingContribution, Scope, WindowMembership, WorkWindow,
        WorkforceEntity, WorkforceRule, WorkloadMeasurement,
    },
};
use eutheto_types::{CancellationToken, Horizon, Rfc3339Timestamp, ScenarioDocument};
use jiff::{SignedDuration, Span, civil::Date};
use std::collections::{BTreeMap, BTreeSet};

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NANOS_PER_MINUTE: i128 = 60 * NANOS_PER_SECOND;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Window {
    start: Rfc3339Timestamp,
    end: Rfc3339Timestamp,
}

impl Window {
    fn interval(self) -> InstantInterval {
        InstantInterval {
            start: self.start,
            end: self.end,
        }
    }
}

#[derive(Clone, Copy)]
struct Contribution {
    shift_id: ShiftId,
    reporting_date: Date,
    interval: InstantInterval,
    elapsed_nanos: i128,
    scheduled_nanos: i128,
}

#[derive(Clone, Copy)]
struct RollingEvent {
    at: Rfc3339Timestamp,
    index: usize,
    entering: bool,
}

/// Evaluates `MaximumHours` directly from the original selected pairs.
///
/// No compiler population, candidate pruning, compiled window, or IR decision is consulted.
pub(super) fn evaluate(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumHours {
        scope,
        bucket_id,
        window,
        maximum_minutes,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    let Some(WorkforceEntity::WorkloadBucket(bucket)) =
        input.domain.entities.get(&bucket_id.as_entity_id())
    else {
        return Err(invalid());
    };
    if bucket.measurement == WorkloadMeasurement::AssignmentCount
        || (bucket.overlapping_contribution == OverlappingContribution::Union
            && bucket.measurement != WorkloadMeasurement::ElapsedMinutes)
        || (matches!(
            *window,
            WorkWindow::Rolling {
                membership: WindowMembership::Intersection,
                ..
            } | WorkWindow::Calendar {
                membership: WindowMembership::Intersection,
                ..
            }
        ) && bucket.measurement != WorkloadMeasurement::ElapsedMinutes)
    {
        return Err(invalid());
    }
    let cap = cap_nanos(*maximum_minutes)?;
    let mut reporting_windows = ReportingWindows::new();
    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? {
            continue;
        }
        let contributions = collect_contributions(input, shifts, scope, *bucket_id, budget)?;
        if contributions.is_empty() {
            continue;
        }
        match *window {
            WorkWindow::Calendar {
                calendar_id,
                membership,
            } => evaluate_calendar(
                document,
                input,
                &contributions,
                calendar_id,
                membership,
                bucket.measurement,
                bucket.overlapping_contribution,
                *bucket_id,
                *maximum_minutes,
                cap,
                &mut reporting_windows,
                *person_id,
                summary,
                budget,
            )?,
            WorkWindow::Rolling {
                duration_minutes,
                membership,
            } => match membership {
                WindowMembership::StartInstant => evaluate_rolling_start(
                    &contributions,
                    duration_minutes,
                    bucket.measurement,
                    bucket.overlapping_contribution,
                    *bucket_id,
                    *maximum_minutes,
                    cap,
                    *person_id,
                    summary,
                    budget,
                )?,
                WindowMembership::Intersection => evaluate_rolling_intersection(
                    &contributions,
                    duration_minutes,
                    bucket.overlapping_contribution,
                    *bucket_id,
                    *maximum_minutes,
                    cap,
                    *person_id,
                    summary,
                    budget,
                )?,
                WindowMembership::ReportingDate => return Err(invalid()),
            },
        }
    }
    Ok(())
}

fn collect_contributions(
    input: &AssignmentInput,
    shifts: &[&crate::temporal::ResolvedShift],
    scope: &Scope,
    bucket_id: WorkloadBucketId,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Contribution>, AssignmentRuleError> {
    let mut contributions = Vec::new();
    for shift in shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if !predicates::shift_matches(shift, &metadata, scope, budget)? {
            continue;
        }
        budget.steps(count(metadata.assignment_type.workload_bucket_ids.len())?)?;
        if !metadata
            .assignment_type
            .workload_bucket_ids
            .contains(&bucket_id)
        {
            continue;
        }
        let contribution = Contribution {
            shift_id: shift.id,
            reporting_date: shift.reporting_date,
            interval: predicates::interval(shift),
            elapsed_nanos: duration_nanos(shift.interval.elapsed_duration())?,
            scheduled_nanos: duration_nanos(shift.interval.scheduled_duration())?,
        };
        // The vector is operation-local scratch, but is still charged before retention.
        budget.reserve(0, 1, 96)?;
        contributions.push(contribution);
    }
    Ok(contributions)
}

// Keep evaluator dimensions explicit; bundling them would add a one-use context type.
#[allow(clippy::too_many_arguments)]
fn evaluate_calendar(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    contributions: &[Contribution],
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    measurement: WorkloadMeasurement,
    overlapping: OverlappingContribution,
    bucket_id: WorkloadBucketId,
    maximum_minutes: u32,
    cap: i128,
    reporting_windows: &mut ReportingWindows,
    person_id: eutheto_types::PersonId,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let instant_windows = if membership == WindowMembership::ReportingDate {
        None
    } else {
        Some(resolve_calendar_ranges(
            document,
            input,
            contributions,
            calendar_id,
            membership,
            budget,
        )?)
    };
    let mut grouped = BTreeMap::<Window, Vec<Contribution>>::new();
    for contribution in contributions {
        budget.step()?;
        if membership == WindowMembership::ReportingDate {
            let windows = resolve_dates(
                document,
                input,
                calendar_id,
                contribution.reporting_date,
                contribution.reporting_date,
                reporting_windows,
                budget,
            )?;
            append_calendar_contribution(
                contribution,
                windows.iter(),
                membership,
                &mut grouped,
                budget,
            )?;
        } else {
            append_calendar_contribution(
                contribution,
                instant_windows.as_ref().ok_or_else(invalid)?.iter(),
                membership,
                &mut grouped,
                budget,
            )?;
        }
    }
    for (window, group) in grouped {
        budget.step()?;
        let actual = aggregate(&group, measurement, overlapping, budget)?;
        check_window(
            person_id,
            &group,
            window,
            bucket_id,
            Some(calendar_id),
            maximum_minutes,
            cap,
            actual,
            summary,
            budget,
        )?;
    }
    Ok(())
}

fn append_calendar_contribution<'a>(
    contribution: &Contribution,
    windows: impl IntoIterator<Item = &'a Window>,
    membership: WindowMembership,
    grouped: &mut BTreeMap<Window, Vec<Contribution>>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for window in windows {
        budget.step()?;
        let member = match membership {
            WindowMembership::ReportingDate => true,
            WindowMembership::StartInstant => {
                contribution.interval.start >= window.start
                    && contribution.interval.start < window.end
            }
            WindowMembership::Intersection => {
                contribution.interval.start < window.end && window.start < contribution.interval.end
            }
        };
        if !member {
            continue;
        }
        let contribution = if membership == WindowMembership::Intersection {
            let Some(interval) = contribution.interval.intersection(window.interval()) else {
                continue;
            };
            Contribution {
                shift_id: contribution.shift_id,
                reporting_date: contribution.reporting_date,
                elapsed_nanos: duration_nanos(
                    interval
                        .start
                        .as_timestamp()
                        .duration_until(interval.end.as_timestamp()),
                )?,
                scheduled_nanos: 0,
                interval,
            }
        } else {
            *contribution
        };
        if !grouped.contains_key(window) {
            budget.reserve(1, 1, 128)?;
        }
        budget.reserve(0, 1, 96)?;
        grouped.entry(*window).or_default().push(contribution);
    }
    Ok(())
}

fn resolve_calendar_ranges(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    contributions: &[Contribution],
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    budget: &mut OperationBudget<'_>,
) -> Result<BTreeSet<Window>, AssignmentRuleError> {
    let Some(WorkforceEntity::Calendar(calendar)) =
        input.domain.entities.get(&calendar_id.as_entity_id())
    else {
        return Err(invalid());
    };
    let scan_cost = match &calendar.period {
        CalendarPeriod::Custom { intervals } => count(intervals.len())?,
        _ => 1,
    };
    let cancellation = match budget.control() {
        Some(eutheto_types::OperationControl::Cancellation(token)) => token.clone(),
        _ => CancellationToken::new(),
    };
    let mut windows = BTreeSet::new();
    for contribution in contributions {
        budget.step()?;
        // A sparse instant query still validates/resolves all custom intervals.
        budget.steps(scan_cost)?;
        let (start, end) = match membership {
            WindowMembership::StartInstant => (
                contribution.interval.start,
                add_timestamp(contribution.interval.start, nano())?,
            ),
            WindowMembership::Intersection => {
                (contribution.interval.start, contribution.interval.end)
            }
            WindowMembership::ReportingDate => unreachable!(),
        };
        let range = Horizon::new(start, end).map_err(|_| arithmetic())?;
        let resolved = crate::temporal::resolve_calendar_for_calendar(
            document,
            calendar_id,
            calendar,
            range,
            &cancellation,
        )?;
        budget.check()?;
        budget.steps(count(resolved.len())?)?;
        for window in resolved {
            let window = Window {
                start: window.interval.starts_at.instant,
                end: window.interval.ends_at.instant,
            };
            if !windows.contains(&window) {
                budget.reserve(0, 1, 64)?;
                windows.insert(window);
            }
        }
    }
    Ok(windows)
}

fn resolve_dates(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    calendar_id: WorkCalendarId,
    start: Date,
    end: Date,
    reporting_windows: &mut ReportingWindows,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Window>, AssignmentRuleError> {
    let mut dates = Vec::new();
    let mut date = start;
    loop {
        budget.step()?;
        if let Some(period) =
            reporting_windows.resolve(document, input, calendar_id, date, budget)?
        {
            let window = Window {
                start: period.start,
                end: period.end,
            };
            if !dates.contains(&window) {
                budget.reserve(0, 1, 64)?;
                dates.push(window);
            }
        }
        if date >= end {
            break;
        }
        date = date
            .checked_add(Span::new().days(1))
            .map_err(|_| arithmetic())?;
    }
    Ok(dates)
}

// Keep evaluator dimensions explicit; bundling them would add a one-use context type.
#[allow(clippy::too_many_arguments)]
fn evaluate_rolling_start(
    contributions: &[Contribution],
    duration_minutes: u32,
    measurement: WorkloadMeasurement,
    overlapping: OverlappingContribution,
    bucket_id: WorkloadBucketId,
    maximum_minutes: u32,
    cap: i128,
    person_id: eutheto_types::PersonId,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let duration = SignedDuration::from_mins(i64::from(duration_minutes));
    let negative_duration = SignedDuration::from_mins(-i64::from(duration_minutes));
    let event_count = contributions.len().checked_mul(2).ok_or_else(arithmetic)?;
    let event_items = count(event_count)?;
    budget.reserve(
        0,
        event_items,
        event_items.checked_mul(32).ok_or_else(arithmetic)?,
    )?;
    let mut events = Vec::with_capacity(event_count);
    for (index, contribution) in contributions.iter().enumerate() {
        budget.step()?;
        let entry = add_timestamp(
            add_timestamp(contribution.interval.start, negative_duration)?,
            nano(),
        )?;
        let exit = add_timestamp(contribution.interval.start, nano())?;
        events.push(RollingEvent {
            at: entry,
            index,
            entering: true,
        });
        events.push(RollingEvent {
            at: exit,
            index,
            entering: false,
        });
    }
    budget.sort_work(events.len())?;
    events.sort_unstable_by_key(|event| (event.at, event.index, !event.entering));
    let active_items = count(contributions.len())?;
    budget.reserve(
        0,
        active_items,
        active_items.checked_mul(1).ok_or_else(arithmetic)?,
    )?;
    let mut active = vec![false; contributions.len()];
    let mut active_count = 0usize;
    let mut total = 0i128;
    let mut position = 0;
    while position < events.len() {
        budget.step()?;
        let at = events[position].at;
        while position < events.len() && events[position].at == at {
            budget.step()?;
            let event = events[position];
            let amount = amount(&contributions[event.index], measurement);
            if event.entering {
                if !active[event.index] {
                    active[event.index] = true;
                    active_count = active_count.checked_add(1).ok_or_else(arithmetic)?;
                    total = total.checked_add(amount).ok_or_else(arithmetic)?;
                }
            } else if active[event.index] {
                active[event.index] = false;
                active_count = active_count.checked_sub(1).ok_or_else(arithmetic)?;
                total = total.checked_sub(amount).ok_or_else(arithmetic)?;
            }
            position += 1;
        }
        if active_count == 0 {
            continue;
        }
        let actual = if overlapping == OverlappingContribution::Sum {
            total
        } else {
            aggregate_active_union(contributions, &active, budget)?
        };
        let end = add_timestamp(at, duration)?;
        let mut group = Vec::new();
        let first_shift = first_active_shift(contributions, &active, budget)?;
        budget.reserve(0, 1, 96)?;
        group.push(contributions[first_shift]);
        check_window(
            person_id,
            &group,
            Window { start: at, end },
            bucket_id,
            None,
            maximum_minutes,
            cap,
            actual,
            summary,
            budget,
        )?;
    }
    Ok(())
}

// Keep evaluator dimensions explicit; bundling them would add a one-use context type.
#[allow(clippy::too_many_arguments)]
fn evaluate_rolling_intersection(
    contributions: &[Contribution],
    duration_minutes: u32,
    overlapping: OverlappingContribution,
    bucket_id: WorkloadBucketId,
    maximum_minutes: u32,
    cap: i128,
    person_id: eutheto_types::PersonId,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let duration = SignedDuration::from_mins(i64::from(duration_minutes));
    let negative_duration = SignedDuration::from_mins(-i64::from(duration_minutes));
    let candidate_count = contributions.len().checked_mul(12).ok_or_else(arithmetic)?;
    budget.reserve(0, count(candidate_count)?, 64)?;
    let mut candidates = Vec::with_capacity(candidate_count);
    for contribution in contributions {
        budget.step()?;
        for boundary in [contribution.interval.start, contribution.interval.end] {
            push_candidate(&mut candidates, boundary, budget)?;
            push_candidate(&mut candidates, add_timestamp(boundary, nano())?, budget)?;
            push_candidate(
                &mut candidates,
                add_timestamp(boundary, SignedDuration::from_nanos(-1))?,
                budget,
            )?;
            let shifted = add_timestamp(boundary, negative_duration)?;
            push_candidate(&mut candidates, shifted, budget)?;
            push_candidate(&mut candidates, add_timestamp(shifted, nano())?, budget)?;
            push_candidate(
                &mut candidates,
                add_timestamp(shifted, SignedDuration::from_nanos(-1))?,
                budget,
            )?;
        }
    }
    budget.sort_work(candidates.len())?;
    candidates.sort_unstable();
    candidates.dedup();
    for start in candidates {
        budget.step()?;
        let end = add_timestamp(start, duration)?;
        let Some(actual) = aggregate_intersection(contributions, start, end, overlapping, budget)?
        else {
            continue;
        };
        let mut group = Vec::new();
        let mut first = None;
        for contribution in contributions {
            budget.step()?;
            if contribution.interval.start < end && start < contribution.interval.end {
                first = Some(contribution.shift_id);
                break;
            }
        }
        let Some(first_shift) = first else {
            continue;
        };
        let contribution = contributions
            .iter()
            .find(|contribution| contribution.shift_id == first_shift)
            .copied()
            .ok_or_else(invalid)?;
        budget.reserve(0, 1, 96)?;
        group.push(contribution);
        check_window(
            person_id,
            &group,
            Window { start, end },
            bucket_id,
            None,
            maximum_minutes,
            cap,
            actual,
            summary,
            budget,
        )?;
    }
    Ok(())
}

fn aggregate_intersection(
    contributions: &[Contribution],
    start: Rfc3339Timestamp,
    end: Rfc3339Timestamp,
    overlapping: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<i128>, AssignmentRuleError> {
    let mut total = 0i128;
    let mut current: Option<InstantInterval> = None;
    let mut merged = 0i128;
    for contribution in contributions {
        budget.step()?;
        let Some(interval) = contribution
            .interval
            .intersection(InstantInterval { start, end })
        else {
            continue;
        };
        if overlapping == OverlappingContribution::Sum {
            total = total
                .checked_add(duration_nanos(
                    interval
                        .start
                        .as_timestamp()
                        .duration_until(interval.end.as_timestamp()),
                )?)
                .ok_or_else(arithmetic)?;
            continue;
        }
        if let Some(existing) = current {
            if interval.start <= existing.end {
                if interval.end > existing.end {
                    current = Some(InstantInterval {
                        start: existing.start,
                        end: interval.end,
                    });
                }
            } else {
                merged = merged
                    .checked_add(duration_nanos(
                        existing
                            .start
                            .as_timestamp()
                            .duration_until(existing.end.as_timestamp()),
                    )?)
                    .ok_or_else(arithmetic)?;
                current = Some(interval);
            }
        } else {
            current = Some(interval);
        }
    }
    if overlapping == OverlappingContribution::Sum {
        return Ok((total != 0).then_some(total));
    }
    if let Some(existing) = current {
        merged = merged
            .checked_add(duration_nanos(
                existing
                    .start
                    .as_timestamp()
                    .duration_until(existing.end.as_timestamp()),
            )?)
            .ok_or_else(arithmetic)?;
    }
    Ok((current.is_some()).then_some(merged))
}

fn aggregate_active_union(
    contributions: &[Contribution],
    active: &[bool],
    budget: &mut OperationBudget<'_>,
) -> Result<i128, AssignmentRuleError> {
    let mut current: Option<InstantInterval> = None;
    let mut total = 0i128;
    for (index, contribution) in contributions.iter().enumerate() {
        budget.step()?;
        if !active[index] {
            continue;
        }
        if let Some(existing) = current {
            if contribution.interval.start <= existing.end {
                if contribution.interval.end > existing.end {
                    current = Some(InstantInterval {
                        start: existing.start,
                        end: contribution.interval.end,
                    });
                }
            } else {
                total = total
                    .checked_add(duration_nanos(
                        existing
                            .start
                            .as_timestamp()
                            .duration_until(existing.end.as_timestamp()),
                    )?)
                    .ok_or_else(arithmetic)?;
                current = Some(contribution.interval);
            }
        } else {
            current = Some(contribution.interval);
        }
    }
    if let Some(existing) = current {
        total = total
            .checked_add(duration_nanos(
                existing
                    .start
                    .as_timestamp()
                    .duration_until(existing.end.as_timestamp()),
            )?)
            .ok_or_else(arithmetic)?;
    }
    Ok(total)
}

fn aggregate(
    contributions: &[Contribution],
    measurement: WorkloadMeasurement,
    overlapping: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<i128, AssignmentRuleError> {
    if overlapping == OverlappingContribution::Sum {
        let mut total = 0i128;
        for contribution in contributions {
            budget.step()?;
            total = total
                .checked_add(amount(contribution, measurement))
                .ok_or_else(arithmetic)?;
        }
        return Ok(total);
    }
    aggregate_union(contributions, budget)
}

fn aggregate_union(
    contributions: &[Contribution],
    budget: &mut OperationBudget<'_>,
) -> Result<i128, AssignmentRuleError> {
    let mut current: Option<InstantInterval> = None;
    let mut total = 0i128;
    for contribution in contributions {
        budget.step()?;
        if let Some(existing) = current {
            if contribution.interval.start <= existing.end {
                if contribution.interval.end > existing.end {
                    current = Some(InstantInterval {
                        start: existing.start,
                        end: contribution.interval.end,
                    });
                }
            } else {
                total = total
                    .checked_add(duration_nanos(
                        existing
                            .start
                            .as_timestamp()
                            .duration_until(existing.end.as_timestamp()),
                    )?)
                    .ok_or_else(arithmetic)?;
                current = Some(contribution.interval);
            }
        } else {
            current = Some(contribution.interval);
        }
    }
    if let Some(existing) = current {
        total = total
            .checked_add(duration_nanos(
                existing
                    .start
                    .as_timestamp()
                    .duration_until(existing.end.as_timestamp()),
            )?)
            .ok_or_else(arithmetic)?;
    }
    Ok(total)
}

// Keep evaluator dimensions explicit; bundling them would add a one-use context type.
#[allow(clippy::too_many_arguments)]
fn check_window(
    person_id: eutheto_types::PersonId,
    group: &[Contribution],
    window: Window,
    bucket_id: WorkloadBucketId,
    calendar_id: Option<WorkCalendarId>,
    maximum_minutes: u32,
    cap: i128,
    actual_nanos: i128,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let actual = signed_duration(actual_nanos)?;
    let shift_id = group
        .iter()
        .map(|contribution| contribution.shift_id)
        .min()
        .ok_or_else(invalid)?;
    let (owner, owner_kind) = calendar_id
        .map_or((bucket_id.as_entity_id(), "workload_bucket"), |calendar| {
            (calendar.as_entity_id(), "calendar")
        });
    let mut witness = Witness::pair(
        crate::model::AssignmentPair {
            person_id,
            shift_id,
        },
        owner,
        owner_kind,
        "maximum_hours",
    );
    witness.interval = Some(window.interval());
    witness.period = Some((window.start, window.end));
    witness.hours = Some(HoursWitness {
        maximum_minutes,
        actual,
        bucket: bucket_id,
        calendar: calendar_id,
    });
    summary.predicate(actual_nanos > cap, &witness, budget)
}

fn first_active_shift(
    contributions: &[Contribution],
    active: &[bool],
    budget: &mut OperationBudget<'_>,
) -> Result<usize, AssignmentRuleError> {
    let mut first = None;
    for (index, contribution) in contributions.iter().enumerate() {
        budget.step()?;
        if active[index]
            && first.is_none_or(|current: usize| {
                contribution.shift_id < contributions[current].shift_id
            })
        {
            first = Some(index);
        }
    }
    first.ok_or_else(invalid)
}

fn push_candidate(
    candidates: &mut Vec<Rfc3339Timestamp>,
    candidate: Rfc3339Timestamp,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.step()?;
    candidates.push(candidate);
    Ok(())
}

fn nano() -> SignedDuration {
    SignedDuration::from_nanos(1)
}

fn add_timestamp(
    timestamp: Rfc3339Timestamp,
    duration: SignedDuration,
) -> Result<Rfc3339Timestamp, AssignmentRuleError> {
    timestamp
        .as_timestamp()
        .checked_add(duration)
        .map(Rfc3339Timestamp::from_timestamp)
        .map_err(|_| arithmetic())
}

fn amount(contribution: &Contribution, measurement: WorkloadMeasurement) -> i128 {
    match measurement {
        WorkloadMeasurement::ElapsedMinutes => contribution.elapsed_nanos,
        WorkloadMeasurement::ScheduledMinutes => contribution.scheduled_nanos,
        WorkloadMeasurement::AssignmentCount => 0,
    }
}

fn duration_nanos(duration: SignedDuration) -> Result<i128, AssignmentRuleError> {
    i128::from(duration.as_secs())
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|seconds| seconds.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or_else(arithmetic)
}

fn cap_nanos(maximum_minutes: u32) -> Result<i128, AssignmentRuleError> {
    i128::from(maximum_minutes)
        .checked_mul(NANOS_PER_MINUTE)
        .ok_or_else(arithmetic)
}

fn signed_duration(nanos: i128) -> Result<SignedDuration, AssignmentRuleError> {
    let seconds = nanos / NANOS_PER_SECOND;
    let remainder = nanos % NANOS_PER_SECOND;
    let seconds = i64::try_from(seconds)
        .map_err(|_| AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord))?;
    let remainder = i32::try_from(remainder).map_err(|_| arithmetic())?;
    Ok(SignedDuration::new(seconds, remainder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_duration_preserves_fractional_negative_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = signed_duration(-1_500_000_001)?;
        assert_eq!(value.as_secs(), -1);
        assert_eq!(value.subsec_nanos(), -500_000_001);
        Ok(())
    }

    #[test]
    fn signed_duration_rejects_values_outside_evidence_seconds()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = i128::from(i64::MAX)
            .checked_add(1)
            .ok_or("test value")?
            .checked_mul(NANOS_PER_SECOND)
            .ok_or("test value")?;
        assert_eq!(
            signed_duration(value),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord
            ))
        );
        Ok(())
    }
    fn timestamp(value: &str) -> Result<Rfc3339Timestamp, Box<dyn std::error::Error>> {
        Ok(value.parse()?)
    }

    fn contribution(
        index: u32,
        start: &str,
        end: &str,
        scheduled_nanos: i128,
    ) -> Result<Contribution, Box<dyn std::error::Error>> {
        let start = timestamp(start)?;
        let end = timestamp(end)?;
        Ok(Contribution {
            shift_id: crate::test_support::id(index).parse()?,
            reporting_date: "2026-01-01".parse()?,
            interval: InstantInterval { start, end },
            elapsed_nanos: duration_nanos(start.as_timestamp().duration_until(end.as_timestamp()))?,
            scheduled_nanos,
        })
    }

    #[test]
    fn sum_and_union_keep_overlapping_elapsed_intervals_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let values = [
            contribution(
                1,
                "2026-01-01T00:00:00Z",
                "2026-01-01T01:00:00Z",
                3_600 * NANOS_PER_SECOND,
            )?,
            contribution(
                2,
                "2026-01-01T00:30:00Z",
                "2026-01-01T01:30:00Z",
                3_600 * NANOS_PER_SECOND,
            )?,
        ];
        let mut budget = OperationBudget::evaluation(None);
        assert_eq!(
            aggregate(
                &values,
                WorkloadMeasurement::ElapsedMinutes,
                OverlappingContribution::Sum,
                &mut budget,
            )?,
            7_200 * NANOS_PER_SECOND
        );
        assert_eq!(
            aggregate_union(&values, &mut budget)?,
            5_400 * NANOS_PER_SECOND
        );
        Ok(())
    }

    #[test]
    fn intersection_preserves_fractional_nanoseconds() -> Result<(), Box<dyn std::error::Error>> {
        let values = [contribution(
            1,
            "2026-01-01T00:00:00.500Z",
            "2026-01-01T00:01:00.500Z",
            60 * NANOS_PER_SECOND,
        )?];
        let mut budget = OperationBudget::evaluation(None);
        assert_eq!(
            aggregate_intersection(
                &values,
                timestamp("2026-01-01T00:00:30.750Z")?,
                timestamp("2026-01-01T00:01:00.750Z")?,
                OverlappingContribution::Sum,
                &mut budget,
            )?,
            Some(29_750_000_000)
        );
        Ok(())
    }

    #[test]
    fn rolling_start_checks_negative_credit_exit() -> Result<(), Box<dyn std::error::Error>> {
        let values = [
            contribution(
                1,
                "2026-01-01T00:30:00Z",
                "2026-01-01T01:30:00Z",
                -30 * 60 * NANOS_PER_SECOND,
            )?,
            contribution(
                2,
                "2026-01-01T00:45:00Z",
                "2026-01-01T01:45:00Z",
                60 * 60 * NANOS_PER_SECOND,
            )?,
        ];
        let person = crate::test_support::id(1).parse()?;
        let bucket = crate::test_support::id(100).parse()?;
        let mut summary = Summary::default();
        let mut budget = OperationBudget::evaluation(None);
        evaluate_rolling_start(
            &values,
            60,
            WorkloadMeasurement::ScheduledMinutes,
            OverlappingContribution::Sum,
            bucket,
            40,
            cap_nanos(40)?,
            person,
            &mut summary,
            &mut budget,
        )?;
        assert!(summary.checked >= 3);
        assert!(summary.violations > 0);
        Ok(())
    }

    fn maximum_rule(
        bucket_id: u32,
        window: &serde_json::Value,
        maximum_minutes: u32,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": crate::test_support::id(20),
            "kind": "maximumHours",
            "active": true,
            "strength": "required",
            "scope": {"people": {"kind": "all"}},
            "bucketId": crate::test_support::id(bucket_id),
            "window": window,
            "maximumMinutes": maximum_minutes,
        })
    }

    fn evaluate_fixture(
        mut document: ScenarioDocument,
        rule: serde_json::Value,
    ) -> Result<eutheto_domain_ir::RuleEvaluation, Box<dyn std::error::Error>> {
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        document.domain.locked_assignments.clear();
        let rule_id: eutheto_types::RuleId = crate::test_support::id(20).parse()?;
        document.domain.rules.insert(rule_id, rule);
        let result = crate::assignment_rules::evaluate_assignment_rules(
            &document,
            &[crate::model::AssignmentPair {
                person_id: crate::test_support::id(1).parse()?,
                shift_id: crate::test_support::id(8).parse()?,
            }],
            None,
        )?;
        result
            .evaluations
            .into_iter()
            .find(|evaluation| evaluation.rule_id == rule_id)
            .ok_or_else(|| "maximum-hours evaluation missing".into())
    }

    #[test]
    fn calendar_reporting_date_counts_full_shift_even_outside_owner_instants()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(2).parse()?)
            .ok_or("calendar")?["period"] = serde_json::json!({
            "kind": "custom",
            "intervals": [{
                "startsAt": "2026-11-01T00:00:00",
                "endsAt": "2026-11-01T01:00:00"
            }]
        });
        let window = serde_json::json!({
            "kind": "calendar",
            "calendarId": crate::test_support::id(2),
            "membership": "reportingDate"
        });
        let over = evaluate_fixture(document.clone(), maximum_rule(3, &window, 119))?;
        assert!(!over.satisfied);
        let at_cap = evaluate_fixture(document, maximum_rule(3, &window, 120))?;
        assert!(at_cap.satisfied);
        Ok(())
    }

    #[test]
    fn calendar_intersection_clips_dst_shift_to_whole_period()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(2).parse()?)
            .ok_or("calendar")?["period"] = serde_json::json!({
            "kind": "custom",
            "intervals": [{
                "startsAt": "2026-11-01T02:00:00",
                "endsAt": "2026-11-01T03:00:00"
            }]
        });
        let window = serde_json::json!({
            "kind": "calendar",
            "calendarId": crate::test_support::id(2),
            "membership": "intersection"
        });
        let at_cap = evaluate_fixture(document.clone(), maximum_rule(3, &window, 30))?;
        assert!(at_cap.satisfied);
        let over = evaluate_fixture(document, maximum_rule(3, &window, 29))?;
        assert!(!over.satisfied);
        Ok(())
    }

    #[test]
    fn bucket_membership_is_required_and_scheduled_measurement_uses_local_duration()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(4).parse()?)
            .ok_or("assignment type")?["workloadBucketIds"] = serde_json::json!([]);
        let window = serde_json::json!({
            "kind": "calendar",
            "calendarId": crate::test_support::id(2),
            "membership": "reportingDate"
        });
        let no_bucket = evaluate_fixture(document, maximum_rule(3, &window, 0))?;
        assert!(no_bucket.satisfied);
        assert_eq!(
            no_bucket
                .observed
                .get(&eutheto_domain_ir::VerificationFactId::new(
                    super::super::CHECKED
                )?),
            Some(&eutheto_domain_ir::VerificationValue::Integer(0))
        );

        let mut scheduled = crate::test_support::fixture()?;
        scheduled.domain.entities.insert(
            crate::test_support::id(30).parse()?,
            serde_json::json!({
                "kind": "workloadBucket",
                "id": crate::test_support::id(30),
                "name": "Scheduled",
                "measurement": "scheduledMinutes",
                "overlappingContribution": "sum"
            }),
        );
        scheduled
            .domain
            .entities
            .get_mut(&crate::test_support::id(4).parse()?)
            .ok_or("assignment type")?["workloadBucketIds"] =
            serde_json::json!([crate::test_support::id(30)]);
        let scheduled = evaluate_fixture(scheduled, maximum_rule(30, &window, 60))?;
        assert!(scheduled.satisfied);
        Ok(())
    }
    #[test]
    fn calendar_nonmidnight_start_uses_previous_period_for_start_and_intersection()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(2).parse()?)
            .ok_or("calendar")?["period"] =
            serde_json::json!({"kind":"day","startTime":"08:00:00"});
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(8).parse()?)
            .ok_or("shift")?["startsAt"] = serde_json::json!({
            "instant":"2026-11-01T12:00:00Z",
            "local":"2026-11-01T07:00:00",
            "offsetSeconds":-18000
        });
        document
            .domain
            .entities
            .get_mut(&crate::test_support::id(8).parse()?)
            .ok_or("shift")?["endsAt"] = serde_json::json!({
            "instant":"2026-11-01T14:00:00Z",
            "local":"2026-11-01T09:00:00",
            "offsetSeconds":-18000
        });
        let intersection = serde_json::json!({
            "kind":"calendar",
            "calendarId":crate::test_support::id(2),
            "membership":"intersection"
        });
        let clipped = evaluate_fixture(document.clone(), maximum_rule(3, &intersection, 59))?;
        assert!(!clipped.satisfied);

        let start = serde_json::json!({
            "kind":"calendar",
            "calendarId":crate::test_support::id(2),
            "membership":"startInstant"
        });
        let started = evaluate_fixture(document, maximum_rule(3, &start, 119))?;
        assert!(!started.satisfied);
        Ok(())
    }

    #[test]
    fn sparse_custom_calendar_charges_every_scanned_interval()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
        let calendar_id: WorkCalendarId = crate::test_support::id(2).parse()?;
        let selected = [contribution(
            8,
            "2026-11-01T05:30:00Z",
            "2026-11-01T07:30:00Z",
            120 * NANOS_PER_MINUTE,
        )?];
        let mut setup = OperationBudget::evaluation(None);
        let regular = AssignmentInput::new(&document, &mut setup)?;
        let mut budget = OperationBudget::evaluation(None);
        budget.steps(crate::assignment_rules::MAX_WORK_STEPS - 500)?;
        assert!(
            !resolve_calendar_ranges(
                &document,
                &regular,
                &selected,
                calendar_id,
                WindowMembership::StartInstant,
                &mut budget,
            )?
            .is_empty()
        );

        let base: jiff::civil::DateTime = "2026-10-31T00:00:00".parse()?;
        let intervals = (0..2_000_i64)
            .map(|second| {
                let starts_at = base.checked_add(Span::new().seconds(second))?;
                let ends_at = base.checked_add(Span::new().seconds(second + 1))?;
                Ok::<_, jiff::Error>(serde_json::json!({
                    "startsAt": starts_at.to_string(),
                    "endsAt": ends_at.to_string(),
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        document
            .domain
            .entities
            .get_mut(&calendar_id.as_entity_id())
            .ok_or("calendar")?["period"] =
            serde_json::json!({"kind":"custom","intervals": intervals});
        let mut setup = OperationBudget::evaluation(None);
        let custom = AssignmentInput::new(&document, &mut setup)?;
        let mut budget = OperationBudget::evaluation(None);
        budget.steps(crate::assignment_rules::MAX_WORK_STEPS - 500)?;
        assert_eq!(
            resolve_calendar_ranges(
                &document,
                &custom,
                &selected,
                calendar_id,
                WindowMembership::StartInstant,
                &mut budget,
            ),
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::WorkSteps
            ))
        );
        Ok(())
    }
}
