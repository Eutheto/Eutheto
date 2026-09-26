use super::super::reporting::{ReportingPeriod, ReportingWindows};
use super::{
    AssignmentInput, AssignmentPair, AssignmentRuleError, DomainValidationReport, OperationBudget,
    Plan, PlannedConstraint, Predicate, ResourceRef, ValidationIssue, WorkforceRule, append,
    invalid, person_scope, shift_scope,
};
use crate::{ids::ShiftId, model::AssignmentLock as ModelAssignmentLock};
use eutheto_types::{PersonId, ScenarioDocument, ValidationSeverity};
use std::collections::{BTreeMap, BTreeSet};

/// Plan one bounded `MaximumAssignmentCount` cardinality for every matching
/// person/reporting period with at least one feasible candidate.
pub(super) fn plan(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    candidates: &[AssignmentPair],
    rule: &WorkforceRule,
    plan: &mut Plan,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumAssignmentCount {
        id,
        scope,
        calendar_id,
        maximum,
        ..
    } = rule
    else {
        return Err(invalid());
    };
    let mut periods = ReportingWindows::new();
    for person_id in &input.people {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !person_scope(scope, person, budget)? {
            continue;
        }
        let start = candidates.partition_point(|pair| pair.person_id < *person_id);
        let mut grouped = BTreeMap::<ReportingPeriod, Vec<usize>>::new();
        for (offset, pair) in candidates[start..].iter().enumerate() {
            budget.step()?;
            if pair.person_id != *person_id {
                break;
            }
            let shift = input.shift(pair.shift_id).ok_or_else(invalid)?;
            let metadata = input.metadata(shift)?;
            if !shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            let Some(period) =
                periods.resolve(document, input, *calendar_id, shift.reporting_date, budget)?
            else {
                // A custom calendar date without an owner contributes to no period.
                continue;
            };
            budget.reserve(0, 1, 8)?;
            grouped.entry(period).or_default().push(start + offset);
        }
        for (period, population) in grouped {
            budget.step()?;
            if population.is_empty() {
                continue;
            }
            append(
                plan,
                PlannedConstraint {
                    rule: *id,
                    predicate: Predicate::MaximumAssignmentCount {
                        person: *person_id,
                        calendar_id: *calendar_id,
                        period,
                        maximum: *maximum,
                    },
                    population,
                    impossible: false,
                },
                budget,
            )?;
        }
    }
    Ok(())
}

/// Diagnose only an observed Hard-lock excess. This is readiness evidence, not lock
/// enforcement or a solver infeasibility claim.
pub(super) fn locked_findings(
    document: &ScenarioDocument,
    input: &AssignmentInput,
    locks: &[&ModelAssignmentLock],
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut periods = ReportingWindows::new();
    for rule in input.domain.rules.values() {
        budget.step()?;
        let WorkforceRule::MaximumAssignmentCount {
            id,
            active: true,
            scope,
            calendar_id,
            maximum,
            ..
        } = rule
        else {
            continue;
        };
        let mut grouped = BTreeMap::<(PersonId, ReportingPeriod), BTreeSet<ShiftId>>::new();
        for lock in locks {
            budget.step()?;
            let person = input.person(lock.person_id).ok_or_else(invalid)?;
            if !person_scope(scope, person, budget)? {
                continue;
            }
            let Some(shift) = input.shift(lock.shift_id) else {
                continue;
            };
            let metadata = input.metadata(shift)?;
            if !shift_scope(scope, shift, &metadata, budget)? {
                continue;
            }
            let Some(period) =
                periods.resolve(document, input, *calendar_id, shift.reporting_date, budget)?
            else {
                continue;
            };
            budget.reserve(0, 1, 16)?;
            grouped
                .entry((lock.person_id, period))
                .or_default()
                .insert(lock.shift_id);
        }
        for ((person_id, period), shifts) in grouped {
            budget.step()?;
            let actual = u64::try_from(shifts.len()).map_err(|_| invalid())?;
            if actual <= u64::from(*maximum) {
                continue;
            }
            budget.reserve(1, 7, 1024)?;
            report.issues.push(ValidationIssue {
                code: "official.workforce.hard_locked_maximum_assignment_count".to_owned(),
                severity: ValidationSeverity::Error,
                message: format!(
                    "Hard locks for person {person_id} contain {actual} distinct assignments in calendar {} period {}–{}, exceeding MaximumAssignmentCount rule {id} maximum {}. This is not a solver feasibility result.",
                    period.calendar_id,
                    period.start,
                    period.end,
                    maximum,
                ),
                field_path: Some(format!("domain.rules.{id}.maximum")),
                resource: Some(ResourceRef::Rule(*id)),
            });
        }
    }
    Ok(())
}
