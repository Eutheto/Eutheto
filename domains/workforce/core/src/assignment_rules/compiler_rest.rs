use super::{
    AssignmentInput, AssignmentLock, AssignmentPair, AssignmentRuleError, DomainValidationReport,
    OperationBudget, Person, Plan, PlannedConstraint, Predicate, ResolvedShift, ResourceRef,
    RuleId, Scope, ValidationIssue, ValidationSeverity, WorkforceRule, append, invalid,
    person_scope, shift_scope,
};
use jiff::SignedDuration;

pub(super) struct RestRule<'a> {
    pub id: RuleId,
    pub scope: &'a Scope,
    pub after_scope: &'a Scope,
    pub before_scope: &'a Scope,
    pub minimum_minutes: u32,
}

impl RestRule<'_> {
    fn matches_person(
        &self,
        person: &Person,
        budget: &mut OperationBudget<'_>,
    ) -> Result<bool, AssignmentRuleError> {
        Ok(person_scope(self.scope, person, budget)?
            && person_scope(self.after_scope, person, budget)?
            && person_scope(self.before_scope, person, budget)?)
    }

    pub(super) fn plan(
        &self,
        input: &AssignmentInput,
        candidates: &[AssignmentPair],
        plan: &mut Plan,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        // Candidates are already person/shift sorted. A forward cursor avoids rescanning
        // other people's assignments and does not allocate another person index.
        let mut position = 0;
        for person_id in &input.people {
            budget.step()?;
            let person = input.person(*person_id).ok_or_else(invalid)?;
            let matches = self.matches_person(person, budget)?;
            let mut roles = Roles::default();
            while let Some(pair) = candidates.get(position) {
                budget.step()?;
                if pair.person_id != *person_id {
                    break;
                }
                if matches {
                    roles.push(
                        position,
                        input.shift(pair.shift_id).ok_or_else(invalid)?,
                        input,
                        self,
                        budget,
                    )?;
                }
                position += 1;
            }
            roles.conflicts(self.minimum_minutes, budget, |source, target, _, budget| {
                budget.reserve(0, 2, 16)?;
                append(
                    plan,
                    PlannedConstraint {
                        rule: self.id,
                        predicate: Predicate::MinimumRest {
                            person: *person_id,
                            source: source.1.id,
                            target: target.1.id,
                            minimum_minutes: self.minimum_minutes,
                        },
                        population: vec![source.0, target.0],
                        impossible: false,
                    },
                    budget,
                )
            })?;
        }
        Ok(())
    }
}

