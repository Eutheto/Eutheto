use super::super::{
    AssignmentRuleError, AssignmentRuleLimit, InstantInterval,
    budget::{OperationBudget, count},
    input::AssignmentInput,
    reporting::ReportingWindows,
};
use super::{QuotaWitness, Selected, Summary, Witness, arithmetic, invalid, predicates};
use crate::{
    ids::{WorkCalendarId, WorkloadBucketId},
    model::{
        AssignmentPair, OverlappingContribution, Scope, WindowMembership, WorkforceEntity,
        WorkforceRule, WorkloadMeasurement,
    },
    temporal::resolve_calendar_for_calendar,
};
use eutheto_types::{EntityId, Horizon, PersonId, Rfc3339Timestamp, ScenarioDocument};
use jiff::SignedDuration;
use std::{collections::BTreeSet, mem::size_of};

const NANOS_PER_SECOND: i128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Window {
    calendar_id: WorkCalendarId,
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
    shift_id: crate::ids::ShiftId,
    interval: InstantInterval,
    elapsed_nanos: i128,
    scheduled_nanos: i128,
}

#[allow(clippy::too_many_lines)]
pub(super) fn evaluate(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::WorkloadQuota(quota) = rule else {
        return Err(invalid());
    };
    let bucket = match quota.measure {
        crate::model::WorkloadQuotaMeasure::AssignmentCount {} => None,
        crate::model::WorkloadQuotaMeasure::BucketMinutes { bucket_id } => {
            let Some(WorkforceEntity::WorkloadBucket(bucket)) =
                input.domain.entities.get(&bucket_id.as_entity_id())
            else {
                return Err(invalid());
            };
            if bucket.measurement == WorkloadMeasurement::AssignmentCount
                || (bucket.overlapping_contribution == OverlappingContribution::Union
                    && bucket.measurement != WorkloadMeasurement::ElapsedMinutes)
                || (quota.membership == WindowMembership::Intersection
                    && bucket.measurement != WorkloadMeasurement::ElapsedMinutes)
            {
                return Err(invalid());
            }
            Some((
                bucket_id,
                bucket.measurement,
                bucket.overlapping_contribution,
            ))
        }
    };
    let owner = EntityId::from_uuid(quota.id.as_uuid());
    let mut source = Vec::<&crate::temporal::ResolvedShift>::new();
    for shift in &input.shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if predicates::shift_matches(shift, &metadata, &quota.scope, budget)?
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
    let quota_windows = owned_windows(
        document,
        input,
        &source,
        quota.calendar_id,
        quota.membership,
        budget,
    )?;
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, &quota.scope, budget)? {
            continue;
        }
        let chosen: &[&crate::temporal::ResolvedShift] =
            selected.by_person.get(person_id).map_or(&[], Vec::as_slice);
        for &window in &quota_windows {
            budget.step()?;
            match bucket {
                None => {
                    let (actual, first_shift) = count_assignments(
                        document,
                        input,
                        chosen,
                        &quota.scope,
                        quota.calendar_id,
                        quota.membership,
                        window,
                        budget,
                    )?;
                    let failed = actual < u64::from(quota.minimum)
                        || quota.maximum.is_some_and(|max| actual > u64::from(max));
                    let mut witness = quota_witness(
                        *person_id,
                        owner,
                        window,
                        quota.minimum,
                        quota.maximum,
                        Some(actual),
                        None,
                    );
                    witness.quota_count = Some(super::QuotaCountWitness {
                        minimum: quota.minimum,
                        maximum: quota.maximum,
                        actual,
                        calendar: quota.calendar_id,
                    });
                    if failed {
                        witness.shift = first_shift;
                    }
                    let members = if failed && summary.collecting_repair() {
                        association_members(
                            document,
                            input,
                            selected,
                            *person_id,
                            &quota.scope,
                            quota.calendar_id,
                            quota.membership,
                            window,
                            None,
                            budget,
                        )?
                    } else {
                        Vec::new()
                    };
                    let collect_repair = summary.collecting_repair();
                    summary.predicate_with_members(
                        failed,
                        &witness,
                        collect_repair.then_some(members),
                        "workload_quota_assignment_count",
                        budget,
                    )?;
                }
                Some((bucket_id, measurement, contribution)) => {
                    let (actual, witness_shift) = minute_total(
                        document,
                        input,
                        chosen,
                        &quota.scope,
                        quota.calendar_id,
                        quota.membership,
                        window,
                        bucket_id,
                        measurement,
                        contribution,
                        budget,
                    )?;
                    let lower = floor_seconds(actual) >= i128::from(quota.minimum) * 60;
                    let upper = quota
                        .maximum
                        .is_none_or(|max| ceil_seconds(actual) <= i128::from(max) * 60);
                    let witness = quota_witness(
                        *person_id,
                        owner,
                        window,
                        quota.minimum,
                        quota.maximum,
                        None,
                        Some(QuotaWitness {
                            minimum_minutes: quota.minimum,
                            maximum_minutes: quota.maximum,
                            actual: signed_duration(actual)?,
                            bucket: bucket_id,
                            calendar: quota.calendar_id,
                        }),
                    );
                    let mut witness = witness;
                    witness.shift = witness_shift;
                    let failed = !(lower && upper);
                    let members = if failed && summary.collecting_repair() {
                        association_members(
                            document,
                            input,
                            selected,
                            *person_id,
                            &quota.scope,
                            quota.calendar_id,
                            quota.membership,
                            window,
                            Some(bucket_id),
                            budget,
                        )?
                    } else {
                        Vec::new()
                    };
                    let collect_repair = summary.collecting_repair();
                    summary.predicate_with_members(
                        failed,
                        &witness,
                        collect_repair.then_some(members),
                        "workload_quota_bucket_minutes",
                        budget,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn quota_witness(
    person: PersonId,
    owner: EntityId,
    window: Window,
    minimum: u32,
    maximum: Option<u32>,
    actual: Option<u64>,
    quota: Option<QuotaWitness>,
) -> Witness {
    Witness {
        person: Some(person),
        other_person: None,
        shift: None,
        other_shift: None,
        owner,
        owner_kind: "rule",
        reason: "workload_quota",
        minimum_rank: 0,
        minimum_hash: None,
        lower: Some(u64::from(minimum)),
        upper: maximum.map(u64::from),
        actual,
        interval: None,
        period: Some((window.start, window.end)),
        consecutive_start_date: None,
        consecutive_end_date: None,
        rest: None,
        transition: None,
        hours: None,
        quota,
        quota_count: None,
    }
}

fn owned_windows(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    source: &[&crate::temporal::ResolvedShift],
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<Window>, AssignmentRuleError> {
    let Some(WorkforceEntity::Calendar(calendar)) =
        input.domain.entities.get(&calendar_id.as_entity_id())
    else {
        return Err(invalid());
    };
    let mut keys = BTreeSet::new();
    let base = resolve_calendar_for_calendar(
        document,
        calendar_id,
        calendar,
        document.settings.horizon,
        &mut |step| budget.calendar_step(step),
    )?;
    budget.steps(count(base.len())?)?;
    for period in base {
        budget.expanded_interval()?;
        insert_window(
            &mut keys,
            Window {
                calendar_id: period.calendar_id,
                start: period.interval.starts_at.instant,
                end: period.interval.ends_at.instant,
            },
            budget,
        )?;
    }
    let mut reporting = ReportingWindows::new();
    for shift in source {
        budget.step()?;
        match membership {
            WindowMembership::ReportingDate => {
                if let Some(period) =
                    reporting.resolve(document, input, calendar_id, shift.reporting_date, budget)?
                {
                    insert_window(
                        &mut keys,
                        Window {
                            calendar_id: period.calendar_id,
                            start: period.start,
                            end: period.end,
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
                let periods = resolve_calendar_for_calendar(
                    document,
                    calendar_id,
                    calendar,
                    range,
                    &mut |step| budget.calendar_step(step),
                )?;
                budget.steps(count(periods.len())?)?;
                for period in periods {
                    budget.expanded_interval()?;
                    insert_window(
                        &mut keys,
                        Window {
                            calendar_id: period.calendar_id,
                            start: period.interval.starts_at.instant,
                            end: period.interval.ends_at.instant,
                        },
                        budget,
                    )?;
                }
            }
        }
    }
    budget.sort_work(keys.len())?;
    budget.reserve(
        0,
        count(keys.len())?,
        count(keys.len())?
            .checked_mul(count(size_of::<Window>())?)
            .ok_or(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::PerRecord,
            ))?,
    )?;
    Ok(keys.into_iter().collect())
}

fn insert_window(
    keys: &mut BTreeSet<Window>,
    window: Window,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    if !keys.contains(&window) {
        budget.reserve(1, 1, count(size_of::<Window>() + 3 * size_of::<usize>())?)?;
        keys.insert(window);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn selected_member(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    shift: &crate::temporal::ResolvedShift,
    scope: &Scope,
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    window: Window,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let metadata = input.metadata(shift)?;
    if !predicates::shift_matches(shift, &metadata, scope, budget)? {
        return Ok(false);
    }
    match membership {
        WindowMembership::ReportingDate => {
            let mut reporting = ReportingWindows::new();
            Ok(reporting
                .resolve(document, input, calendar_id, shift.reporting_date, budget)?
                .is_some_and(|p| p.start == window.start && p.end == window.end))
        }
        WindowMembership::StartInstant => Ok(shift.interval.starts_at.instant >= window.start
            && shift.interval.starts_at.instant < window.end),
        WindowMembership::Intersection => Ok(shift.interval.starts_at.instant < window.end
            && window.start < shift.interval.ends_at.instant),
    }
}

#[allow(clippy::too_many_arguments)]
fn association_members(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    selected: &Selected<'_>,
    person_id: PersonId,
    scope: &Scope,
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    window: Window,
    bucket_id: Option<WorkloadBucketId>,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentPair>, AssignmentRuleError> {
    let mut members = Vec::new();
    if let Some(associations) = selected.associations() {
        for pair in associations {
            budget.step()?;
            if pair.person_id != person_id {
                continue;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            if !selected_member(
                document,
                input,
                shift,
                scope,
                calendar_id,
                membership,
                window,
                budget,
            )? {
                continue;
            }
            if let Some(bucket_id) = bucket_id
                && !input
                    .metadata(shift)?
                    .assignment_type
                    .workload_bucket_ids
                    .contains(&bucket_id)
            {
                continue;
            }
            budget.reserve(0, 1, 48)?;
            members.push(*pair);
        }
    }
    Ok(members)
}

#[allow(clippy::too_many_arguments)]
fn count_assignments(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    chosen: &[&crate::temporal::ResolvedShift],
    scope: &Scope,
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    window: Window,
    budget: &mut OperationBudget<'_>,
) -> Result<(u64, Option<crate::ids::ShiftId>), AssignmentRuleError> {
    let mut result = 0u64;
    let mut first_shift = None;
    for shift in chosen {
        budget.step()?;
        if selected_member(
            document,
            input,
            shift,
            scope,
            calendar_id,
            membership,
            window,
            budget,
        )? {
            first_shift.get_or_insert(shift.id);
            result = result.checked_add(1).ok_or_else(arithmetic)?;
        }
    }
    Ok((result, first_shift))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn minute_total(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    chosen: &[&crate::temporal::ResolvedShift],
    scope: &Scope,
    calendar_id: WorkCalendarId,
    membership: WindowMembership,
    window: Window,
    bucket_id: WorkloadBucketId,
    measurement: WorkloadMeasurement,
    contribution: OverlappingContribution,
    budget: &mut OperationBudget<'_>,
) -> Result<(i128, Option<crate::ids::ShiftId>), AssignmentRuleError> {
    let mut members = Vec::new();
    for shift in chosen {
        budget.step()?;
        if !selected_member(
            document,
            input,
            shift,
            scope,
            calendar_id,
            membership,
            window,
            budget,
        )? {
            continue;
        }
        let metadata = input.metadata(shift)?;
        if !metadata
            .assignment_type
            .workload_bucket_ids
            .contains(&bucket_id)
        {
            continue;
        }
        let full = predicates::interval(shift);
        let interval = if membership == WindowMembership::Intersection {
            full.intersection(window.interval()).ok_or_else(invalid)?
        } else {
            full
        };
        let elapsed = duration_nanos(
            interval
                .start
                .as_timestamp()
                .duration_until(interval.end.as_timestamp()),
        )?;
        let scheduled = if membership == WindowMembership::Intersection {
            0
        } else {
            duration_nanos(shift.interval.scheduled_duration())?
        };
        budget.reserve(0, 1, 96)?;
        members.push(Contribution {
            shift_id: shift.id,
            interval,
            elapsed_nanos: elapsed,
            scheduled_nanos: scheduled,
        });
    }
    if contribution == OverlappingContribution::Sum {
        let mut total = 0i128;
        for member in &members {
            total = total
                .checked_add(if measurement == WorkloadMeasurement::ElapsedMinutes {
                    member.elapsed_nanos
                } else {
                    member.scheduled_nanos
                })
                .ok_or_else(arithmetic)?;
        }
        return Ok((total, members.first().map(|member| member.shift_id)));
    }
    members.sort_by_key(|member| (member.interval.start, member.interval.end, member.shift_id));
    let mut total = 0i128;
    let mut current: Option<InstantInterval> = None;
    let mut first = None;
    for member in members {
        if first.is_none() {
            first = Some(member.shift_id);
        }
        match current {
            None => current = Some(member.interval),
            Some(interval) if member.interval.start <= interval.end => {
                current = Some(InstantInterval {
                    start: interval.start,
                    end: interval.end.max(member.interval.end),
                });
            }
            Some(interval) => {
                total = total
                    .checked_add(duration_nanos(
                        interval
                            .start
                            .as_timestamp()
                            .duration_until(interval.end.as_timestamp()),
                    )?)
                    .ok_or_else(arithmetic)?;
                current = Some(member.interval);
            }
        }
    }
    if let Some(interval) = current {
        total = total
            .checked_add(duration_nanos(
                interval
                    .start
                    .as_timestamp()
                    .duration_until(interval.end.as_timestamp()),
            )?)
            .ok_or_else(arithmetic)?;
    }
    Ok((total, first))
}

fn duration_nanos(duration: SignedDuration) -> Result<i128, AssignmentRuleError> {
    i128::from(duration.as_secs())
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|seconds| seconds.checked_add(i128::from(duration.subsec_nanos())))
        .ok_or_else(arithmetic)
}
fn signed_duration(nanos: i128) -> Result<SignedDuration, AssignmentRuleError> {
    let seconds = i64::try_from(nanos / NANOS_PER_SECOND)
        .map_err(|_| AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::PerRecord))?;
    let remainder = i32::try_from(nanos % NANOS_PER_SECOND).map_err(|_| arithmetic())?;
    Ok(SignedDuration::new(seconds, remainder))
}
fn floor_seconds(nanos: i128) -> i128 {
    nanos.div_euclid(NANOS_PER_SECOND)
}
fn ceil_seconds(nanos: i128) -> i128 {
    let (q, r) = (
        nanos.div_euclid(NANOS_PER_SECOND),
        nanos.rem_euclid(NANOS_PER_SECOND),
    );
    if r == 0 { q } else { q + 1 }
}
