use super::{invalid, Witness};
use super::super::{
    AssignmentRuleError, InstantInterval,
    budget::{OperationBudget, count},
    input::{AssignmentInput, ShiftMetadata},
    intervals::availability_intervals,
};
use crate::{
    ids::QualificationId,
    model::{Availability, AvailabilityKind, Person, PersonSelection, QualificationExpression,
        QualificationMatch, Scope, ShiftScope},
    temporal::{ResolvedShift, weekday},
};
use eutheto_types::ScenarioSettings;

pub(super) fn interval(shift: &ResolvedShift) -> InstantInterval {
    InstantInterval { start: shift.interval.starts_at.instant, end: shift.interval.ends_at.instant }
}

fn contains<T: PartialEq>(values: &[T], value: &T, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    budget.steps(count(values.len())?)?;
    Ok(values.contains(value))
}

fn restriction<T: PartialEq>(values: Option<&[T]>, value: Option<&T>, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    match values {
        None => Ok(true),
        Some(values) => match value {
            Some(value) => contains(values, value, budget),
            None => Ok(false),
        },
    }
}

pub(super) fn person_matches(person: &Person, scope: &Scope, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    let selected = match &scope.people {
        PersonSelection::All {} => true,
        PersonSelection::Selected { person_ids } => contains(person_ids, &person.id, budget)?,
        PersonSelection::Filter { all_tags, any_tags } => {
            let mut all = true;
            for tag in all_tags { all &= contains(&person.tags, tag, budget)?; }
            let mut any = any_tags.is_empty();
            for tag in any_tags { any |= contains(&person.tags, tag, budget)?; }
            all && any
        }
    };
    if !selected { return Ok(false); }
    if let Some(teams) = &scope.team_ids {
        let mut matched = false;
        for team in teams { matched |= contains(&person.team_ids, team, budget)?; }
        if !matched { return Ok(false); }
    }
    Ok(true)
}

pub(super) fn shift_matches(shift: &ResolvedShift, metadata: &ShiftMetadata<'_>, scope: &Scope, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    Ok(restriction(scope.assignment_type_ids.as_deref(), Some(&metadata.assignment_type.id), budget)?
        && restriction(scope.categories.as_deref(), Some(&metadata.assignment_type.category), budget)?
        && restriction(scope.weekdays.as_deref(), Some(&weekday(shift.reporting_date)), budget)?
        && restriction(scope.location_ids.as_deref(), metadata.location_id.as_ref(), budget)?)
}

pub(super) fn requirement_matches(scope: &ShiftScope, shift: &ResolvedShift, metadata: &ShiftMetadata<'_>, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    match scope {
        ShiftScope::All {} => Ok(true),
        ShiftScope::Selected { shift_ids } => contains(shift_ids, &shift.id, budget),
        ShiftScope::Filter { assignment_type_ids, start_date_range, location_ids } => {
            let date = shift.interval.starts_at.local.as_datetime().date();
            Ok(start_date_range.is_none_or(|range| range.start_date <= date && date < range.end_date_exclusive)
                && restriction(assignment_type_ids.as_deref(), Some(&metadata.assignment_type.id), budget)?
                && restriction(location_ids.as_deref(), metadata.location_id.as_ref(), budget)?)
        }
    }
}

pub(super) fn type_allowed(person: &Person, metadata: &ShiftMetadata<'_>, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    contains(&person.eligible_assignment_type_ids, &metadata.assignment_type.id, budget)
}

// Advance a coverage frontier using only grants of this qualification. A grant beginning at
// the frontier is an adjoining renewal; even a one-nanosecond gap prevents advancement.
fn qualification(person: &Person, id: QualificationId, query: InstantInterval, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    let mut cursor = query.start;
    loop {
        let mut next = cursor;
        for grant in &person.qualification_grants {
            budget.step()?;
            if grant.qualification_id == id && grant.effective_from.is_none_or(|start| start <= cursor) {
                next = next.max(grant.expires_at.unwrap_or(query.end).min(query.end));
            }
        }
        if next >= query.end { return Ok(true); }
        if next == cursor { return Ok(false); }
        cursor = next;
    }
}

pub(super) fn qualification_match(person: &Person, expression: &QualificationMatch, query: InstantInterval, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    let mut all = true;
    for id in &expression.all_qualification_ids {
        budget.step()?;
        all &= qualification(person, *id, query, budget)?;
    }
    let mut any = expression.any_qualification_ids.is_empty();
    for id in &expression.any_qualification_ids {
        budget.step()?;
        any |= qualification(person, *id, query, budget)?;
    }
    Ok(all && any)
}

pub(super) fn qualified(person: &Person, expression: &QualificationExpression, query: InstantInterval, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    match expression {
        QualificationExpression::Unconstrained {} => Ok(true),
        QualificationExpression::Matches(expression) => qualification_match(person, expression, query, budget),
    }
}

pub(super) fn availability_matches(record: &Availability, metadata: &ShiftMetadata<'_>, budget: &mut OperationBudget<'_>) -> Result<bool, AssignmentRuleError> {
    budget.step()?;
    Ok(restriction(record.assignment_type_ids.as_deref(), Some(&metadata.assignment_type.id), budget)?
        && restriction(record.location_ids.as_deref(), metadata.location_id.as_ref(), budget)?)
}

pub(super) fn availability_failure(record: &Availability, shift: &ResolvedShift, settings: &ScenarioSettings, budget: &mut OperationBudget<'_>) -> Result<Option<InstantInterval>, AssignmentRuleError> {
    let expanded = availability_intervals(record, interval(shift), settings, budget)?;
    let Some(query) = expanded.query else { return Ok(None); };
    match record.availability_kind {
        AvailabilityKind::AvailableOnly => {
            let mut cursor = query.start;
            for window in expanded.intervals {
                budget.step()?;
                if window.start > cursor {
                    return Ok(Some(InstantInterval { start: cursor, end: window.start }));
                }
                cursor = cursor.max(window.end);
                if cursor >= query.end { return Ok(None); }
            }
            Ok(Some(InstantInterval { start: cursor, end: query.end }))
        }
        AvailabilityKind::Unavailable | AvailabilityKind::ApprovedTimeOff => {
            budget.step()?;
            Ok(expanded.intervals.first().copied())
        }
        AvailabilityKind::RequestedTimeOff => Err(invalid()),
    }
}

pub(super) fn pair_witness(input: &AssignmentInput, pair: crate::model::AssignmentPair, reason: &'static str) -> Result<Witness, AssignmentRuleError> {
    let metadata = input.metadata(input.shift(pair.shift_id).ok_or_else(invalid)?)?;
    Ok(Witness::pair(pair, metadata.assignment_type.id.as_entity_id(), "assignment_type", reason))
}
