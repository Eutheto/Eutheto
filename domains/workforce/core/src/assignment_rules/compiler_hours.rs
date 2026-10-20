use super::{
    AssignmentInput, AssignmentPair, AssignmentRuleError, AssignmentRuleLimit,
    DomainValidationReport, OperationBudget, Plan, ResourceRef, ValidationIssue, WorkforceEntity,
    WorkforceRule, add, count, person_scope, shift_scope, within,
};
use crate::{
    ids::{ShiftId, WorkCalendarId, WorkloadBucketId},
    model::{
        AssignmentLock, OverlappingContribution, WindowMembership, WorkWindow, WorkloadMeasurement,
    },
    temporal::{CalendarWindow, ResolvedShift, resolve_calendar_for_calendar},
};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{
    EntityId, Horizon, PersonId, Rfc3339Timestamp, RuleId, ScenarioDocument, ValidationSeverity,
};
use jiff::SignedDuration;
use std::collections::{BTreeMap, BTreeSet};

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const MAX_NANOS_REMAINDER: i64 = 999_999_999;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HoursWindow {
    pub calendar_id: Option<WorkCalendarId>,
    pub start: Rfc3339Timestamp,
    pub end: Rfc3339Timestamp,
    pub duration_minutes: Option<u32>,
    pub membership: WindowMembership,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct HoursTerm {
    pub index: usize,
    pub seconds: i64,
    pub remainder: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HoursSegment {
    pub start: Rfc3339Timestamp,
    pub end: Rfc3339Timestamp,
    pub seconds: i64,
    pub remainder: i64,
    pub members: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum HoursAggregate {
    Sum { terms: Vec<HoursTerm> },
    Union { segments: Vec<HoursSegment> },
}

impl HoursAggregate {
    fn has_fractional(&self) -> bool {
        match self {
            Self::Sum { terms } => terms.iter().any(|term| term.remainder != 0),
            Self::Union { segments } => segments.iter().any(|segment| segment.remainder != 0),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HoursPlanKind {
    MaximumHours,
    WorkloadQuota,
}

#[derive(Clone, Copy)]
enum HoursHelperFact {
    Carry,
    Fractional,
    UnionSegment,
}

const fn helper_message(kind: HoursPlanKind, helper: HoursHelperFact) -> &'static str {
    match (kind, helper) {
        (HoursPlanKind::MaximumHours, HoursHelperFact::Carry) => {
            "official.workforce.maximum_hours.carry"
        }
        (HoursPlanKind::MaximumHours, HoursHelperFact::Fractional) => {
            "official.workforce.maximum_hours.fractional"
        }
        (HoursPlanKind::MaximumHours, HoursHelperFact::UnionSegment) => {
            "official.workforce.maximum_hours.union_segment"
        }
        (HoursPlanKind::WorkloadQuota, HoursHelperFact::Carry) => {
            "official.workforce.workload_quota.carry"
        }
        (HoursPlanKind::WorkloadQuota, HoursHelperFact::Fractional) => {
            "official.workforce.workload_quota.fractional"
        }
        (HoursPlanKind::WorkloadQuota, HoursHelperFact::UnionSegment) => {
            "official.workforce.workload_quota.union_segment"
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HoursPlan {
    pub rule: RuleId,
    pub person: PersonId,
    pub bucket_id: WorkloadBucketId,
    pub measurement: WorkloadMeasurement,
    pub contribution: OverlappingContribution,
    pub minimum_minutes: u32,
    pub maximum_minutes: Option<u32>,
    pub kind: HoursPlanKind,
    pub window: HoursWindow,
    pub aggregate: HoursAggregate,
}
fn requires_lower(hours: &HoursPlan) -> bool {
    hours.kind == HoursPlanKind::WorkloadQuota || hours.minimum_minutes > 0
}

#[derive(Clone, Copy)]
pub(super) struct Member<'a> {
    pub(super) index: usize,
    pub(super) shift: &'a ResolvedShift,
}

#[derive(Clone, Copy)]
pub(super) struct IntervalMember {
    pub(super) index: usize,
    pub(super) start: Rfc3339Timestamp,
    pub(super) end: Rfc3339Timestamp,
    pub(super) scheduled: SignedDuration,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct WindowKey {
    pub(super) calendar_id: Option<WorkCalendarId>,
    pub(super) start: Rfc3339Timestamp,
    pub(super) end: Rfc3339Timestamp,
    pub(super) duration_minutes: Option<u32>,
}

impl WindowKey {
    pub(super) fn window(self, membership: WindowMembership) -> HoursWindow {
        HoursWindow {
            calendar_id: self.calendar_id,
            start: self.start,
            end: self.end,
            duration_minutes: self.duration_minutes,
            membership,
        }
    }
}

/// Plan every active `MaximumHours` window from the original candidate graph. Calendar queries are
/// deliberately wider than the scenario horizon: a shift start in the horizon owns its complete
/// interval, and an intersection query must retain a calendar period reached by its tail.
#[allow(clippy::too_many_lines)]
pub(super) fn plan(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    rule: &WorkforceRule,
    plan: &mut Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumHours {
        id,
        active: true,
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
    if bucket.measurement == WorkloadMeasurement::AssignmentCount {
        return Err(invalid());
    }
    let mut any = false;
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !person_scope(scope, person, budget)? {
            continue;
        }
        let mut members = Vec::new();
        for (index, pair) in candidates.iter().enumerate() {
            budget.step()?;
            if pair.person_id != *person_id {
                continue;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            let metadata = input.metadata(shift)?;
            if !shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            budget.steps(count(metadata.assignment_type.workload_bucket_ids.len())?)?;
            if !metadata
                .assignment_type
                .workload_bucket_ids
                .contains(bucket_id)
            {
                continue;
            }
            budget.reserve(0, 1, 64)?;
            members.push(Member { index, shift });
        }
        if members.is_empty() {
            continue;
        }
        let windows = match *window {
            WorkWindow::Calendar {
                calendar_id,
                membership,
            } => calendar_windows(
                document,
                input,
                &members,
                calendar_id,
                membership,
                bucket.measurement,
                bucket.overlapping_contribution,
                budget,
            )?,
            WorkWindow::Rolling {
                duration_minutes,
                membership,
            } => rolling_windows(
                &members,
                duration_minutes,
                membership,
                bucket.measurement,
                bucket.overlapping_contribution,
                budget,
            )?,
        };
        for (window, aggregate) in windows {
            if aggregate_empty(&aggregate) {
                continue;
            }
            validate_aggregate(&aggregate, limits, 0, Some(*maximum_minutes))?;
            budget.reserve(1, 8, 768)?;
            plan.hours.push(HoursPlan {
                rule: *id,
                person: *person_id,
                bucket_id: *bucket_id,
                measurement: bucket.measurement,
                contribution: bucket.overlapping_contribution,
                minimum_minutes: 0,
                maximum_minutes: Some(*maximum_minutes),
                kind: HoursPlanKind::MaximumHours,
                window,
                aggregate,
            });
            any = true;
        }
    }
    if any {
        plan.parents.insert(*id, "maximum_hours");
    }
    Ok(())
}

/// Diagnose only observed hard-lock excesses for active `MaximumHours` rules. This mirrors the
/// planner's membership and clipping recipes but evaluates every locked pair as selected; it never
/// turns a lock conflict into a solver-feasibility claim.
// Keep one source-rule's lock grouping, window selection and finding together.
#[allow(clippy::too_many_lines)]
pub(super) fn locked_findings(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    locks: &[&AssignmentLock],
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for rule in input.domain.rules.values() {
        budget.step()?;
        let WorkforceRule::MaximumHours {
            id,
            active: true,
            scope,
            bucket_id,
            window,
            maximum_minutes,
            ..
        } = rule
        else {
            continue;
        };
        let Some(WorkforceEntity::WorkloadBucket(bucket)) =
            input.domain.entities.get(&bucket_id.as_entity_id())
        else {
            return Err(invalid());
        };
        if bucket.measurement == WorkloadMeasurement::AssignmentCount {
            return Err(invalid());
        }
        for person_id in &input.people {
            budget.step()?;
            let person = input.person(*person_id).ok_or_else(invalid)?;
            if !person_scope(scope, person, budget)? {
                continue;
            }
            let mut shifts = BTreeMap::<ShiftId, &ResolvedShift>::new();
            for lock in locks {
                budget.step()?;
                if lock.person_id != *person_id {
                    continue;
                }
                let Some(shift) = input.shift(lock.shift_id) else {
                    continue;
                };
                let metadata = input.metadata(shift)?;
                if !shift_scope(scope, shift, &metadata, budget)? {
                    continue;
                }
                if !metadata
                    .assignment_type
                    .workload_bucket_ids
                    .contains(bucket_id)
                {
                    continue;
                }
                shifts.insert(lock.shift_id, shift);
            }
            if shifts.is_empty() {
                continue;
            }
            let members: Vec<_> = shifts
                .values()
                .enumerate()
                .map(|(index, shift)| Member { index, shift })
                .collect();
            let windows = match *window {
                WorkWindow::Calendar {
                    calendar_id,
                    membership,
                } => calendar_windows(
                    document,
                    input,
                    &members,
                    calendar_id,
                    membership,
                    bucket.measurement,
                    bucket.overlapping_contribution,
                    budget,
                )?,
                WorkWindow::Rolling {
                    duration_minutes,
                    membership,
                } => rolling_windows(
                    &members,
                    duration_minutes,
                    membership,
                    bucket.measurement,
                    bucket.overlapping_contribution,
                    budget,
                )?,
            };
            let cap = i128::from(*maximum_minutes)
                .checked_mul(60)
                .and_then(|seconds| seconds.checked_mul(NANOS_PER_SECOND))
                .ok_or_else(arithmetic)?;
            for (hours_window, aggregate) in windows {
                budget.step()?;
                let actual = aggregate_nanos(&aggregate)?;
                if actual <= cap {
                    continue;
                }
                let seconds = actual.div_euclid(NANOS_PER_SECOND);
                let remainder = actual.rem_euclid(NANOS_PER_SECOND);
                budget.reserve(1, 8, 768)?;
                report.issues.push(ValidationIssue {
                    code: "official.workforce.hard_locked_maximum_hours".to_owned(),
                    severity: ValidationSeverity::Error,
                    message: format!(
                        "Hard locks for person {person_id} contain {seconds}.{remainder:09}s in {:?} {:?} window {}–{}, exceeding MaximumHours rule {id} maximum {maximum_minutes} minutes. This is not a solver feasibility result.",
                        bucket.measurement,
                        bucket.overlapping_contribution,
                        hours_window.start,
                        hours_window.end,
                    ),
                    field_path: Some(format!("domain.rules.{id}.maximumMinutes")),
                    resource: Some(ResourceRef::Rule(*id)),
                });
            }
        }
    }
    Ok(())
}

fn aggregate_nanos(aggregate: &HoursAggregate) -> Result<i128, AssignmentRuleError> {
    let mut total = 0_i128;
    match aggregate {
        HoursAggregate::Sum { terms } => {
            for term in terms {
                total = total
                    .checked_add(
                        i128::from(term.seconds)
                            .checked_mul(NANOS_PER_SECOND)
                            .and_then(|value| value.checked_add(i128::from(term.remainder)))
                            .ok_or_else(arithmetic)?,
                    )
                    .ok_or_else(arithmetic)?;
            }
        }
        HoursAggregate::Union { segments } => {
            for segment in segments {
                total = total
                    .checked_add(
                        i128::from(segment.seconds)
                            .checked_mul(NANOS_PER_SECOND)
                            .and_then(|value| value.checked_add(i128::from(segment.remainder)))
                            .ok_or_else(arithmetic)?,
                    )
                    .ok_or_else(arithmetic)?;
            }
        }
    }
    Ok(total)
}

fn aggregate_empty(aggregate: &HoursAggregate) -> bool {
    match aggregate {
        HoursAggregate::Sum { terms } => terms.is_empty(),
        HoursAggregate::Union { segments } => segments.is_empty(),
    }
}

// A calendar window needs both source bindings and the bucket's independent measurement policy.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn calendar_windows(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    members: &[Member<'_>],
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    measurement: WorkloadMeasurement,
    contribution: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(HoursWindow, HoursAggregate)>, AssignmentRuleError> {
    let Some(WorkforceEntity::Calendar(calendar)) =
        input.domain.entities.get(&calendar_id.as_entity_id())
    else {
        return Err(invalid());
    };
    let mut grouped = BTreeMap::<WindowKey, Vec<IntervalMember>>::new();
    let mut periods = super::super::reporting::ReportingWindows::new();
    for member in members {
        budget.step()?;
        let shift_start = member.shift.interval.starts_at.instant;
        let shift_end = member.shift.interval.ends_at.instant;
        match membership {
            WindowMembership::ReportingDate => {
                let Some(period) = periods.resolve(
                    document,
                    input,
                    calendar_id,
                    member.shift.reporting_date,
                    budget,
                )?
                else {
                    continue;
                };
                let key = WindowKey {
                    calendar_id: Some(period.calendar_id),
                    start: period.start,
                    end: period.end,
                    duration_minutes: None,
                };
                insert_interval(
                    &mut grouped,
                    key,
                    IntervalMember {
                        index: member.index,
                        start: shift_start,
                        end: shift_end,
                        scheduled: member.shift.interval.scheduled_duration(),
                    },
                    budget,
                )?;
            }
            WindowMembership::StartInstant => {
                // A StartInstant calendar period is owned by the instant at which the shift
                // starts. Query only that instant's native calendar period; extending to the
                // shift end can introduce an unrelated ambiguous DST boundary.
                let query_end = offset(shift_start, 0, 1)?;
                let range = Horizon::new(shift_start, query_end).map_err(|_| invalid())?;
                let windows = resolve_calendar_for_calendar(
                    document,
                    calendar_id,
                    calendar,
                    range,
                    &mut |step| budget.calendar_step(step),
                )?;
                budget.steps(count(windows.len())?)?;
                for window in windows {
                    budget.expanded_interval()?;
                    if !(window.interval.starts_at.instant <= shift_start
                        && shift_start < window.interval.ends_at.instant)
                    {
                        continue;
                    }
                    let key = calendar_key(window);
                    insert_interval(
                        &mut grouped,
                        key,
                        IntervalMember {
                            index: member.index,
                            start: shift_start,
                            end: shift_end,
                            scheduled: member.shift.interval.scheduled_duration(),
                        },
                        budget,
                    )?;
                }
            }
            WindowMembership::Intersection => {
                let range = Horizon::new(
                    member.shift.interval.starts_at.instant,
                    member.shift.interval.ends_at.instant,
                )
                .map_err(|_| invalid())?;
                let windows = resolve_calendar_for_calendar(
                    document,
                    calendar_id,
                    calendar,
                    range,
                    &mut |step| budget.calendar_step(step),
                )?;
                budget.steps(count(windows.len())?)?;
                for window in windows {
                    budget.expanded_interval()?;
                    let key = calendar_key(window);
                    let Some((start, end)) = clip(shift_start, shift_end, key.start, key.end)
                    else {
                        continue;
                    };
                    insert_interval(
                        &mut grouped,
                        key,
                        IntervalMember {
                            index: member.index,
                            start,
                            end,
                            scheduled: SignedDuration::new(0, 0),
                        },
                        budget,
                    )?;
                }
            }
        }
    }
    grouped_aggregates(grouped, membership, measurement, contribution, budget)
}

pub(super) fn calendar_key(window: CalendarWindow) -> WindowKey {
    WindowKey {
        calendar_id: Some(window.calendar_id),
        start: window.interval.starts_at.instant,
        end: window.interval.ends_at.instant,
        duration_minutes: None,
    }
}

fn rolling_windows(
    members: &[Member<'_>],
    duration_minutes: u32,
    membership: WindowMembership,
    measurement: WorkloadMeasurement,
    contribution: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(HoursWindow, HoursAggregate)>, AssignmentRuleError> {
    let duration_seconds = i64::from(duration_minutes)
        .checked_mul(60)
        .ok_or_else(invalid)?;
    let mut starts = BTreeSet::new();
    for member in members {
        budget.step()?;
        match membership {
            WindowMembership::StartInstant => {
                let entry = offset(
                    member.shift.interval.starts_at.instant,
                    -duration_seconds,
                    1,
                )?;
                let exit = offset(member.shift.interval.starts_at.instant, 0, 1)?;
                add_boundary(&mut starts, entry, budget)?;
                add_boundary(&mut starts, exit, budget)?;
            }
            WindowMembership::Intersection => {
                // Overlap length against each fixed shift is continuous piecewise-linear in the
                // rolling start; extrema therefore occur at shift starts/ends or those points
                // shifted by -D, without +/-1ns probes.
                let start = member.shift.interval.starts_at.instant;
                let end = member.shift.interval.ends_at.instant;
                for boundary in [
                    start,
                    end,
                    offset(start, -duration_seconds, 0)?,
                    offset(end, -duration_seconds, 0)?,
                ] {
                    add_boundary(&mut starts, boundary, budget)?;
                }
            }
            WindowMembership::ReportingDate => return Err(invalid()),
        }
    }
    let mut result = Vec::new();
    for start in starts {
        budget.step()?;
        let end = offset(start, duration_seconds, 0)?;
        let key = WindowKey {
            calendar_id: None,
            start,
            end,
            duration_minutes: Some(duration_minutes),
        };
        let mut intervals = Vec::new();
        for member in members {
            budget.step()?;
            match membership {
                WindowMembership::StartInstant => {
                    let instant = member.shift.interval.starts_at.instant;
                    if start <= instant && instant < end {
                        intervals.push(IntervalMember {
                            index: member.index,
                            start: member.shift.interval.starts_at.instant,
                            end: member.shift.interval.ends_at.instant,
                            scheduled: member.shift.interval.scheduled_duration(),
                        });
                    }
                }
                WindowMembership::Intersection => {
                    let Some((clipped_start, clipped_end)) = clip(
                        member.shift.interval.starts_at.instant,
                        member.shift.interval.ends_at.instant,
                        start,
                        end,
                    ) else {
                        continue;
                    };
                    intervals.push(IntervalMember {
                        index: member.index,
                        start: clipped_start,
                        end: clipped_end,
                        scheduled: SignedDuration::new(0, 0),
                    });
                }
                WindowMembership::ReportingDate => unreachable!(),
            }
        }
        if intervals.is_empty() {
            continue;
        }
        let aggregate =
            aggregate_intervals(&intervals, membership, measurement, contribution, budget)?;
        if !aggregate_empty(&aggregate) {
            result.push((key.window(membership), aggregate));
        }
    }
    Ok(result)
}

pub(super) fn grouped_aggregates(
    grouped: BTreeMap<WindowKey, Vec<IntervalMember>>,
    membership: WindowMembership,
    measurement: WorkloadMeasurement,
    contribution: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(HoursWindow, HoursAggregate)>, AssignmentRuleError> {
    let mut result = Vec::new();
    for (key, intervals) in grouped {
        budget.step()?;
        let aggregate =
            aggregate_intervals(&intervals, membership, measurement, contribution, budget)?;
        if !aggregate_empty(&aggregate) {
            result.push((key.window(membership), aggregate));
        }
    }
    Ok(result)
}

pub(super) fn aggregate_intervals(
    intervals: &[IntervalMember],
    membership: WindowMembership,
    measurement: WorkloadMeasurement,
    contribution: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<HoursAggregate, AssignmentRuleError> {
    match contribution {
        OverlappingContribution::Sum => {
            let mut terms = Vec::new();
            for interval in intervals {
                budget.step()?;
                let duration = match measurement {
                    WorkloadMeasurement::ElapsedMinutes => interval
                        .start
                        .as_timestamp()
                        .duration_until(interval.end.as_timestamp()),
                    WorkloadMeasurement::ScheduledMinutes => {
                        if membership == WindowMembership::Intersection {
                            return Err(invalid());
                        }
                        // Sum/whole-shift scheduled duration is intentionally signed. A fold can
                        // produce a negative local duration and remains a negative credit.
                        interval.scheduled
                    }
                    WorkloadMeasurement::AssignmentCount => return Err(invalid()),
                };
                let (seconds, remainder) = split_duration(duration)?;
                if seconds != 0 || remainder != 0 {
                    budget.reserve(0, 1, 64)?;
                    terms.push(HoursTerm {
                        index: interval.index,
                        seconds,
                        remainder,
                    });
                }
            }
            Ok(HoursAggregate::Sum { terms })
        }
        OverlappingContribution::Union => {
            if measurement != WorkloadMeasurement::ElapsedMinutes {
                return Err(invalid());
            }
            let mut endpoints = BTreeSet::new();
            for interval in intervals {
                budget.step()?;
                endpoints.insert(interval.start);
                endpoints.insert(interval.end);
            }
            let mut segments = Vec::new();
            let mut iter = endpoints.into_iter();
            let Some(mut start) = iter.next() else {
                return Ok(HoursAggregate::Union { segments });
            };
            for end in iter {
                budget.step()?;
                if start >= end {
                    start = end;
                    continue;
                }
                let mut members = Vec::new();
                for interval in intervals {
                    budget.step()?;
                    if interval.start <= start && end <= interval.end {
                        members.push(interval.index);
                    }
                }
                if !members.is_empty() {
                    members.sort_unstable();
                    members.dedup();
                    let (seconds, remainder) =
                        split_duration(start.as_timestamp().duration_until(end.as_timestamp()))?;
                    budget.reserve(1, add(count(members.len())?, 3)?, 256)?;
                    segments.push(HoursSegment {
                        start,
                        end,
                        seconds,
                        remainder,
                        members,
                    });
                }
                start = end;
            }
            Ok(HoursAggregate::Union { segments })
        }
    }
}

pub(super) fn insert_interval(
    grouped: &mut BTreeMap<WindowKey, Vec<IntervalMember>>,
    key: WindowKey,
    interval: IntervalMember,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.reserve(0, 1, 64)?;
    grouped.entry(key).or_default().push(interval);
    Ok(())
}

pub(super) fn add_boundary(
    starts: &mut BTreeSet<Rfc3339Timestamp>,
    instant: Rfc3339Timestamp,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    budget.reserve(0, 1, 32)?;
    starts.insert(instant);
    Ok(())
}

pub(super) fn clip(
    shift_start: Rfc3339Timestamp,
    shift_end: Rfc3339Timestamp,
    window_start: Rfc3339Timestamp,
    window_end: Rfc3339Timestamp,
) -> Option<(Rfc3339Timestamp, Rfc3339Timestamp)> {
    let start = shift_start.max(window_start);
    let end = shift_end.min(window_end);
    (start < end).then_some((start, end))
}

pub(super) fn offset(
    instant: Rfc3339Timestamp,
    seconds: i64,
    nanos: i64,
) -> Result<Rfc3339Timestamp, AssignmentRuleError> {
    let duration = SignedDuration::new(seconds, i32::try_from(nanos).map_err(|_| invalid())?);
    instant
        .as_timestamp()
        .checked_add(duration)
        .map(Rfc3339Timestamp::from_timestamp)
        .map_err(|_| invalid())
}

pub(super) fn split_duration(duration: SignedDuration) -> Result<(i64, i64), AssignmentRuleError> {
    let total = i128::from(duration.as_secs())
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|value| value.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or_else(invalid)?;
    let seconds = total.div_euclid(NANOS_PER_SECOND);
    let remainder = total.rem_euclid(NANOS_PER_SECOND);
    let seconds = i64::try_from(seconds).map_err(|_| invalid())?;
    let remainder = i64::try_from(remainder).map_err(|_| invalid())?;
    if !(0..=MAX_NANOS_REMAINDER).contains(&remainder) {
        return Err(invalid());
    }
    Ok((seconds, remainder))
}

pub(super) fn validate_aggregate(
    aggregate: &HoursAggregate,
    limits: PlanningIrLimitsV1,
    minimum_minutes: u32,
    maximum_minutes: Option<u32>,
) -> Result<(), AssignmentRuleError> {
    let coefficient_limit = limits
        .max_abs_coefficient
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_coefficient);
    let value_limit = limits
        .max_abs_value
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_value);
    if coefficient_limit < 0 || value_limit < 0 {
        return Err(invalid());
    }
    for minutes in [Some(minimum_minutes), maximum_minutes]
        .into_iter()
        .flatten()
    {
        let target = i64::from(minutes).checked_mul(60).ok_or_else(invalid)?;
        if target.unsigned_abs() > value_limit.cast_unsigned() {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
    }
    let check = |seconds: i64, remainder: i64| -> Result<(), AssignmentRuleError> {
        if seconds.unsigned_abs() > coefficient_limit.cast_unsigned()
            || remainder.unsigned_abs() > coefficient_limit.cast_unsigned()
        {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
        Ok(())
    };
    match aggregate {
        HoursAggregate::Sum { terms } => {
            for term in terms {
                check(term.seconds, term.remainder)?;
            }
        }
        HoursAggregate::Union { segments } => {
            for segment in segments {
                check(segment.seconds, segment.remainder)?;
            }
        }
    }
    Ok(())
}

/// Source-independent exact nanosecond range check used by both compiler paths. The generic IR
/// validates i64 expression bounds and the OR-Tools adapter additionally requires ±i64/2.
pub(super) fn check_expression_range(
    minimum: i128,
    maximum: i128,
    rhs: i64,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let value_limit = i128::from(
        limits
            .max_abs_value
            .min(PlanningIrLimitsV1::DEFAULT.max_abs_value),
    );
    let native_limit = i128::from(i64::MAX / 2);
    if minimum < -value_limit
        || maximum > value_limit
        || minimum < -native_limit
        || maximum > native_limit
        || i128::from(rhs).abs() > value_limit
    {
        return Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord,
        ));
    }
    Ok(())
}

fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

use super::super::{AssignmentConstructionIssue, AssignmentVariable};
use crate::assignment_rules::identity::{IdentityKind, PlanningIdentities};
use eutheto_domain_ir::{DomainEntityId, DomainEntityKindId, DomainEntityRef};
use eutheto_planning_ir::{
    ComparisonOp, Constraint, ConstraintRecord, InclusiveRange, IntDomain, IntVariable,
    IntVariableId, LinearComparison, LinearExpression, LinearTerm, Literal, PlanningConstraintId,
    ProvenanceId, ProvenanceParameter, ProvenanceRecord, ProvenanceSourceKind, Variable,
};
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use std::fmt::{self, Write as _};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct HoursShape {
    pub pair_indices: u64,
    pub helper_variables: u64,
    pub constraints: u64,
    pub provenance: u64,
    /// Logical references charged by contribution preflight: fields which point to
    /// variables/provenance, constraint terms/enforcement, and provenance payloads.
    pub references: u64,
    /// Hyperedge references used by the generic component bound. Provenance payload
    /// fields are deliberately not graph edges.
    pub component_edges: u64,
}

/// Count every auxiliary declaration and channel before either contribution or complete-model
/// construction allocates them. Parent rule facts are counted by the existing plan parent pass.
pub(crate) fn shape(plan: &Plan) -> Result<HoursShape, AssignmentRuleError> {
    let mut shape = HoursShape::default();
    let mut indices = BTreeSet::new();
    for hours in &plan.hours {
        match &hours.aggregate {
            HoursAggregate::Sum { terms } => {
                for term in terms {
                    indices.insert(term.index);
                }
                add_window_shape(&mut shape, hours, terms, None)?;
            }
            HoursAggregate::Union { segments } => {
                for segment in segments {
                    for index in &segment.members {
                        indices.insert(*index);
                    }
                }
                add_window_shape(&mut shape, hours, &[], Some(segments))?;
            }
        }
    }
    shape.pair_indices = count(indices.len())?;
    let channel_constraints = shape.pair_indices.checked_mul(2).ok_or_else(arithmetic)?;
    shape.constraints = shape
        .constraints
        .checked_add(channel_constraints)
        .ok_or_else(arithmetic)?;
    shape.provenance = shape
        .provenance
        .checked_add(shape.pair_indices)
        .ok_or_else(arithmetic)?;
    shape.references = shape
        .references
        .checked_add(
            shape
                .pair_indices
                .checked_mul(channel_reference_count())
                .ok_or_else(arithmetic)?,
        )
        .ok_or_else(arithmetic)?;
    Ok(shape)
}

#[allow(clippy::too_many_lines)]
fn add_window_shape(
    shape: &mut HoursShape,
    hours: &HoursPlan,
    terms: &[HoursTerm],
    segments: Option<&[HoursSegment]>,
) -> Result<(), AssignmentRuleError> {
    let (lower, upper, seconds) = if let Some(segments) = segments {
        aggregate_term_counts_segments(segments)?
    } else {
        aggregate_term_counts(terms)?
    };
    let helpers = if lower == 0 { 0 } else { 2 };
    let segment_count = count(segments.map_or(0, <[HoursSegment]>::len))?;
    shape.helper_variables = shape
        .helper_variables
        .checked_add(helpers)
        .and_then(|value| value.checked_add(segment_count))
        .ok_or_else(arithmetic)?;
    let comparisons =
        helpers + u64::from(requires_lower(hours)) + u64::from(hours.maximum_minutes.is_some());
    shape.constraints = shape
        .constraints
        .checked_add(comparisons)
        .ok_or_else(arithmetic)?;
    shape.provenance = shape
        .provenance
        .checked_add(comparisons + helpers)
        .ok_or_else(arithmetic)?;
    let mut references = helpers; // helper variable provenance references
    let mut component_edges = 0_u64;
    let mut fact_references = hours_fact_references(hours, false)?
        .checked_mul(helpers)
        .ok_or_else(arithmetic)?;
    if let Some(segments) = segments {
        for segment in segments {
            let members = count(segment.members.len())?;
            shape.constraints = shape
                .constraints
                .checked_add(members.checked_add(1).ok_or_else(arithmetic)?)
                .ok_or_else(arithmetic)?;
            shape.provenance = shape
                .provenance
                .checked_add(members.checked_add(2).ok_or_else(arithmetic)?)
                .ok_or_else(arithmetic)?;
            let segment_references = members
                .checked_mul(4)
                .and_then(|value| value.checked_add(3))
                .ok_or_else(arithmetic)?;
            references = references
                .checked_add(segment_references)
                .ok_or_else(arithmetic)?;
            component_edges = component_edges
                .checked_add(members.checked_mul(2).ok_or_else(arithmetic)?)
                .ok_or_else(arithmetic)?;
            fact_references = fact_references
                .checked_add(
                    hours_fact_references(hours, true)?
                        .checked_mul(members.checked_add(2).ok_or_else(arithmetic)?)
                        .ok_or_else(arithmetic)?,
                )
                .ok_or_else(arithmetic)?;
        }
    }
    let floor = seconds
        .checked_sub(u64::from(helpers != 0))
        .ok_or_else(arithmetic)?;
    references = references
        .checked_add(lower)
        .and_then(|value| value.checked_add(upper))
        .and_then(|value| value.checked_add(if requires_lower(hours) { floor } else { 0 }))
        .and_then(|value| {
            value.checked_add(if hours.maximum_minutes.is_some() {
                seconds
            } else {
                0
            })
        })
        .and_then(|value| value.checked_add(comparisons))
        .ok_or_else(arithmetic)?;
    component_edges = component_edges
        .checked_add(lower.saturating_sub(1))
        .and_then(|value| value.checked_add(upper.saturating_sub(1)))
        .and_then(|value| {
            value.checked_add(if requires_lower(hours) {
                floor.saturating_sub(1)
            } else {
                0
            })
        })
        .and_then(|value| {
            value.checked_add(if hours.maximum_minutes.is_some() {
                seconds.saturating_sub(1)
            } else {
                0
            })
        })
        .ok_or_else(arithmetic)?;
    fact_references = fact_references
        .checked_add(
            hours_fact_references(hours, true)?
                .checked_mul(comparisons)
                .ok_or_else(arithmetic)?,
        )
        .ok_or_else(arithmetic)?;
    shape.references = shape
        .references
        .checked_add(references)
        .and_then(|value| value.checked_add(fact_references))
        .ok_or_else(arithmetic)?;
    shape.component_edges = shape
        .component_edges
        .checked_add(component_edges)
        .ok_or_else(arithmetic)?;
    Ok(())
}

fn aggregate_term_counts(terms: &[HoursTerm]) -> Result<(u64, u64, u64), AssignmentRuleError> {
    let nonzero_remainders = count(terms.iter().filter(|term| term.remainder != 0).count())?;
    let nonzero_seconds = count(terms.iter().filter(|term| term.seconds != 0).count())?;
    if nonzero_remainders == 0 {
        return Ok((0, 0, nonzero_seconds));
    }
    Ok((
        nonzero_remainders.checked_add(1).ok_or_else(arithmetic)?,
        nonzero_remainders.checked_add(2).ok_or_else(arithmetic)?,
        nonzero_seconds.checked_add(2).ok_or_else(arithmetic)?,
    ))
}

fn aggregate_term_counts_segments(
    segments: &[HoursSegment],
) -> Result<(u64, u64, u64), AssignmentRuleError> {
    let nonzero_remainders = count(
        segments
            .iter()
            .filter(|segment| segment.remainder != 0)
            .count(),
    )?;
    let nonzero_seconds = count(
        segments
            .iter()
            .filter(|segment| segment.seconds != 0)
            .count(),
    )?;
    if nonzero_remainders == 0 {
        return Ok((0, 0, nonzero_seconds));
    }
    Ok((
        nonzero_remainders.checked_add(1).ok_or_else(arithmetic)?,
        nonzero_remainders.checked_add(2).ok_or_else(arithmetic)?,
        nonzero_seconds.checked_add(2).ok_or_else(arithmetic)?,
    ))
}

fn hours_fact_references(hours: &HoursPlan, component: bool) -> Result<u64, AssignmentRuleError> {
    let entities = 2 + u64::from(hours.window.calendar_id.is_some());
    let parameters = 5
        + u64::from(hours.maximum_minutes.is_some())
        + u64::from(matches!(hours.kind, HoursPlanKind::WorkloadQuota))
        + u64::from(component);
    entities
        .checked_add(parameters)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(arithmetic)
}

fn channel_reference_count() -> u64 {
    // Channel fact: two entities and one parent; integer variable: one provenance;
    // each enforced comparison has one term, one enforcement literal, and one provenance.
    3 + 1 + 2 * 3
}

/// Apply all hours-specific per-record limits before any helper graph is allocated.
pub(crate) fn checked_shape(
    plan: &Plan,
    limits: PlanningIrLimitsV1,
) -> Result<HoursShape, AssignmentRuleError> {
    let shape = shape(plan)?;
    let defaults = PlanningIrLimitsV1::DEFAULT;
    let max_id = limits.max_id_bytes.min(defaults.max_id_bytes);
    for hours in &plan.hours {
        if hours.window.calendar_id.is_some() {
            within(
                count("official.workforce.calendar.00000000-0000-0000-0000-000000000000".len())?,
                max_id,
                AssignmentRuleLimit::PerRecord,
            )?;
        }
        for id in [
            MEASURED_BOOL_ID,
            MEASURED_INT_ID,
            MEASURED_CONSTRAINT_ID,
            MEASURED_PROVENANCE_ID,
        ] {
            within(count(id.len())?, max_id, AssignmentRuleLimit::PerRecord)?;
        }
        check_hours_record_limits(hours, limits, defaults)?;
    }
    if shape.pair_indices > 0 {
        within(
            1,
            limits
                .max_enforcement_literals
                .min(defaults.max_enforcement_literals),
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            1,
            limits.max_refs_per_node.min(defaults.max_refs_per_node),
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            2,
            limits
                .max_entity_refs_per_record
                .min(defaults.max_entity_refs_per_record),
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            36,
            limits
                .max_metadata_text_bytes
                .min(defaults.max_metadata_text_bytes),
            AssignmentRuleLimit::PerRecord,
        )?;
        within(
            count("official.workforce.maximum_hours.channel".len())?,
            limits
                .max_metadata_text_bytes
                .min(defaults.max_metadata_text_bytes),
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    Ok(shape)
}

// The exact per-record facts, coefficients and aggregate all share this admission decision.
#[allow(clippy::too_many_lines)]
fn check_hours_record_limits(
    hours: &HoursPlan,
    limits: PlanningIrLimitsV1,
    defaults: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let max_entity_refs = limits
        .max_entity_refs_per_record
        .min(defaults.max_entity_refs_per_record);
    let max_parameters = limits
        .max_parameters_per_record
        .min(defaults.max_parameters_per_record);
    let max_parameter_text = limits
        .max_parameter_text_bytes
        .min(defaults.max_parameter_text_bytes);
    let max_metadata_text = limits
        .max_metadata_text_bytes
        .min(defaults.max_metadata_text_bytes);
    let entity_refs = 2 + u64::from(hours.window.calendar_id.is_some());
    within(entity_refs, max_entity_refs, AssignmentRuleLimit::PerRecord)?;
    let parameters = 5
        + u64::from(matches!(hours.kind, HoursPlanKind::WorkloadQuota))
        + u64::from(hours.maximum_minutes.is_some());
    within(parameters, max_parameters, AssignmentRuleLimit::PerRecord)?;
    within(
        parameters + 1,
        max_parameters,
        AssignmentRuleLimit::PerRecord,
    )?;
    within(36, max_metadata_text, AssignmentRuleLimit::PerRecord)?;
    for message in [
        "official.workforce.maximum_hours",
        "official.workforce.workload_quota",
        helper_message(hours.kind, HoursHelperFact::Carry),
        helper_message(hours.kind, HoursHelperFact::Fractional),
        helper_message(hours.kind, HoursHelperFact::UnionSegment),
    ] {
        within(
            count(message.len())?,
            max_metadata_text,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for key in [
        "component",
        "maximum_minutes",
        "minimum_minutes",
        "measurement",
        "membership",
        "overlapping_contribution",
        "window_end",
        "window_start",
    ] {
        within(
            count(key.len())?,
            max_parameter_text,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    for text in [
        "ElapsedMinutes",
        "ScheduledMinutes",
        "AssignmentCount",
        "ReportingDate",
        "StartInstant",
        "Intersection",
        "Sum",
        "Union",
    ] {
        within(
            count(text.len())?,
            max_parameter_text,
            AssignmentRuleLimit::PerRecord,
        )?;
    }
    within(
        timestamp_length(hours.window.start)?,
        max_parameter_text,
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        timestamp_length(hours.window.end)?,
        max_parameter_text,
        AssignmentRuleLimit::PerRecord,
    )?;
    let fractional = hours.aggregate.has_fractional();
    if fractional {
        check_coefficient(-1_000_000_000, limits)?;
        check_coefficient(-MAX_NANOS_REMAINDER, limits)?;
    }
    check_coefficient(1, limits)?;
    let minimum_target = i64::from(hours.minimum_minutes)
        .checked_mul(60)
        .ok_or_else(arithmetic)?;
    check_value(minimum_target, limits)?;
    if let Some(maximum_minutes) = hours.maximum_minutes {
        let target = i64::from(maximum_minutes)
            .checked_mul(60)
            .ok_or_else(arithmetic)?;
        check_value(target, limits)?;
    }
    let check_term = |seconds: i64, remainder: i64| -> Result<(), AssignmentRuleError> {
        check_coefficient(seconds, limits)?;
        check_coefficient(remainder, limits)?;
        Ok(())
    };
    let max_refs = limits.max_refs_per_node.min(defaults.max_refs_per_node);
    let check_expression = |term_count: u64| -> Result<(), AssignmentRuleError> {
        within(term_count, max_refs, AssignmentRuleLimit::PerRecord)
    };
    match &hours.aggregate {
        HoursAggregate::Sum { terms } => {
            check_value(2, limits)?;
            for term in terms {
                check_term(term.seconds, term.remainder)?;
            }
            let (lower, upper, seconds) = aggregate_term_counts(terms)?;
            check_expression(lower)?;
            check_expression(upper)?;
            check_expression(seconds)?;
            if fractional {
                check_value(
                    i64::try_from(terms.len()).map_err(|_| arithmetic())?,
                    limits,
                )?;
            }
        }
        HoursAggregate::Union { segments } => {
            check_value(
                i64::try_from(segments.len()).map_err(|_| arithmetic())?,
                limits,
            )?;
            for (segment_index, segment) in segments.iter().enumerate() {
                check_value(
                    i64::try_from(segment_index).map_err(|_| arithmetic())?,
                    limits,
                )?;
                check_term(segment.seconds, segment.remainder)?;
                let members = count(segment.members.len())?;
                check_expression(2)?;
                check_expression(members.checked_add(1).ok_or_else(arithmetic)?)?;
                check_value(1, limits)?;
            }
            let (lower, upper, seconds) = aggregate_term_counts_segments(segments)?;
            check_expression(lower)?;
            check_expression(upper)?;
            check_expression(seconds)?;
            check_value(
                i64::try_from(segments.len()).map_err(|_| arithmetic())?,
                limits,
            )?;
        }
    }
    check_hours_expression_ranges(hours, limits)?;
    within(
        1,
        limits.max_domain_ranges.min(defaults.max_domain_ranges),
        AssignmentRuleLimit::PerRecord,
    )?;
    within(
        2,
        limits.max_provenance_depth,
        AssignmentRuleLimit::PerRecord,
    )?;
    Ok(())
}

fn check_hours_expression_ranges(
    hours: &HoursPlan,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let (carry_max, seconds_min, seconds_max, remainders_max) = match &hours.aggregate {
        HoursAggregate::Sum { terms } => {
            let carry_max = i128::from(count(terms.len())?);
            let mut seconds_min = 0_i128;
            let mut seconds_max = 0_i128;
            let mut remainders_max = 0_i128;
            for term in terms {
                let seconds = i128::from(term.seconds);
                seconds_min = seconds_min
                    .checked_add(seconds.min(0))
                    .ok_or_else(arithmetic)?;
                seconds_max = seconds_max
                    .checked_add(seconds.max(0))
                    .ok_or_else(arithmetic)?;
                remainders_max = remainders_max
                    .checked_add(i128::from(term.remainder))
                    .ok_or_else(arithmetic)?;
            }
            (carry_max, seconds_min, seconds_max, remainders_max)
        }
        HoursAggregate::Union { segments } => {
            let carry_max = i128::from(count(segments.len())?);
            let mut seconds_min = 0_i128;
            let mut seconds_max = 0_i128;
            let mut remainders_max = 0_i128;
            for segment in segments {
                let seconds = i128::from(segment.seconds);
                seconds_min = seconds_min
                    .checked_add(seconds.min(0))
                    .ok_or_else(arithmetic)?;
                seconds_max = seconds_max
                    .checked_add(seconds.max(0))
                    .ok_or_else(arithmetic)?;
                remainders_max = remainders_max
                    .checked_add(i128::from(segment.remainder))
                    .ok_or_else(arithmetic)?;
            }
            (carry_max, seconds_min, seconds_max, remainders_max)
        }
    };
    let fractional = hours.aggregate.has_fractional();
    let carry_max = if fractional { carry_max } else { 0 };
    if fractional {
        check_expression_range(
            -NANOS_PER_SECOND
                .checked_mul(carry_max)
                .ok_or_else(arithmetic)?,
            remainders_max,
            0,
            limits,
        )?;
    }
    let minimum_target = i64::from(hours.minimum_minutes)
        .checked_mul(60)
        .ok_or_else(arithmetic)?;
    check_expression_range(
        seconds_min,
        seconds_max.checked_add(carry_max).ok_or_else(arithmetic)?,
        minimum_target,
        limits,
    )?;
    if let Some(maximum_minutes) = hours.maximum_minutes {
        let target = i64::from(maximum_minutes)
            .checked_mul(60)
            .ok_or_else(arithmetic)?;
        check_expression_range(
            seconds_min,
            seconds_max
                .checked_add(carry_max)
                .and_then(|maximum| maximum.checked_add(i128::from(fractional)))
                .ok_or_else(arithmetic)?,
            target,
            limits,
        )?;
    }
    Ok(())
}

fn check_coefficient(value: i64, limits: PlanningIrLimitsV1) -> Result<(), AssignmentRuleError> {
    let maximum = limits
        .max_abs_coefficient
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_coefficient);
    if maximum < 0 || value.unsigned_abs() > maximum.cast_unsigned() {
        return Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord,
        ));
    }
    Ok(())
}
const MEASURED_BOOL_ID: &str =
    "official.workforce.bool.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURED_INT_ID: &str =
    "official.workforce.int.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURED_CONSTRAINT_ID: &str = "official.workforce.constraint.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURED_PROVENANCE_ID: &str = "official.workforce.provenance.0000000000000000000000000000000000000000000000000000000000000000";
const MEASURED_UUID: &str = "00000000-0000-0000-0000-000000000000";
const MEASURED_TIMESTAMP: &str = "0000000000000000000000000000000000000000000000000000000000000000";

struct FormatLength(usize);

impl fmt::Write for FormatLength {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.0 = self.0.checked_add(value.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

fn timestamp_length(value: Rfc3339Timestamp) -> Result<u64, AssignmentRuleError> {
    let mut length = FormatLength(0);
    write!(&mut length, "{value}").map_err(|_| arithmetic())?;
    count(length.0)
}

struct MeasuredTimestamp(Rfc3339Timestamp);

impl Serialize for MeasuredTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let length = timestamp_length(self.0).map_err(serde::ser::Error::custom)?;
        let length = usize::try_from(length).map_err(|_| serde::ser::Error::custom("timestamp"))?;
        let value = MEASURED_TIMESTAMP
            .get(..length)
            .ok_or_else(|| serde::ser::Error::custom("timestamp length"))?;
        serializer.serialize_str(value)
    }
}

enum MeasuredText<'a> {
    Static(&'a str),
    Timestamp(Rfc3339Timestamp),
}

impl Serialize for MeasuredText<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Static(value) => serializer.serialize_str(value),
            Self::Timestamp(value) => MeasuredTimestamp(*value).serialize(serializer),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredParameter<'a> {
    Integer(i64),
    Text(MeasuredText<'a>),
}

struct MeasuredParameters<'a> {
    hours: &'a HoursPlan,
    component: Option<usize>,
}

impl Serialize for MeasuredParameters<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let quota = matches!(self.hours.kind, HoursPlanKind::WorkloadQuota);
        let count = 5
            + usize::from(self.component.is_some())
            + usize::from(self.hours.maximum_minutes.is_some())
            + usize::from(quota);
        let mut map = serializer.serialize_map(Some(count))?;
        if let Some(component) = self.component {
            let component =
                i64::try_from(component).map_err(|_| serde::ser::Error::custom("component"))?;
            map.serialize_entry("component", &MeasuredParameter::Integer(component))?;
        }
        if quota {
            map.serialize_entry(
                "minimum_minutes",
                &MeasuredParameter::Integer(i64::from(self.hours.minimum_minutes)),
            )?;
        }
        if let Some(maximum_minutes) = self.hours.maximum_minutes {
            map.serialize_entry(
                "maximum_minutes",
                &MeasuredParameter::Integer(i64::from(maximum_minutes)),
            )?;
        }
        map.serialize_entry(
            "measurement",
            &MeasuredParameter::Text(MeasuredText::Static(measurement_name(
                self.hours.measurement,
            ))),
        )?;
        map.serialize_entry(
            "membership",
            &MeasuredParameter::Text(MeasuredText::Static(membership_name(
                self.hours.window.membership,
            ))),
        )?;
        map.serialize_entry(
            "overlapping_contribution",
            &MeasuredParameter::Text(MeasuredText::Static(contribution_name(
                self.hours.contribution,
            ))),
        )?;
        map.serialize_entry(
            "window_end",
            &MeasuredParameter::Text(MeasuredText::Timestamp(self.hours.window.end)),
        )?;
        map.serialize_entry(
            "window_start",
            &MeasuredParameter::Text(MeasuredText::Timestamp(self.hours.window.start)),
        )?;
        map.end()
    }
}

fn measurement_name(value: WorkloadMeasurement) -> &'static str {
    match value {
        WorkloadMeasurement::AssignmentCount => "AssignmentCount",
        WorkloadMeasurement::ElapsedMinutes => "ElapsedMinutes",
        WorkloadMeasurement::ScheduledMinutes => "ScheduledMinutes",
    }
}

fn membership_name(value: WindowMembership) -> &'static str {
    match value {
        WindowMembership::ReportingDate => "ReportingDate",
        WindowMembership::StartInstant => "StartInstant",
        WindowMembership::Intersection => "Intersection",
    }
}

fn contribution_name(value: OverlappingContribution) -> &'static str {
    match value {
        OverlappingContribution::Sum => "Sum",
        OverlappingContribution::Union => "Union",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredEntity {
    kind: &'static str,
    id: &'static str,
}

struct MeasuredEntities {
    calendar: bool,
    channel: bool,
}

impl Serialize for MeasuredEntities {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let length = if self.channel {
            2
        } else {
            2 + usize::from(self.calendar)
        };
        let mut sequence = serializer.serialize_seq(Some(length))?;
        if self.calendar {
            sequence.serialize_element(&MeasuredEntity {
                kind: "official.workforce.calendar",
                id: "official.workforce.00000000-0000-0000-0000-000000000000",
            })?;
        }
        sequence.serialize_element(&MeasuredEntity {
            kind: "official.workforce.person",
            id: "official.workforce.00000000-0000-0000-0000-000000000000",
        })?;
        sequence.serialize_element(&MeasuredEntity {
            kind: if self.channel {
                "official.workforce.shift"
            } else {
                "official.workforce.workload_bucket"
            },
            id: "official.workforce.00000000-0000-0000-0000-000000000000",
        })?;
        sequence.end()
    }
}

enum MeasuredFactKind<'a> {
    Hours {
        hours: &'a HoursPlan,
        component: Option<usize>,
        message: &'static str,
    },
    Channel,
}

struct MeasuredFact<'a> {
    kind: MeasuredFactKind<'a>,
}

impl Serialize for MeasuredFact<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(7))?;
        map.serialize_entry("id", MEASURED_PROVENANCE_ID)?;
        map.serialize_entry("sourceKind", &ProvenanceSourceKind::Derived)?;
        match &self.kind {
            MeasuredFactKind::Hours {
                hours,
                component,
                message,
            } => {
                map.serialize_entry("sourceId", MEASURED_UUID)?;
                map.serialize_entry(
                    "entityRefs",
                    &MeasuredEntities {
                        calendar: hours.window.calendar_id.is_some(),
                        channel: false,
                    },
                )?;
                map.serialize_entry("messageKey", *message)?;
                map.serialize_entry(
                    "parameters",
                    &MeasuredParameters {
                        hours,
                        component: *component,
                    },
                )?;
            }
            MeasuredFactKind::Channel => {
                map.serialize_entry("sourceId", MEASURED_UUID)?;
                map.serialize_entry(
                    "entityRefs",
                    &MeasuredEntities {
                        calendar: false,
                        channel: true,
                    },
                )?;
                map.serialize_entry("messageKey", "official.workforce.maximum_hours.channel")?;
                map.serialize_entry("parameters", &EmptyParameters)?;
            }
        }
        map.serialize_entry("parent", &Some(MEASURED_PROVENANCE_ID))?;
        map.end()
    }
}

struct EmptyParameters;

impl Serialize for EmptyParameters {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_map(Some(0))?.end()
    }
}

fn check_value(value: i64, limits: PlanningIrLimitsV1) -> Result<(), AssignmentRuleError> {
    let maximum = limits
        .max_abs_value
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_value);
    if maximum < 0 || value.unsigned_abs() > maximum.cast_unsigned() {
        return Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::PerRecord,
        ));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredRange {
    start: i64,
    end: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredDomain {
    inclusive_ranges: [MeasuredRange; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredIntegerVariable {
    id: &'static str,
    domain: MeasuredDomain,
    provenance: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredVariable {
    Integer(MeasuredIntegerVariable),
}

struct MeasuredTerm {
    variable: &'static str,
    coefficient: i64,
}

impl Serialize for MeasuredTerm {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("variable", self.variable)?;
        map.serialize_entry("coefficient", &self.coefficient)?;
        map.end()
    }
}

enum MeasuredTermSource<'a> {
    Single,
    SegmentLower,
    SegmentUpper(usize),
    Sum {
        terms: &'a [HoursTerm],
        carry: bool,
        remainder: bool,
        fractional: bool,
    },
    Union {
        segments: &'a [HoursSegment],
        carry: bool,
        remainder: bool,
        fractional: bool,
    },
}

impl MeasuredTermSource<'_> {
    fn length(&self) -> usize {
        match self {
            Self::Single => 1,
            Self::SegmentLower => 2,
            Self::SegmentUpper(members) => 1 + members,
            Self::Sum {
                terms,
                carry,
                remainder,
                fractional,
            } => {
                usize::from(*carry)
                    + terms
                        .iter()
                        .filter(|term| {
                            if *remainder {
                                term.remainder != 0
                            } else {
                                term.seconds != 0
                            }
                        })
                        .count()
                    + usize::from(*fractional)
            }
            Self::Union {
                segments,
                carry,
                remainder,
                fractional,
            } => {
                usize::from(*carry)
                    + segments
                        .iter()
                        .filter(|segment| {
                            if *remainder {
                                segment.remainder != 0
                            } else {
                                segment.seconds != 0
                            }
                        })
                        .count()
                    + usize::from(*fractional)
            }
        }
    }
}

impl Serialize for MeasuredTermSource<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.length()))?;
        match self {
            Self::Single => {
                sequence.serialize_element(&MeasuredTerm {
                    variable: MEASURED_INT_ID,
                    coefficient: 1,
                })?;
            }
            Self::SegmentLower => {
                sequence.serialize_element(&MeasuredTerm {
                    variable: MEASURED_INT_ID,
                    coefficient: 1,
                })?;
                sequence.serialize_element(&MeasuredTerm {
                    variable: MEASURED_INT_ID,
                    coefficient: -1,
                })?;
            }
            Self::SegmentUpper(members) => {
                sequence.serialize_element(&MeasuredTerm {
                    variable: MEASURED_INT_ID,
                    coefficient: 1,
                })?;
                for _ in 0..*members {
                    sequence.serialize_element(&MeasuredTerm {
                        variable: MEASURED_INT_ID,
                        coefficient: -1,
                    })?;
                }
            }
            Self::Sum {
                terms,
                carry,
                remainder,
                fractional,
            } => {
                if *carry {
                    sequence.serialize_element(&MeasuredTerm {
                        variable: MEASURED_INT_ID,
                        coefficient: if *remainder { -1_000_000_000 } else { 1 },
                    })?;
                }
                for term in *terms {
                    let coefficient = if *remainder {
                        term.remainder
                    } else {
                        term.seconds
                    };
                    if coefficient != 0 {
                        sequence.serialize_element(&MeasuredTerm {
                            variable: MEASURED_INT_ID,
                            coefficient,
                        })?;
                    }
                }
                if *fractional {
                    sequence.serialize_element(&MeasuredTerm {
                        variable: MEASURED_INT_ID,
                        coefficient: if *remainder { -MAX_NANOS_REMAINDER } else { 1 },
                    })?;
                }
            }
            Self::Union {
                segments,
                carry,
                remainder,
                fractional,
            } => {
                if *carry {
                    sequence.serialize_element(&MeasuredTerm {
                        variable: MEASURED_INT_ID,
                        coefficient: if *remainder { -1_000_000_000 } else { 1 },
                    })?;
                }
                for segment in *segments {
                    let coefficient = if *remainder {
                        segment.remainder
                    } else {
                        segment.seconds
                    };
                    if coefficient != 0 {
                        sequence.serialize_element(&MeasuredTerm {
                            variable: MEASURED_INT_ID,
                            coefficient,
                        })?;
                    }
                }
                if *fractional {
                    sequence.serialize_element(&MeasuredTerm {
                        variable: MEASURED_INT_ID,
                        coefficient: if *remainder { -MAX_NANOS_REMAINDER } else { 1 },
                    })?;
                }
            }
        }
        sequence.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredExpression<'a> {
    terms: MeasuredTermSource<'a>,
    constant: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredComparison<'a> {
    expression: MeasuredExpression<'a>,
    op: ComparisonOp,
    rhs: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
enum MeasuredConstraintBody<'a> {
    LinearComparison(MeasuredComparison<'a>),
}

struct MeasuredEnforcement(Option<bool>);

impl Serialize for MeasuredEnforcement {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(usize::from(self.0.is_some())))?;
        if let Some(positive) = self.0 {
            sequence.serialize_element(&MeasuredLiteral {
                variable: MEASURED_BOOL_ID,
                positive,
            })?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredLiteral {
    variable: &'static str,
    positive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeasuredConstraintRecord<'a> {
    id: &'static str,
    body: MeasuredConstraintBody<'a>,
    enforcement: MeasuredEnforcement,
    provenance: &'static str,
    tags: [(); 0],
}

fn measured_variable(end: i64) -> MeasuredVariable {
    MeasuredVariable::Integer(MeasuredIntegerVariable {
        id: MEASURED_INT_ID,
        domain: MeasuredDomain {
            inclusive_ranges: [MeasuredRange { start: 0, end }],
        },
        provenance: MEASURED_PROVENANCE_ID,
    })
}

fn measured_constraint(
    source: MeasuredTermSource<'_>,
    op: ComparisonOp,
    rhs: i64,
    enforcement: Option<bool>,
) -> MeasuredConstraintRecord<'_> {
    MeasuredConstraintRecord {
        id: MEASURED_CONSTRAINT_ID,
        body: MeasuredConstraintBody::LinearComparison(MeasuredComparison {
            expression: MeasuredExpression {
                terms: source,
                constant: 0,
            },
            op,
            rhs,
        }),
        enforcement: MeasuredEnforcement(enforcement),
        provenance: MEASURED_PROVENANCE_ID,
        tags: [],
    }
}

#[derive(Clone, Copy)]
enum ChargeMode {
    Retained,
    Ir,
}

fn charge_record<T: Serialize>(
    value: &T,
    references: u64,
    mode: ChargeMode,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    // Retained preflight also covers each construction-time scratch reservation; IR charging
    // remains the exact serialized size used for complete-model retention.
    let bytes = match mode {
        ChargeMode::Retained => budget.measure(value)?.max(768),
        ChargeMode::Ir => budget.measure_ir(value)?,
    };
    match mode {
        ChargeMode::Retained => budget.reserve(1, references, bytes),
        ChargeMode::Ir => budget.reserve_ir(1, references, bytes),
    }
}

fn charge_channel(
    selected: bool,
    mode: ChargeMode,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    charge_record(
        &MeasuredConstraintRecord {
            id: MEASURED_CONSTRAINT_ID,
            body: MeasuredConstraintBody::LinearComparison(MeasuredComparison {
                expression: MeasuredExpression {
                    terms: MeasuredTermSource::Single,
                    constant: 0,
                },
                op: ComparisonOp::Equal,
                rhs: i64::from(selected),
            }),
            enforcement: MeasuredEnforcement(Some(selected)),
            provenance: MEASURED_PROVENANCE_ID,
            tags: [],
        },
        3,
        mode,
        budget,
    )?;
    Ok(())
}

fn charge_hours(
    hours: &HoursPlan,
    mode: ChargeMode,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let carry = hours.aggregate.has_fractional();
    if carry {
        let length = match &hours.aggregate {
            HoursAggregate::Sum { terms } => terms.len(),
            HoursAggregate::Union { segments } => segments.len(),
        };
        let carry_max = i64::try_from(length).map_err(|_| arithmetic())?;
        for (maximum, helper) in [
            (carry_max, HoursHelperFact::Carry),
            (1, HoursHelperFact::Fractional),
        ] {
            charge_record(&measured_variable(maximum), 1, mode, budget)?;
            charge_record(
                &MeasuredFact {
                    kind: MeasuredFactKind::Hours {
                        hours,
                        component: None,
                        message: helper_message(hours.kind, helper),
                    },
                },
                hours_fact_references(hours, false)?,
                mode,
                budget,
            )?;
        }
    }
    match &hours.aggregate {
        HoursAggregate::Sum { terms } => {
            charge_sum_constraints(hours, terms, None, carry, mode, budget)?;
        }
        HoursAggregate::Union { segments } => {
            for (segment_index, segment) in segments.iter().enumerate() {
                charge_record(&measured_variable(1), 1, mode, budget)?;
                charge_record(
                    &MeasuredFact {
                        kind: MeasuredFactKind::Hours {
                            hours,
                            component: Some(segment_index),
                            message: helper_message(hours.kind, HoursHelperFact::UnionSegment),
                        },
                    },
                    hours_fact_references(hours, true)?,
                    mode,
                    budget,
                )?;
                for _ in &segment.members {
                    charge_aggregate_fact(hours, segment_index, mode, budget)?;
                    charge_record(
                        &measured_constraint(
                            MeasuredTermSource::SegmentLower,
                            ComparisonOp::GreaterOrEqual,
                            0,
                            None,
                        ),
                        3_u64
                            .checked_add(hours_fact_references(hours, true)?)
                            .ok_or_else(arithmetic)?,
                        mode,
                        budget,
                    )?;
                }
                charge_aggregate_fact(hours, segment_index, mode, budget)?;
                charge_record(
                    &measured_constraint(
                        MeasuredTermSource::SegmentUpper(segment.members.len()),
                        ComparisonOp::LessOrEqual,
                        0,
                        None,
                    ),
                    add(
                        add(count(segment.members.len())?, 2)?,
                        hours_fact_references(hours, true)?,
                    )?,
                    mode,
                    budget,
                )?;
            }
            charge_sum_constraints(hours, &[], Some(segments), carry, mode, budget)?;
        }
    }
    Ok(())
}

fn charge_aggregate_fact(
    hours: &HoursPlan,
    component: usize,
    mode: ChargeMode,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    charge_record(
        &MeasuredFact {
            kind: MeasuredFactKind::Hours {
                hours,
                component: Some(component),
                message: match hours.kind {
                    HoursPlanKind::MaximumHours => "official.workforce.maximum_hours",
                    HoursPlanKind::WorkloadQuota => "official.workforce.workload_quota",
                },
            },
        },
        hours_fact_references(hours, true)?,
        mode,
        budget,
    )
}

#[allow(clippy::too_many_lines)]
fn charge_sum_constraints(
    hours: &HoursPlan,
    terms: &[HoursTerm],
    segments: Option<&[HoursSegment]>,
    carry: bool,
    mode: ChargeMode,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let (lower, upper, floor, seconds) = if let Some(segments) = segments {
        (
            MeasuredTermSource::Union {
                segments,
                carry,
                remainder: true,
                fractional: false,
            },
            MeasuredTermSource::Union {
                segments,
                carry,
                remainder: true,
                fractional: carry,
            },
            MeasuredTermSource::Union {
                segments,
                carry,
                remainder: false,
                fractional: false,
            },
            MeasuredTermSource::Union {
                segments,
                carry,
                remainder: false,
                fractional: carry,
            },
        )
    } else {
        (
            MeasuredTermSource::Sum {
                terms,
                carry,
                remainder: true,
                fractional: false,
            },
            MeasuredTermSource::Sum {
                terms,
                carry,
                remainder: true,
                fractional: carry,
            },
            MeasuredTermSource::Sum {
                terms,
                carry,
                remainder: false,
                fractional: false,
            },
            MeasuredTermSource::Sum {
                terms,
                carry,
                remainder: false,
                fractional: carry,
            },
        )
    };
    let aggregate_component = segments.map_or(0, <[HoursSegment]>::len);
    let floor_component = if segments.is_some() {
        aggregate_component + 1
    } else {
        2
    };
    let seconds_component = if segments.is_some() {
        aggregate_component + 1
    } else {
        3
    };
    let floor_refs = u64::try_from(floor.length())
        .map_err(|_| arithmetic())?
        .checked_add(1)
        .ok_or_else(arithmetic)?;
    let seconds_refs = u64::try_from(seconds.length())
        .map_err(|_| arithmetic())?
        .checked_add(1)
        .ok_or_else(arithmetic)?;
    if carry {
        let lower_refs = u64::try_from(lower.length())
            .map_err(|_| arithmetic())?
            .checked_add(1)
            .ok_or_else(arithmetic)?;
        let upper_refs = u64::try_from(upper.length())
            .map_err(|_| arithmetic())?
            .checked_add(1)
            .ok_or_else(arithmetic)?;
        charge_aggregate_fact(hours, 0, mode, budget)?;
        charge_record(
            &measured_constraint(lower, ComparisonOp::GreaterOrEqual, 0, None),
            lower_refs
                .checked_add(hours_fact_references(hours, true)?)
                .ok_or_else(arithmetic)?,
            mode,
            budget,
        )?;
        charge_aggregate_fact(hours, 1, mode, budget)?;
        charge_record(
            &measured_constraint(upper, ComparisonOp::LessOrEqual, 0, None),
            upper_refs
                .checked_add(hours_fact_references(hours, true)?)
                .ok_or_else(arithmetic)?,
            mode,
            budget,
        )?;
    }
    if requires_lower(hours) {
        let target = i64::from(hours.minimum_minutes)
            .checked_mul(60)
            .ok_or_else(arithmetic)?;
        charge_aggregate_fact(hours, floor_component, mode, budget)?;
        charge_record(
            &measured_constraint(floor, ComparisonOp::GreaterOrEqual, target, None),
            floor_refs
                .checked_add(hours_fact_references(hours, true)?)
                .ok_or_else(arithmetic)?,
            mode,
            budget,
        )?;
    }
    if let Some(maximum_minutes) = hours.maximum_minutes {
        let target = i64::from(maximum_minutes)
            .checked_mul(60)
            .ok_or_else(arithmetic)?;
        charge_aggregate_fact(hours, seconds_component, mode, budget)?;
        charge_record(
            &measured_constraint(seconds, ComparisonOp::LessOrEqual, target, None),
            seconds_refs
                .checked_add(hours_fact_references(hours, true)?)
                .ok_or_else(arithmetic)?,
            mode,
            budget,
        )?;
    }
    Ok(())
}

/// Preflight the contribution-only hours records, including channel helpers.
pub(crate) fn preflight_contribution(
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let shape = checked_shape(plan, limits)?;
    for _ in 0..shape.pair_indices {
        charge_record(&measured_variable(1), 1, ChargeMode::Retained, budget)?;
        // Channel variable+fact construction reserves five items; their logical refs total four.
        budget.reserve(0, 1, 0)?;
        charge_record(
            &MeasuredFact {
                kind: MeasuredFactKind::Channel,
            },
            3,
            ChargeMode::Retained,
            budget,
        )?;
        charge_channel(true, ChargeMode::Retained, budget)?;
        charge_channel(false, ChargeMode::Retained, budget)?;
    }
    for hours in &plan.hours {
        charge_hours(hours, ChargeMode::Retained, budget)?;
    }
    let records = shape
        .helper_variables
        .checked_add(shape.constraints)
        .and_then(|value| value.checked_add(shape.provenance))
        .ok_or_else(arithmetic)?;
    budget.steps(
        records
            .checked_add(shape.references)
            .ok_or_else(arithmetic)?,
    )
}

/// Preflight the complete-model hours records in the IR budget. Rank integer channels
/// are reused, so contribution-only pair channels are deliberately excluded.
pub(crate) fn preflight_complete_ir(
    plan: &Plan,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let shape = checked_shape(plan, limits)?;
    for hours in &plan.hours {
        charge_hours(hours, ChargeMode::Ir, budget)?;
    }
    let channel_constraints = shape.pair_indices.checked_mul(2).ok_or_else(arithmetic)?;
    let channel_provenance = shape.pair_indices;
    let channel_references = shape
        .pair_indices
        .checked_mul(channel_reference_count())
        .ok_or_else(arithmetic)?;
    let hour_constraints = shape
        .constraints
        .checked_sub(channel_constraints)
        .ok_or_else(arithmetic)?;
    let hour_provenance = shape
        .provenance
        .checked_sub(channel_provenance)
        .ok_or_else(arithmetic)?;
    let records = shape
        .helper_variables
        .checked_add(hour_constraints)
        .and_then(|value| value.checked_add(hour_provenance))
        .ok_or_else(arithmetic)?;
    let references = shape
        .references
        .checked_sub(channel_references)
        .ok_or_else(arithmetic)?;
    budget.steps(records.checked_add(references).ok_or_else(arithmetic)?)
}
pub(crate) struct ContributionAuxiliaries {
    pub variables: Vec<Variable>,
    pub constraints: Vec<ConstraintRecord>,
    pub provenance: Vec<ProvenanceRecord>,
    pub pair_ints: BTreeMap<usize, IntVariableId>,
}

/// Contribution-only output has no rank integer to reuse. Materialize one explicit 0/1 integer
/// channel for every pair referenced by a `MaximumHours` aggregate and bind it to its Boolean.
pub(crate) fn prepare_contribution(
    plan: &Plan,
    pairs: &[AssignmentPair],
    booleans: &[AssignmentVariable],
    identities: &mut PlanningIdentities,
    budget: &mut OperationBudget<'_>,
) -> Result<ContributionAuxiliaries, AssignmentRuleError> {
    let required = required_indices(plan);
    let mut result = ContributionAuxiliaries {
        variables: Vec::new(),
        constraints: Vec::new(),
        provenance: Vec::new(),
        pair_ints: BTreeMap::new(),
    };
    for index in required {
        budget.step()?;
        let pair = *pairs.get(index).ok_or_else(invalid_record)?;
        let boolean = booleans.get(index).ok_or_else(invalid_record)?;
        let id = IntVariableId::new(identities.derive(
            IdentityKind::Integer,
            &("maximum_hours_channel", pair.person_id, pair.shift_id),
            budget,
        )?)
        .map_err(|_| invalid_id())?;
        let fact_id = ProvenanceId::new(identities.derive(
            IdentityKind::Provenance,
            &("maximum_hours_channel_fact", pair.person_id, pair.shift_id),
            budget,
        )?)
        .map_err(|_| invalid_id())?;
        budget.reserve(1, 5, 512)?;
        let fact = channel_fact(pair, fact_id.clone(), boolean.variable.provenance.clone())?;
        result.provenance.push(fact);
        result.variables.push(Variable::Integer(IntVariable {
            id: id.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
            },
            provenance: fact_id.clone(),
        }));
        for selected in [true, false] {
            budget.step()?;
            let constraint_id = PlanningConstraintId::new(identities.derive(
                IdentityKind::Constraint,
                &(
                    "maximum_hours_channel",
                    pair.person_id,
                    pair.shift_id,
                    selected,
                ),
                budget,
            )?)
            .map_err(|_| invalid_id())?;
            let expression = LinearExpression::new(
                vec![LinearTerm {
                    variable: id.clone(),
                    coefficient: 1,
                }],
                0,
            )
            .map_err(|_| invalid_record())?;
            budget.reserve(1, 3, 384)?;
            result.constraints.push(ConstraintRecord {
                id: constraint_id,
                body: Constraint::LinearComparison(LinearComparison {
                    expression,
                    op: ComparisonOp::Equal,
                    rhs: i64::from(selected),
                }),
                enforcement: vec![Literal {
                    variable: boolean.variable.id.clone(),
                    positive: selected,
                }],
                provenance: fact_id.clone(),
                tags: Vec::new(),
            });
        }
        result.pair_ints.insert(index, id);
    }
    Ok(result)
}

