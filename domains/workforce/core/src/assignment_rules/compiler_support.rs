use super::super::{
    AssignmentConstructionIssue, AssignmentRuleError, InstantInterval,
    budget::{OperationBudget, count},
    input::{AssignmentInput, ShiftMetadata},
};
use crate::{
    ids::QualificationId,
    model::{ActiveRange, Availability, CategoryPair, Coverage, Person, PersonSelection,
        QualificationExpression, Scope, ShiftScope},
    temporal::{ResolvedShift, weekday},
};
use eutheto_types::ScenarioSettings;
use serde::Serialize;

pub(super) fn invalid() -> AssignmentRuleError {
    AssignmentRuleError::InvalidConstruction(AssignmentConstructionIssue::InvalidRecord)
}

pub(super) fn interval(shift: &ResolvedShift) -> InstantInterval {
    InstantInterval {
        start: shift.interval.starts_at.instant,
        end: shift.interval.ends_at.instant,
    }
}

fn contains<T: PartialEq>(
    values: &[T],
    value: &T,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.steps(count(values.len())?)?;
    Ok(values.contains(value))
}

pub(super) fn person_scope(
    scope: &Scope,
    person: &Person,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    let selected = match &scope.people {
        PersonSelection::All {} => true,
        PersonSelection::Selected { person_ids } => contains(person_ids, &person.id, budget)?,
        PersonSelection::Filter { all_tags, any_tags } => {
            let mut all = true;
            let mut any = any_tags.is_empty();
            for tag in all_tags {
                budget.step()?;
                all &= contains(&person.tags, tag, budget)?;
            }
            for tag in any_tags {
                budget.step()?;
                any |= contains(&person.tags, tag, budget)?;
            }
            all && any
        }
    };
    let mut team = scope.team_ids.is_none();
    if let Some(ids) = &scope.team_ids {
        for id in ids {
            budget.step()?;
            team |= contains(&person.team_ids, id, budget)?;
        }
    }
    Ok(selected && team)
}

pub(super) fn shift_scope(
    scope: &Scope,
    shift: &ResolvedShift,
    metadata: &ShiftMetadata<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    let mut result = true;
    if let Some(ids) = &scope.assignment_type_ids {
        result &= contains(ids, &metadata.assignment_type.id, budget)?;
    }
    if let Some(categories) = &scope.categories {
        result &= contains(categories, &metadata.assignment_type.category, budget)?;
    }
    if let Some(days) = &scope.weekdays {
        result &= contains(days, &weekday(shift.reporting_date), budget)?;
    }
    if let Some(ids) = &scope.location_ids {
        budget.steps(count(ids.len())?)?;
        result &= metadata.location_id.is_some_and(|id| ids.contains(&id));
    }
    Ok(result)
}

pub(super) fn requirement_scope(
    scope: &ShiftScope,
    shift: &ResolvedShift,
    metadata: &ShiftMetadata<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    match scope {
        ShiftScope::All {} => Ok(true),
        ShiftScope::Selected { shift_ids } => contains(shift_ids, &shift.id, budget),
        ShiftScope::Filter {
            assignment_type_ids,
            start_date_range,
            location_ids,
        } => {
            let mut result = true;
            if let Some(ids) = assignment_type_ids {
                result &= contains(ids, &metadata.assignment_type.id, budget)?;
            }
            if let Some(range) = start_date_range {
                let date = shift.interval.starts_at.local.as_datetime().date();
                result &= range.start_date <= date && date < range.end_date_exclusive;
            }
            if let Some(ids) = location_ids {
                budget.steps(count(ids.len())?)?;
                result &= metadata.location_id.is_some_and(|id| ids.contains(&id));
            }
            Ok(result)
        }
    }
}

pub(super) fn availability_scope(
    record: &Availability,
    metadata: &ShiftMetadata<'_>,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    let mut result = true;
    if let Some(ids) = &record.assignment_type_ids {
        result &= contains(ids, &metadata.assignment_type.id, budget)?;
    }
    if let Some(ids) = &record.location_ids {
        budget.steps(count(ids.len())?)?;
        result &= metadata.location_id.is_some_and(|id| ids.contains(&id));
    }
    Ok(result)
}

// Grow a same-qualification covered prefix. No sorted/copying grant buffer is necessary;
// every scan either reaches the end, makes strict progress, or proves the earliest gap.
fn qualification(
    person: &Person,
    id: QualificationId,
    query: InstantInterval,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let mut cursor = query.start;
    loop {
        let mut end = cursor;
        for grant in &person.qualification_grants {
            budget.step()?;
            if grant.qualification_id == id
                && grant.effective_from.is_none_or(|start| start <= cursor)
            {
                end = end.max(grant.expires_at.unwrap_or(query.end).min(query.end));
            }
        }
        if end >= query.end {
            return Ok(true);
        }
        if end == cursor {
            return Ok(false);
        }
        cursor = end;
    }
}