// These are compact borrowed role indexes, never a retained Cartesian conflict matrix.
#[derive(Default)]
struct Roles<'a> {
    sources: Vec<(usize, &'a ResolvedShift)>,
    targets: Vec<(usize, &'a ResolvedShift)>,
}

impl<'a> Roles<'a> {
    fn push(
        &mut self,
        index: usize,
        shift: &'a ResolvedShift,
        input: &AssignmentInput,
        rule: &RestRule<'_>,
        budget: &mut OperationBudget<'_>,
    ) -> Result<(), AssignmentRuleError> {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if !shift_scope(rule.scope, shift, &metadata, budget)? {
            return Ok(());
        }
        for (scope, population) in [
            (rule.after_scope, &mut self.sources),
            (rule.before_scope, &mut self.targets),
        ] {
            budget.step()?;
            if shift_scope(scope, shift, &metadata, budget)? {
                budget.reserve(0, 2, 16)?;
                population.push((index, shift));
            }
        }
        Ok(())
    }

    fn conflicts(
        &mut self,
        minimum_minutes: u32,
        budget: &mut OperationBudget<'_>,
        mut emit: impl FnMut(
            (usize, &'a ResolvedShift),
            (usize, &'a ResolvedShift),
            SignedDuration,
            &mut OperationBudget<'_>,
        ) -> Result<(), AssignmentRuleError>,
    ) -> Result<(), AssignmentRuleError> {
        for population in [&mut self.sources, &mut self.targets] {
            budget.sort_work(population.len())?;
            population.sort_unstable_by_key(|(index, shift)| {
                (shift.interval.starts_at.instant, shift.id, *index)
            });
        }
        // u32 minutes times sixty fits i64. Compare signed durations, not a potentially
        // overflowing end + threshold timestamp or rounded wall-clock minutes.
        let required = SignedDuration::from_secs(i64::from(minimum_minutes) * 60);
        let mut first_target = 0;
        for source in &self.sources {
            budget.step()?;
            while let Some(target) = self.targets.get(first_target) {
                budget.step()?;
                if target.1.interval.starts_at.instant >= source.1.interval.starts_at.instant {
                    break;
                }
                first_target += 1;
            }
            for target in &self.targets[first_target..] {
                budget.step()?;
                if source.1.id == target.1.id {
                    continue;
                }
                let elapsed = source
                    .1
                    .interval
                    .ends_at
                    .instant
                    .as_timestamp()
                    .duration_until(target.1.interval.starts_at.instant.as_timestamp());
                if elapsed >= required {
                    break;
                }
                emit(*source, *target, elapsed, budget)?;
            }
        }
        Ok(())
    }
}

pub(super) fn locked_rest_findings(
    input: &AssignmentInput,
    locks: &[&AssignmentLock],
    report: &mut DomainValidationReport,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    for rule in input.domain.rules.values() {
        budget.step()?;
        let WorkforceRule::MinimumRest(value) = rule else {
            continue;
        };
        if !value.active {
            continue;
        }
        let rule = RestRule {
            id: value.id,
            scope: &value.scope,
            after_scope: &value.after_scope,
            before_scope: &value.before_scope,
            minimum_minutes: value.minimum_minutes,
        };
        let mut position = 0;
        for person_id in &input.people {
            budget.step()?;
            let person = input.person(*person_id).ok_or_else(invalid)?;
            let matches = rule.matches_person(person, budget)?;
            let mut roles = Roles::default();
            while let Some(lock) = locks.get(position) {
                budget.step()?;
                if lock.person_id != *person_id {
                    break;
                }
                // Original resolved Hard locks remain visible even when unary pruning rejected
                // the pair. Unresolved locks already retain their separate readiness error.
                if matches && let Some(shift) = input.shift(lock.shift_id) {
                    roles.push(position, shift, input, &rule, budget)?;
                }
                position += 1;
            }
            roles.conflicts(rule.minimum_minutes, budget, |source, target, elapsed, budget| {
                let source = locks[source.0];
                let target = locks[target.0];
                budget.step()?;
                budget.reserve(1, 5, 1024)?;
                report.issues.push(ValidationIssue {
                    code: "official.workforce.hard_lock_minimum_rest".to_owned(),
                    severity: ValidationSeverity::Error,
                    message: format!(
                        "Source Hard lock {} and target Hard lock {} conflict under active MinimumRest rule {}: elapsed rest {} seconds and {} subsecond nanoseconds is below {} required minutes. This is not a solver feasibility result.",
                        source.id, target.id, rule.id, elapsed.as_secs(), elapsed.subsec_nanos(), rule.minimum_minutes,
                    ),
                    field_path: Some(format!("domain.lockedAssignments.{}", source.id)),
                    resource: Some(ResourceRef::Assignment(source.id)),
                });
                Ok(())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fixture, id};
    use eutheto_planning_ir::PlanningIrLimitsV1;
    use eutheto_types::CancellationToken;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn cancellation_inside_rest_conflict_window_uses_the_real_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = fixture()?;
        document.domain.locked_assignments.clear();
        document.domain.entities.remove(&id(6).parse()?);
        let shift = document
            .domain
            .entities
            .get(&id(8).parse()?)
            .ok_or("shift")?
            .clone();
        for index in 1_000..1_032 {
            let mut record = shift.clone();
            record["id"] = json!(id(index));
            document.domain.entities.insert(id(index).parse()?, record);
        }
        let token = CancellationToken::new();
        let mut budget = OperationBudget::analysis(Some(&token), PlanningIrLimitsV1::DEFAULT);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let scope: Scope = serde_json::from_value(json!({"people":{"kind":"all"}}))?;
        let rule = RestRule {
            id: id(24).parse()?,
            scope: &scope,
            after_scope: &scope,
            before_scope: &scope,
            minimum_minutes: 600,
        };
        let mut roles = Roles::default();
        for (index, shift) in input.shifts.iter().enumerate() {
            roles.push(index, shift, &input, &rule, &mut budget)?;
        }
        let mut plan = Plan {
            definitions: Vec::new(), constraints: Vec::new(), parents: BTreeMap::new(),
        };
        let mut entered = false;
        let result = roles.conflicts(600, &mut budget, |source, target, _, budget| {
            // Arm only once a real conflict has been found, after validated setup,
            // role filtering, sorting and entry to the actual dense semantic window.
            if !entered {
                entered = true;
                budget.cancel_after_steps(128)?;
            }
            budget.reserve(0, 2, 16)?;
            append(
                &mut plan,
                PlannedConstraint {
                    rule: rule.id,
                    predicate: Predicate::MinimumRest {
                        person: id(1).parse().map_err(|_| invalid())?,
                        source: source.1.id,
                        target: target.1.id,
                        minimum_minutes: 600,
                    },
                    population: vec![source.0, target.0],
                    impossible: false,
                },
                budget,
            )
        });
        assert_eq!(result, Err(AssignmentRuleError::Cancelled));
        assert!(entered && token.is_cancelled());
        Ok(())
    }
}