// The two compilation entry points share the same pair-to-integer channel and output buffers.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn compile(
    plan: &Plan,
    source: &BTreeMap<usize, IntVariableId>,
    parents: &BTreeMap<RuleId, ProvenanceId>,
    identities: &mut PlanningIdentities,
    variables: &mut Vec<Variable>,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    for (window_index, hours) in plan.hours.iter().enumerate() {
        budget.step()?;
        let parent = parents.get(&hours.rule).ok_or_else(invalid_record)?.clone();
        match &hours.aggregate {
            HoursAggregate::Sum { terms } => {
                let integer_terms = terms
                    .iter()
                    .map(|term| {
                        Ok((
                            source
                                .get(&term.index)
                                .cloned()
                                .ok_or_else(invalid_record)?,
                            term.seconds,
                            term.remainder,
                        ))
                    })
                    .collect::<Result<Vec<_>, AssignmentRuleError>>()?;
                emit_sum(
                    hours,
                    window_index,
                    &integer_terms,
                    None,
                    parent,
                    identities,
                    variables,
                    constraints,
                    provenance,
                    budget,
                    limits,
                )?;
            }
            HoursAggregate::Union { segments } => {
                let mut segment_ids = Vec::with_capacity(segments.len());
                for (segment_index, segment) in segments.iter().enumerate() {
                    budget.step()?;
                    let id = IntVariableId::new(identities.derive(
                        IdentityKind::Integer,
                        &(
                            "maximum_hours_union_segment",
                            hours.rule,
                            hours.person,
                            hours.bucket_id,
                            hours.window.start,
                            hours.window.end,
                            segment_index,
                            segment.start,
                            segment.end,
                        ),
                        budget,
                    )?)
                    .map_err(|_| invalid_id())?;
                    let fact_id = ProvenanceId::new(identities.derive(
                        IdentityKind::Provenance,
                        &(
                            "maximum_hours_union_segment_fact",
                            hours.rule,
                            hours.person,
                            hours.bucket_id,
                            hours.window.start,
                            hours.window.end,
                            segment_index,
                        ),
                        budget,
                    )?)
                    .map_err(|_| invalid_id())?;
                    budget.reserve(1, 6, 512)?;
                    provenance.push(hours_fact(
                        hours,
                        fact_id.clone(),
                        parent.clone(),
                        helper_message(hours.kind, HoursHelperFact::UnionSegment),
                        Some(segment_index),
                        budget,
                    )?);
                    variables.push(Variable::Integer(IntVariable {
                        id: id.clone(),
                        domain: IntDomain {
                            inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
                        },
                        provenance: fact_id.clone(),
                    }));
                    let mut members = Vec::new();
                    for member in &segment.members {
                        members.push(source.get(member).cloned().ok_or_else(invalid_record)?);
                    }
                    members.sort();
                    members.dedup();
                    for (member_position, member) in members.into_iter().enumerate() {
                        let expression = linear(
                            vec![
                                LinearTerm {
                                    variable: id.clone(),
                                    coefficient: 1,
                                },
                                LinearTerm {
                                    variable: member,
                                    coefficient: -1,
                                },
                            ],
                            0,
                        )?;
                        emit_constraint(
                            hours,
                            window_index,
                            segment_index,
                            member_position,
                            "union_lower",
                            Constraint::LinearComparison(LinearComparison {
                                expression,
                                op: ComparisonOp::GreaterOrEqual,
                                rhs: 0,
                            }),
                            parent.clone(),
                            identities,
                            constraints,
                            provenance,
                            budget,
                        )?;
                    }
                    let mut terms = vec![LinearTerm {
                        variable: id.clone(),
                        coefficient: 1,
                    }];
                    for member in &segment.members {
                        terms.push(LinearTerm {
                            variable: source.get(member).cloned().ok_or_else(invalid_record)?,
                            coefficient: -1,
                        });
                    }
                    emit_constraint(
                        hours,
                        window_index,
                        segment_index,
                        0,
                        "union_upper",
                        Constraint::LinearComparison(LinearComparison {
                            expression: linear(terms, 0)?,
                            op: ComparisonOp::LessOrEqual,
                            rhs: 0,
                        }),
                        parent.clone(),
                        identities,
                        constraints,
                        provenance,
                        budget,
                    )?;
                    segment_ids.push((id, segment.seconds, segment.remainder));
                }
                emit_sum(
                    hours,
                    window_index,
                    &segment_ids,
                    Some(segments.len()),
                    parent,
                    identities,
                    variables,
                    constraints,
                    provenance,
                    budget,
                    limits,
                )?;
            }
        }
    }
    Ok(())
}