pub(super) fn qualification_match(
    person: &Person,
    all: &[QualificationId],
    any: &[QualificationId],
    query: InstantInterval,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    let mut all_match = true;
    let mut any_match = any.is_empty();
    for id in all {
        budget.step()?;
        all_match &= qualification(person, *id, query, budget)?;
    }
    for id in any {
        budget.step()?;
        any_match |= qualification(person, *id, query, budget)?;
    }
    Ok(all_match && any_match)
}

pub(super) fn expression(
    person: &Person,
    expression: &QualificationExpression,
    query: InstantInterval,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    match expression {
        QualificationExpression::Unconstrained {} => Ok(true),
        QualificationExpression::Matches(value) => qualification_match(
            person,
            &value.all_qualification_ids,
            &value.any_qualification_ids,
            query,
            budget,
        ),
    }
}

pub(super) fn outside(query: InstantInterval, allowed: InstantInterval) -> Option<InstantInterval> {
    if query.start < allowed.start {
        Some(InstantInterval {
            start: query.start,
            end: query.end.min(allowed.start),
        })
    } else if query.end > allowed.end {
        Some(InstantInterval {
            start: query.start.max(allowed.end),
            end: query.end,
        })
    } else {
        None
    }
}

pub(super) fn uncovered(
    query: InstantInterval,
    windows: &[InstantInterval],
    budget: &mut OperationBudget<'_>,
) -> Result<Option<InstantInterval>, AssignmentRuleError> {
    let mut cursor = query.start;
    for window in windows {
        budget.step()?;
        if window.start > cursor {
            return Ok(Some(InstantInterval {
                start: cursor,
                end: window.start.min(query.end),
            }));
        }
        cursor = cursor.max(window.end);
        if cursor >= query.end {
            return Ok(None);
        }
    }
    Ok((cursor < query.end).then_some(InstantInterval {
        start: cursor,
        end: query.end,
    }))
}

pub(super) fn active_interval(
    person: &Person,
    settings: &ScenarioSettings,
) -> Result<Option<InstantInterval>, AssignmentRuleError> {
    match person.active_range {
        ActiveRange::Always {} => Ok(None),
        ActiveRange::DateRange(range) => super::super::intervals::date_range(
            range,
            settings,
            eutheto_types::EntityId::from_uuid(person.id.as_uuid()),
        )
        .map(Some),
    }
}

pub(super) fn incompatible(
    input: &AssignmentInput,
    first: &ResolvedShift,
    second: &ResolvedShift,
    pairs: &[CategoryPair],
    budget: &mut OperationBudget<'_>,
) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    if interval(first).intersection(interval(second)).is_none() {
        return Ok(false);
    }
    let a = &input.metadata(first)?.assignment_type.category;
    let b = &input.metadata(second)?.assignment_type.category;
    for pair in pairs {
        budget.step()?;
        if (&pair.first_category == a && &pair.second_category == b)
            || (&pair.first_category == b && &pair.second_category == a)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(in crate::assignment_rules) struct MinimumKey {
    pub all: Vec<QualificationId>,
    pub any: Vec<QualificationId>,
    pub minimum: u16,
    /// Canonical JSON is interned once per unique owner-local definition. Identity keys
    /// borrow this string rather than re-encoding the qualification sets per occurrence.
    #[serde(skip)]
    pub identity: String,
}

pub(super) fn canonical_minima(
    coverage: &Coverage,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<MinimumKey>, AssignmentRuleError> {
    let values = match coverage {
        Coverage::Exact {
            qualification_minimums,
            ..
        }
        | Coverage::AtLeast {
            qualification_minimums,
            ..
        } => qualification_minimums,
    };
    let mut result = Vec::new();
    for value in values {
        budget.step()?;
        let items = count(
            value.qualifications.all_qualification_ids.len()
                + value.qualifications.any_qualification_ids.len(),
        )?;
        let bytes = budget.measure(value)?;
        budget.reserve(1, items, bytes)?;
        let mut key = MinimumKey {
            all: value.qualifications.all_qualification_ids.clone(),
            any: value.qualifications.any_qualification_ids.clone(),
            minimum: value.minimum,
            identity: String::new(),
        };
        budget.sort_work(key.all.len())?;
        key.all.sort_unstable();
        budget.steps(count(key.all.len())?)?;
        key.all.dedup();
        budget.sort_work(key.any.len())?;
        key.any.sort_unstable();
        budget.steps(count(key.any.len())?)?;
        key.any.dedup();
        result.push(key);
    }
    budget.sort_work(result.len())?;
    result.sort_unstable();
    budget.steps(count(result.len())?)?;
    result.dedup();
    for key in &mut result {
        budget.step()?;
        let bytes = budget.measure(key)?;
        budget.reserve(0, 1, bytes)?;
        key.identity = serde_json::to_string(key).map_err(|_| invalid())?;
    }
    Ok(result)
}

pub(super) fn bounds(coverage: &Coverage) -> (u64, Option<u64>) {
    match coverage {
        Coverage::Exact { count, .. } => (u64::from(*count), Some(u64::from(*count))),
        Coverage::AtLeast {
            minimum,
            maximum_count,
            ..
        } => (u64::from(*minimum), maximum_count.map(u64::from)),
    }
}
