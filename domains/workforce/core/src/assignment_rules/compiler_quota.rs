use super::super::reporting::{ReportingPeriod, ReportingWindows};
use super::{
    AssignmentInput, AssignmentPair, AssignmentRuleError, AssignmentRuleLimit, OperationBudget,
    Plan, PlannedConstraint, Predicate, WorkforceEntity, WorkforceRule, WorkloadQuotaMeasure,
    append, count, invalid, person_scope, shift_scope,
};
use crate::{
    ids::WorkCalendarId,
    model::{OverlappingContribution, WindowMembership},
    temporal::{ResolvedShift, resolve_calendar_for_calendar},
};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{Horizon, ScenarioDocument};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
};

use super::hours::{
    self, HoursAggregate, HoursPlan, HoursPlanKind, HoursWindow, IntervalMember, Member, WindowKey,
};

/// Plans an independently-authored Required per-person assignment count or minute quota.
///
/// Period ownership is intentionally sourced from every matching resolved shift, not from the
/// candidate graph. Candidate pruning only controls which Boolean decisions can receive credit.
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
    let WorkforceRule::WorkloadQuota(quota) = rule else {
        return Err(invalid());
    };
    if !matches!(
        input.domain.entities.get(&quota.calendar_id.as_entity_id()),
        Some(WorkforceEntity::Calendar(_))
    ) {
        return Err(invalid());
    }
    let bucket = match quota.measure {
        WorkloadQuotaMeasure::AssignmentCount {} => None,
        WorkloadQuotaMeasure::BucketMinutes { bucket_id } => {
            let Some(WorkforceEntity::WorkloadBucket(bucket)) =
                input.domain.entities.get(&bucket_id.as_entity_id())
            else {
                return Err(invalid());
            };
            Some((
                bucket_id,
                bucket.measurement,
                bucket.overlapping_contribution,
            ))
        }
    };
    let mut source = Vec::<&ResolvedShift>::new();
    for shift in &input.shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if shift_scope(&quota.scope, shift, &metadata, budget)?
            && bucket.is_none_or(|(bucket_id, _, _)| {
                metadata
                    .assignment_type
                    .workload_bucket_ids
                    .contains(&bucket_id)
            })
        {
            budget.reserve(0, 1, 32)?;
            source.push(shift);
        }
    }
    let owned = owned_windows(
        document,
        input,
        &source,
        quota.calendar_id,
        quota.membership,
        budget,
    )?;
    let mut any = false;
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !person_scope(&quota.scope, person, budget)? {
            continue;
        }
        match bucket {
            None => {
                plan_count(
                    document,
                    input,
                    candidates,
                    &owned,
                    *person_id,
                    &quota.scope,
                    quota,
                    plan,
                    budget,
                )?;
                any |= !owned.is_empty();
            }
            Some((bucket_id, measurement, contribution)) => {
                let mut members = Vec::new();
                for (index, pair) in candidates.iter().enumerate() {
                    budget.step()?;
                    if pair.person_id != *person_id {
                        continue;
                    }
                    let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
                    let metadata = input.metadata(shift)?;
                    if !shift_scope(&quota.scope, shift, &metadata, budget)?
                        || !metadata
                            .assignment_type
                            .workload_bucket_ids
                            .contains(&bucket_id)
                    {
                        continue;
                    }
                    budget.reserve(0, 1, 64)?;
                    members.push(Member { index, shift });
                }
                let aggregates = aggregate_quota_windows(
                    document,
                    input,
                    &owned,
                    &members,
                    quota.calendar_id,
                    quota.membership,
                    measurement,
                    contribution,
                    budget,
                )?;
                for (window, aggregate) in aggregates {
                    budget.step()?;
                    let Some(aggregate) = aggregate else {
                        if quota.minimum == 0 {
                            continue;
                        }
                        append(
                            plan,
                            PlannedConstraint {
                                rule: quota.id,
                                predicate: Predicate::WorkloadQuota {
                                    person: *person_id,
                                    calendar_id: quota.calendar_id,
                                    period: period(window)?,
                                    minimum: quota.minimum,
                                    maximum: quota.maximum,
                                    measure: quota.measure,
                                },
                                population: Vec::new(),
                                impossible: true,
                            },
                            budget,
                        )?;
                        any = true;
                        continue;
                    };
                    hours::validate_aggregate(&aggregate, limits, quota.minimum, quota.maximum)?;
                    budget.reserve(1, 8, 768)?;
                    plan.hours.push(HoursPlan {
                        rule: quota.id,
                        person: *person_id,
                        bucket_id,
                        measurement,
                        contribution,
                        minimum_minutes: quota.minimum,
                        maximum_minutes: quota.maximum,
                        kind: HoursPlanKind::WorkloadQuota,
                        window,
                        aggregate,
                    });
                    any = true;
                }
            }
        }
    }
    if any {
        plan.parents.insert(quota.id, "workload_quota");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_count(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    owned: &[HoursWindow],
    person_id: eutheto_types::PersonId,
    scope: &crate::model::Scope,
    quota: &crate::model::WorkloadQuotaRule,
    plan: &mut Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut periods = BTreeMap::<WindowKey, Vec<usize>>::new();
    for window in owned {
        budget.step()?;
        budget.reserve(0, 1, 64)?;
        periods.entry(key(*window)).or_default();
    }
    let mut reporting = ReportingWindows::new();
    for (index, pair) in candidates.iter().enumerate() {
        budget.step()?;
        if pair.person_id != person_id {
            continue;
        }
        let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
        let metadata = input.metadata(shift)?;
        if !shift_scope(scope, shift, &metadata, budget)? {
            continue;
        }
        for window in owned {
            budget.step()?;
            let matches = match quota.membership {
                WindowMembership::ReportingDate => {
                    let period = reporting.resolve(
                        document,
                        input,
                        quota.calendar_id,
                        shift.reporting_date,
                        budget,
                    )?;
                    period.is_some_and(|period| {
                        period.start == window.start && period.end == window.end
                    })
                }
                WindowMembership::StartInstant | WindowMembership::Intersection => {
                    member_of(shift, *window, quota.membership, budget)?
                }
            };
            if matches {
                budget.reserve(0, 1, count(size_of::<usize>())?)?;
                periods.entry(key(*window)).or_default().push(index);
            }
        }
    }
    for (window, population) in periods {
        budget.step()?;
        let impossible =
            population.is_empty() || u64::from(quota.minimum) > population.len() as u64;
        if impossible && quota.minimum == 0 {
            continue;
        }
        append(
            plan,
            PlannedConstraint {
                rule: quota.id,
                predicate: Predicate::WorkloadQuota {
                    person: person_id,
                    calendar_id: quota.calendar_id,
                    period: period_from_key(window)?,
                    minimum: quota.minimum,
                    maximum: quota.maximum,
                    measure: quota.measure,
                },
                population,
                impossible,
            },
            budget,
        )?;
    }
    Ok(())
}

/// Base horizon periods plus only source-reached extensions allowed by quota membership.
fn owned_windows(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    source: &[&ResolvedShift],
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<HoursWindow>, AssignmentRuleError> {
    let Some(WorkforceEntity::Calendar(calendar)) =
        input.domain.entities.get(&calendar_id.as_entity_id())
    else {
        return Err(invalid());
    };
    let base = resolve_calendar_for_calendar(
        document,
        calendar_id,
        calendar,
        document.settings.horizon,
        &mut |step| budget.calendar_step(step),
    )?;
    let mut keys = BTreeSet::new();
    budget.steps(count(base.len())?)?;
    for window in base {
        budget.expanded_interval()?;
        insert_owned_key(&mut keys, hours::calendar_key(window), budget)?;
    }
    let mut reporting = ReportingWindows::new();
    for shift in source {
        budget.step()?;
        match membership {
            WindowMembership::ReportingDate => {
                if let Some(period) =
                    reporting.resolve(document, input, calendar_id, shift.reporting_date, budget)?
                {
                    insert_owned_key(
                        &mut keys,
                        WindowKey {
                            calendar_id: Some(period.calendar_id),
                            start: period.start,
                            end: period.end,
                            duration_minutes: None,
                        },
                        budget,
                    )?;
                }
            }
            WindowMembership::StartInstant => {}
            WindowMembership::Intersection => {
                let range = Horizon::new(
                    shift.interval.starts_at.instant,
                    shift.interval.ends_at.instant,
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
                    insert_owned_key(&mut keys, hours::calendar_key(window), budget)?;
                }
            }
        }
    }
    budget.sort_work(keys.len())?;
    budget.reserve(
        0,
        count(keys.len())?,
        count(keys.len())?
            .checked_mul(count(size_of::<HoursWindow>())?)
            .ok_or(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ))?,
    )?;
    Ok(keys.into_iter().map(|key| key.window(membership)).collect())
}

fn insert_owned_key(
    keys: &mut BTreeSet<WindowKey>,
    key: WindowKey,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    if !keys.contains(&key) {
        budget.reserve(
            1,
            1,
            count(size_of::<WindowKey>() + 3 * size_of::<usize>())?,
        )?;
        keys.insert(key);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn aggregate_quota_windows(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    owned: &[HoursWindow],
    members: &[Member<'_>],
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    measurement: crate::model::WorkloadMeasurement,
    contribution: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<(HoursWindow, Option<HoursAggregate>)>, AssignmentRuleError> {
    let mut grouped = BTreeMap::<WindowKey, Vec<IntervalMember>>::new();
    let mut reporting = ReportingWindows::new();
    for member in members {
        budget.step()?;
        for window in owned {
            budget.step()?;
            if key(*window).calendar_id != Some(calendar_id) {
                continue;
            }
            let interval = match membership {
                WindowMembership::ReportingDate => {
                    let Some(period) = reporting.resolve(
                        document,
                        input,
                        calendar_id,
                        member.shift.reporting_date,
                        budget,
                    )?
                    else {
                        continue;
                    };
                    if key(*window)
                        != (WindowKey {
                            calendar_id: Some(period.calendar_id),
                            start: period.start,
                            end: period.end,
                            duration_minutes: None,
                        })
                    {
                        continue;
                    }
                    Some((
                        member.shift.interval.starts_at.instant,
                        member.shift.interval.ends_at.instant,
                    ))
                }
                WindowMembership::StartInstant => {
                    member_interval(member.shift, *window, WindowMembership::StartInstant)
                }
                WindowMembership::Intersection => {
                    member_interval(member.shift, *window, WindowMembership::Intersection)
                }
            };
            let Some((start, end)) = interval else {
                continue;
            };
            hours::insert_interval(
                &mut grouped,
                key(*window),
                IntervalMember {
                    index: member.index,
                    start,
                    end,
                    scheduled: member.shift.interval.scheduled_duration(),
                },
                budget,
            )?;
            if membership != WindowMembership::Intersection {
                break;
            }
        }
    }
    let aggregates =
        hours::grouped_aggregates(grouped, membership, measurement, contribution, budget)?;
    let mut by_key = BTreeMap::new();
    for (window, aggregate) in aggregates {
        budget.reserve(1, 1, count(size_of::<(WindowKey, HoursAggregate)>())?)?;
        by_key.insert(key(window), aggregate);
    }
    budget.reserve(
        0,
        count(owned.len())?,
        count(owned.len())?
            .checked_mul(count(size_of::<(HoursWindow, Option<HoursAggregate>)>())?)
            .ok_or(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ))?,
    )?;
    Ok(owned
        .iter()
        .map(|window| (*window, by_key.remove(&key(*window))))
        .collect())
}

fn member_interval(
    shift: &ResolvedShift,
    window: HoursWindow,
    membership: WindowMembership,
) -> Option<(
    eutheto_types::Rfc3339Timestamp,
    eutheto_types::Rfc3339Timestamp,
)> {
    match membership {
        WindowMembership::ReportingDate => Some((
            shift.interval.starts_at.instant,
            shift.interval.ends_at.instant,
        )),
        WindowMembership::StartInstant => {
            let instant = shift.interval.starts_at.instant;
            (window.start <= instant && instant < window.end).then_some((
                shift.interval.starts_at.instant,
                shift.interval.ends_at.instant,
            ))
        }
        WindowMembership::Intersection => hours::clip(
            shift.interval.starts_at.instant,
            shift.interval.ends_at.instant,
            window.start,
            window.end,
        ),
    }
}

fn member_of(
    shift: &ResolvedShift,
    window: HoursWindow,
    membership: WindowMembership,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    Ok(member_interval(shift, window, membership).is_some())
}

fn key(window: HoursWindow) -> WindowKey {
    WindowKey {
        calendar_id: window.calendar_id,
        start: window.start,
        end: window.end,
        duration_minutes: window.duration_minutes,
    }
}

fn period(window: HoursWindow) -> Result<ReportingPeriod, AssignmentRuleError> {
    period_from_key(key(window))
}

fn period_from_key(key: WindowKey) -> Result<ReportingPeriod, AssignmentRuleError> {
    Ok(ReportingPeriod {
        calendar_id: key.calendar_id.ok_or_else(invalid)?,
        start: key.start,
        end: key.end,
    })
}