// Keep the exact second/remainder equations and their provenance adjacent for review.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn emit_sum(
    hours: &HoursPlan,
    window_index: usize,
    terms: &[(IntVariableId, i64, i64)],
    segment_count: Option<usize>,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    variables: &mut Vec<Variable>,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
    limits: PlanningIrLimitsV1,
) -> Result<(), AssignmentRuleError> {
    let coefficient_limit = limits
        .max_abs_coefficient
        .min(PlanningIrLimitsV1::DEFAULT.max_abs_coefficient);
    let mut early_seconds_min = 0_i128;
    let mut early_seconds_max = 0_i128;
    let mut early_remainders_max = 0_i128;
    let mut fractional = false;
    for (_, seconds, remainder) in terms {
        fractional |= *remainder != 0;
        if i128::from(*seconds).abs() > i128::from(coefficient_limit)
            || i128::from(*remainder).abs() > i128::from(coefficient_limit)
        {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
        let seconds = i128::from(*seconds);
        early_seconds_min = early_seconds_min
            .checked_add(seconds.min(0))
            .ok_or_else(arithmetic)?;
        early_seconds_max = early_seconds_max
            .checked_add(seconds.max(0))
            .ok_or_else(arithmetic)?;
        early_remainders_max = early_remainders_max
            .checked_add(i128::from(*remainder))
            .ok_or_else(arithmetic)?;
    }
    let carry_max = if fractional {
        i64::try_from(terms.len()).map_err(|_| arithmetic())?
    } else {
        0
    };
    let carry_max_i128 = i128::from(carry_max);
    if fractional {
        if coefficient_limit < i64::try_from(NANOS_PER_SECOND).map_err(|_| arithmetic())? {
            return Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ));
        }
        check_expression_range(
            -NANOS_PER_SECOND * carry_max_i128,
            early_remainders_max,
            0,
            limits,
        )?;
    }
    let minimum_target = i64::from(hours.minimum_minutes)
        .checked_mul(60)
        .ok_or_else(arithmetic)?;
    check_expression_range(
        early_seconds_min,
        early_seconds_max
            .checked_add(carry_max_i128)
            .ok_or_else(arithmetic)?,
        minimum_target,
        limits,
    )?;
    if let Some(maximum_minutes) = hours.maximum_minutes {
        let target = i64::from(maximum_minutes)
            .checked_mul(60)
            .ok_or_else(arithmetic)?;
        check_expression_range(
            early_seconds_min,
            early_seconds_max
                .checked_add(carry_max_i128)
                .and_then(|maximum| maximum.checked_add(i128::from(fractional)))
                .ok_or_else(arithmetic)?,
            target,
            limits,
        )?;
    }
    // Integral coefficients need neither Euclidean carry nor a fractional upper-cap bit.
    let auxiliary = if fractional {
        let carry_id = IntVariableId::new(identities.derive(
            IdentityKind::Integer,
            &(
                "maximum_hours_carry",
                hours.rule,
                hours.person,
                hours.bucket_id,
                hours.window.start,
                hours.window.end,
                window_index,
                segment_count,
            ),
            budget,
        )?)
        .map_err(|_| invalid_id())?;
        let carry_fact_id = ProvenanceId::new(identities.derive(
            IdentityKind::Provenance,
            &(
                "maximum_hours_carry_fact",
                hours.rule,
                hours.person,
                hours.bucket_id,
                hours.window.start,
                hours.window.end,
                window_index,
                segment_count,
            ),
            budget,
        )?)
        .map_err(|_| invalid_id())?;
        budget.reserve(1, 6, 512)?;
        provenance.push(hours_fact(
            hours,
            carry_fact_id.clone(),
            parent.clone(),
            helper_message(hours.kind, HoursHelperFact::Carry),
            None,
            budget,
        )?);
        variables.push(Variable::Integer(IntVariable {
            id: carry_id.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange {
                    start: 0,
                    end: carry_max,
                }],
            },
            provenance: carry_fact_id.clone(),
        }));
        // An integral-second cap cannot admit even one extra nanosecond. The 0/1 integer
        // requires one extra second whenever the Euclidean remainder is positive.
        let fractional_id = IntVariableId::new(identities.derive(
            IdentityKind::Integer,
            &(
                "maximum_hours_fractional",
                hours.rule,
                hours.person,
                hours.bucket_id,
                hours.window.start,
                hours.window.end,
                window_index,
                segment_count,
            ),
            budget,
        )?)
        .map_err(|_| invalid_id())?;
        let fractional_fact_id = ProvenanceId::new(identities.derive(
            IdentityKind::Provenance,
            &(
                "maximum_hours_fractional_fact",
                hours.rule,
                hours.person,
                hours.bucket_id,
                hours.window.start,
                hours.window.end,
                window_index,
                segment_count,
            ),
            budget,
        )?)
        .map_err(|_| invalid_id())?;
        budget.reserve(1, 6, 512)?;
        provenance.push(hours_fact(
            hours,
            fractional_fact_id.clone(),
            parent.clone(),
            helper_message(hours.kind, HoursHelperFact::Fractional),
            None,
            budget,
        )?);
        variables.push(Variable::Integer(IntVariable {
            id: fractional_id.clone(),
            domain: IntDomain {
                inclusive_ranges: vec![InclusiveRange { start: 0, end: 1 }],
            },
            provenance: fractional_fact_id,
        }));
        Some((carry_id, fractional_id))
    } else {
        None
    };
    let mut remainder_terms = Vec::new();
    let mut floor_terms = requires_lower(hours).then(Vec::new);
    let mut seconds_terms = Vec::new();
    if let Some((carry_id, _)) = &auxiliary {
        remainder_terms.push(LinearTerm {
            variable: carry_id.clone(),
            coefficient: -1_000_000_000,
        });
        if let Some(floor_terms) = floor_terms.as_mut() {
            floor_terms.push(LinearTerm {
                variable: carry_id.clone(),
                coefficient: 1,
            });
        }
        if hours.maximum_minutes.is_some() {
            seconds_terms.push(LinearTerm {
                variable: carry_id.clone(),
                coefficient: 1,
            });
        }
    }
    for (variable, seconds, remainder) in terms {
        if *remainder != 0 {
            remainder_terms.push(LinearTerm {
                variable: variable.clone(),
                coefficient: *remainder,
            });
        }
        if *seconds != 0 {
            if let Some(floor_terms) = floor_terms.as_mut() {
                floor_terms.push(LinearTerm {
                    variable: variable.clone(),
                    coefficient: *seconds,
                });
            }
            if hours.maximum_minutes.is_some() {
                seconds_terms.push(LinearTerm {
                    variable: variable.clone(),
                    coefficient: *seconds,
                });
            }
        }
    }
    if let Some((_, fractional_id)) = auxiliary {
        let remainder_lower_terms = remainder_terms.clone();
        if hours.maximum_minutes.is_some() {
            seconds_terms.push(LinearTerm {
                variable: fractional_id.clone(),
                coefficient: 1,
            });
        }
        remainder_terms.push(LinearTerm {
            variable: fractional_id,
            coefficient: -MAX_NANOS_REMAINDER,
        });
        emit_constraint(
            hours,
            window_index,
            0,
            0,
            "remainder_lower",
            Constraint::LinearComparison(LinearComparison {
                expression: linear(remainder_lower_terms, 0)?,
                op: ComparisonOp::GreaterOrEqual,
                rhs: 0,
            }),
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
        )?;
        emit_constraint(
            hours,
            window_index,
            1,
            0,
            "remainder_upper",
            Constraint::LinearComparison(LinearComparison {
                expression: linear(remainder_terms, 0)?,
                op: ComparisonOp::LessOrEqual,
                rhs: 0,
            }),
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
        )?;
    }
    if let Some(floor_terms) = floor_terms {
        emit_constraint(
            hours,
            window_index,
            if let Some(segments) = segment_count {
                segments + 1
            } else {
                2
            },
            0,
            "seconds_lower",
            Constraint::LinearComparison(LinearComparison {
                expression: linear(floor_terms, 0)?,
                op: ComparisonOp::GreaterOrEqual,
                rhs: minimum_target,
            }),
            parent.clone(),
            identities,
            constraints,
            provenance,
            budget,
        )?;
    }
    if let Some(maximum_minutes) = hours.maximum_minutes {
        let target = i64::from(maximum_minutes)
            .checked_mul(60)
            .ok_or_else(arithmetic)?;
        emit_constraint(
            hours,
            window_index,
            if let Some(segments) = segment_count {
                segments + 1
            } else {
                3
            },
            0,
            "seconds_upper",
            Constraint::LinearComparison(LinearComparison {
                expression: linear(seconds_terms, 0)?,
                op: ComparisonOp::LessOrEqual,
                rhs: target,
            }),
            parent,
            identities,
            constraints,
            provenance,
            budget,
        )?;
    }
    Ok(())
}

// Constraint identity and its source fact must be published as one bounded pair.
#[allow(clippy::too_many_arguments)]
fn emit_constraint(
    hours: &HoursPlan,
    window_index: usize,
    component_index: usize,
    subcomponent_index: usize,
    kind: &'static str,
    body: Constraint,
    parent: ProvenanceId,
    identities: &mut PlanningIdentities,
    constraints: &mut Vec<ConstraintRecord>,
    provenance: &mut Vec<ProvenanceRecord>,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let id = PlanningConstraintId::new(identities.derive(
        IdentityKind::Constraint,
        &(
            "maximum_hours",
            kind,
            hours.rule,
            hours.person,
            hours.bucket_id,
            hours.window.start,
            hours.window.end,
            window_index,
            component_index,
            subcomponent_index,
        ),
        budget,
    )?)
    .map_err(|_| invalid_id())?;
    let fact_id = ProvenanceId::new(identities.derive(
        IdentityKind::Provenance,
        &(
            "maximum_hours_constraint_fact",
            kind,
            hours.rule,
            hours.person,
            hours.bucket_id,
            hours.window.start,
            hours.window.end,
            window_index,
            component_index,
            subcomponent_index,
        ),
        budget,
    )?)
    .map_err(|_| invalid_id())?;
    budget.reserve(1, 5, 768)?;
    provenance.push(hours_fact(
        hours,
        fact_id.clone(),
        parent,
        match hours.kind {
            HoursPlanKind::MaximumHours => "official.workforce.maximum_hours",
            HoursPlanKind::WorkloadQuota => "official.workforce.workload_quota",
        },
        Some(component_index),
        budget,
    )?);
    constraints.push(ConstraintRecord {
        id,
        body,
        enforcement: Vec::new(),
        provenance: fact_id,
        tags: Vec::new(),
    });
    Ok(())
}

fn hours_fact(
    hours: &HoursPlan,
    id: ProvenanceId,
    parent: ProvenanceId,
    message_key: &str,
    component: Option<usize>,
    budget: &mut OperationBudget<'_>,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    let mut refs = vec![
        entity("person", EntityId::from_uuid(hours.person.as_uuid()))?,
        entity("workload_bucket", hours.bucket_id.as_entity_id())?,
    ];
    if let Some(calendar_id) = hours.window.calendar_id {
        refs.push(entity("calendar", calendar_id.as_entity_id())?);
    }
    refs.sort_unstable();
    let mut parameters = BTreeMap::new();
    if matches!(hours.kind, HoursPlanKind::WorkloadQuota) {
        parameters.insert(
            "minimum_minutes".to_owned(),
            ProvenanceParameter::Integer(i64::from(hours.minimum_minutes)),
        );
    }
    if let Some(maximum_minutes) = hours.maximum_minutes {
        parameters.insert(
            "maximum_minutes".to_owned(),
            ProvenanceParameter::Integer(i64::from(maximum_minutes)),
        );
    }
    parameters.insert(
        "window_start".to_owned(),
        ProvenanceParameter::Text(hours.window.start.to_string()),
    );
    parameters.insert(
        "window_end".to_owned(),
        ProvenanceParameter::Text(hours.window.end.to_string()),
    );
    parameters.insert(
        "membership".to_owned(),
        ProvenanceParameter::Text(format!("{:?}", hours.window.membership)),
    );
    parameters.insert(
        "measurement".to_owned(),
        ProvenanceParameter::Text(format!("{:?}", hours.measurement)),
    );
    parameters.insert(
        "overlapping_contribution".to_owned(),
        ProvenanceParameter::Text(format!("{:?}", hours.contribution)),
    );
    if let Some(component) = component {
        parameters.insert(
            "component".to_owned(),
            ProvenanceParameter::Integer(i64::try_from(component).map_err(|_| arithmetic())?),
        );
    }
    budget.sort_work(refs.len())?;
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Derived,
        source_id: hours.rule.to_string(),
        entity_refs: refs,
        message_key: message_key.to_owned(),
        parameters,
        parent: Some(parent),
    })
}

fn channel_fact(
    pair: AssignmentPair,
    id: ProvenanceId,
    parent: ProvenanceId,
) -> Result<ProvenanceRecord, AssignmentRuleError> {
    Ok(ProvenanceRecord {
        id,
        source_kind: ProvenanceSourceKind::Derived,
        source_id: pair.person_id.to_string(),
        entity_refs: vec![
            entity("person", EntityId::from_uuid(pair.person_id.as_uuid()))?,
            entity("shift", pair.shift_id.as_entity_id())?,
        ],
        message_key: "official.workforce.maximum_hours.channel".to_owned(),
        parameters: BTreeMap::new(),
        parent: Some(parent),
    })
}

fn entity(kind: &str, id: EntityId) -> Result<DomainEntityRef, AssignmentRuleError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(format!("official.workforce.{kind}"))
            .map_err(|_| invalid_record())?,
        id: DomainEntityId::new(format!("official.workforce.{id}"))
            .map_err(|_| invalid_record())?,
    })
}

fn linear(terms: Vec<LinearTerm>, constant: i64) -> Result<LinearExpression, AssignmentRuleError> {
    LinearExpression::new(terms, constant).map_err(|_| invalid_record())
}

fn required_indices(plan: &Plan) -> BTreeSet<usize> {
    let mut indices = BTreeSet::new();
    for hours in &plan.hours {
        match &hours.aggregate {
            HoursAggregate::Sum { terms } => {
                indices.extend(terms.iter().map(|term| term.index));
            }
            HoursAggregate::Union { segments } => {
                for segment in segments {
                    indices.extend(segment.members.iter().copied());
                }
            }
        }
    }
    indices
}

fn invalid_record() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

fn invalid_id() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidIdentifier)
}

fn arithmetic() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_calendar_compilation_charges_one_scan() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = crate::test_support::fixture()?;
        document
            .domain
            .entities
            .remove(&crate::test_support::id(6).parse()?);
        document.domain.locked_assignments.clear();
        let calendar_id: WorkCalendarId = crate::test_support::id(2).parse()?;
        let start: jiff::civil::Date = "2026-10-01".parse()?;
        let intervals = (0..100)
            .map(|day| {
                Ok(crate::model::LocalInterval {
                    starts_at: eutheto_types::LocalWallTime::from_datetime(
                        start
                            .checked_add(jiff::Span::new().days(day))?
                            .to_datetime(jiff::civil::Time::MIN),
                    ),
                    ends_at: eutheto_types::LocalWallTime::from_datetime(
                        start
                            .checked_add(jiff::Span::new().days(day + 1))?
                            .to_datetime(jiff::civil::Time::MIN),
                    ),
                })
            })
            .collect::<Result<Vec<_>, jiff::Error>>()?;
        document
            .domain
            .entities
            .get_mut(&calendar_id.as_entity_id())
            .ok_or("calendar")?["period"] =
            serde_json::to_value(crate::model::CalendarPeriod::Custom { intervals })?;
        let input = AssignmentInput::new(&document, &mut OperationBudget::evaluation(None))?;
        let shift = input.shifts.first().ok_or("shift")?;
        for membership in [
            WindowMembership::StartInstant,
            WindowMembership::Intersection,
        ] {
            let mut budget = OperationBudget::evaluation(None);
            // Enough for one 100-interval scan and one member's aggregation, not two scans.
            budget.steps(crate::assignment_rules::MAX_WORK_STEPS - 150)?;
            let windows = calendar_windows(
                &document,
                &input,
                &[Member { index: 0, shift }],
                calendar_id,
                membership,
                WorkloadMeasurement::ElapsedMinutes,
                OverlappingContribution::Sum,
                &mut budget,
            )?;
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].0.start, "2026-11-01T04:00:00Z".parse()?);
            assert_eq!(windows[0].0.end, "2026-11-02T05:00:00Z".parse()?);
            let HoursAggregate::Sum { terms } = &windows[0].1 else {
                return Err("expected elapsed sum".into());
            };
            assert_eq!(terms.len(), 1);
            assert_eq!(terms[0].seconds, 7200);
            assert_eq!(terms[0].remainder, 0);
        }
        Ok(())
    }

    fn hours_plan(
        kind: HoursPlanKind,
        contribution: OverlappingContribution,
        fractional: bool,
    ) -> Result<Plan, Box<dyn std::error::Error>> {
        let rule = crate::test_support::id(20).parse()?;
        let remainder = i64::from(fractional);
        let mut aggregate = match contribution {
            OverlappingContribution::Sum => HoursAggregate::Sum {
                terms: vec![
                    HoursTerm {
                        index: 0,
                        seconds: 3600,
                        remainder: 0,
                    },
                    HoursTerm {
                        index: 1,
                        seconds: 3600,
                        remainder,
                    },
                ],
            },
            OverlappingContribution::Union => HoursAggregate::Union {
                segments: vec![
                    HoursSegment {
                        start: "2026-11-01T08:00:00Z".parse()?,
                        end: "2026-11-01T08:30:00Z".parse()?,
                        seconds: 1800,
                        remainder: 0,
                        members: vec![0],
                    },
                    HoursSegment {
                        start: "2026-11-01T08:30:00Z".parse()?,
                        end: "2026-11-01T09:00:00Z".parse()?,
                        seconds: 1800,
                        remainder: 0,
                        members: vec![0, 1],
                    },
                    HoursSegment {
                        start: "2026-11-01T09:00:00Z".parse()?,
                        end: if fractional {
                            "2026-11-01T09:30:00.000000001Z"
                        } else {
                            "2026-11-01T09:30:00Z"
                        }
                        .parse()?,
                        seconds: 1800,
                        remainder,
                        members: vec![1],
                    },
                ],
            },
        };
        if let HoursAggregate::Union { segments } = &mut aggregate {
            for index in 2..9 {
                segments.push(HoursSegment {
                    start: format!(
                        "2026-11-01T{:02}:{:02}:00Z",
                        10 + (index - 2) / 2,
                        (index - 2) % 2 * 30,
                    )
                    .parse()?,
                    end: format!(
                        "2026-11-01T{:02}:{:02}:00Z",
                        10 + (index - 1) / 2,
                        (index - 1) % 2 * 30,
                    )
                    .parse()?,
                    seconds: 1800,
                    remainder: 0,
                    members: vec![index],
                });
            }
        }
        let plan = Plan {
            definitions: Vec::new(),
            constraints: Vec::new(),
            weekends: Vec::new(),
            parents: BTreeMap::new(),
            hours: vec![HoursPlan {
                rule,
                person: crate::test_support::id(1).parse()?,
                bucket_id: crate::test_support::id(3).parse()?,
                measurement: WorkloadMeasurement::ElapsedMinutes,
                contribution,
                minimum_minutes: 0,
                maximum_minutes: Some(90),
                kind,
                window: HoursWindow {
                    calendar_id: Some(crate::test_support::id(2).parse()?),
                    start: "2026-11-01T00:00:00Z".parse()?,
                    end: "2026-11-02T00:00:00Z".parse()?,
                    duration_minutes: None,
                    membership: WindowMembership::Intersection,
                },
                aggregate,
            }],
        };
        Ok(plan)
    }

    #[test]
    fn measured_hours_cover_every_emitted_record() -> Result<(), Box<dyn std::error::Error>> {
        for kind in [HoursPlanKind::MaximumHours, HoursPlanKind::WorkloadQuota] {
            for contribution in [OverlappingContribution::Sum, OverlappingContribution::Union] {
                for fractional in [false, true] {
                    let plan = hours_plan(kind, contribution, fractional)?;
                    let limits = PlanningIrLimitsV1::DEFAULT;
                    let shape = checked_shape(&plan, limits)?;
                    let mut budget = OperationBudget::analysis(None, limits);
                    preflight_contribution(&plan, &mut budget, limits)?;
                    preflight_complete_ir(&plan, &mut budget, limits)?;
                    budget.reserve_ir_retention()?;
                    let source_count = match contribution {
                        OverlappingContribution::Sum => 2,
                        OverlappingContribution::Union => 9,
                    };
                    let source = (0..source_count)
                        .map(|index| {
                            Ok((
                                index,
                                IntVariableId::new(format!("official.workforce.int.{index:064x}"))?,
                            ))
                        })
                        .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;
                    let parents = BTreeMap::from([(
                        plan.hours[0].rule,
                        ProvenanceId::new(MEASURED_PROVENANCE_ID)?,
                    )]);
                    let mut variables = Vec::new();
                    let mut constraints = Vec::new();
                    let mut provenance = Vec::new();
                    compile(
                        &plan,
                        &source,
                        &parents,
                        &mut PlanningIdentities::default(),
                        &mut variables,
                        &mut constraints,
                        &mut provenance,
                        &mut budget,
                        limits,
                    )?;
                    let mut actual_bytes = 0;
                    for variable in &variables {
                        actual_bytes =
                            add(actual_bytes, count(serde_json::to_vec(variable)?.len())?)?;
                    }
                    for constraint in &constraints {
                        actual_bytes =
                            add(actual_bytes, count(serde_json::to_vec(constraint)?.len())?)?;
                    }
                    for fact in &provenance {
                        actual_bytes = add(actual_bytes, count(serde_json::to_vec(fact)?.len())?)?;
                    }
                    assert_eq!(budget.reserved_ir_bytes(), actual_bytes);
                    assert_eq!(shape.helper_variables, count(variables.len())?);
                    assert_eq!(
                        shape.constraints - shape.pair_indices * 2,
                        count(constraints.len())?
                    );
                    assert_eq!(
                        shape.provenance - shape.pair_indices,
                        count(provenance.len())?
                    );
                }
            }
        }
        Ok(())
    }
}
