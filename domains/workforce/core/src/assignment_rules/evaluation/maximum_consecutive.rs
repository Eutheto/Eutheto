use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add},
    input::AssignmentInput,
};
use super::{Selected, Summary, Witness, arithmetic, invalid, predicates};
use crate::{
    model::{AssignmentPair, ConsecutiveMode, Scope, WorkforceRule},
    temporal::ResolvedShift,
};
use eutheto_types::{EntityId, PersonId, RuleId};
use jiff::{SignedDuration, Span, civil::Date};
use std::collections::BTreeMap;

/// Evaluates `MaximumConsecutive` directly from original selected pairs.
///
/// Candidate pruning, compiler populations and IR constraints are deliberately not consulted here.
pub(super) fn evaluate(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    rule: &WorkforceRule,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let WorkforceRule::MaximumConsecutive {
        id,
        scope,
        mode,
        maximum,
        ..
    } = rule
    else {
        return Err(invalid());
    };

    for (person_id, shifts) in &selected.by_person {
        budget.step()?;
        let person = input.person(*person_id).ok_or_else(invalid)?;
        if !predicates::person_matches(person, scope, budget)? {
            continue;
        }
        let matching = matching_shifts(input, shifts, scope, budget)?;
        match mode {
            ConsecutiveMode::WorkedDays {} => {
                evaluate_worked_days(*person_id, *id, *maximum, &matching, summary, budget)?;
            }
            ConsecutiveMode::Assignments { break_minutes } => evaluate_assignments(
                *person_id,
                *id,
                *maximum,
                *break_minutes,
                &matching,
                summary,
                budget,
            )?,
        }
    }
    Ok(())
}

fn matching_shifts<'a>(
    input: &AssignmentInput,
    shifts: &[&'a ResolvedShift],
    scope: &Scope,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<&'a ResolvedShift>, AssignmentRuleError> {
    let mut matching = Vec::new();
    for shift in shifts {
        budget.step()?;
        let metadata = input.metadata(shift)?;
        if predicates::shift_matches(shift, &metadata, scope, budget)? {
            // Charge each retained scratch member before pushing it into the owned vector.
            budget.reserve(0, 1, 32)?;
            matching.push(*shift);
        }
    }
    budget.sort_work(matching.len())?;
    matching.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));
    Ok(matching)
}

fn evaluate_worked_days(
    person_id: PersonId,
    rule_id: RuleId,
    maximum: u32,
    matching: &[&ResolvedShift],
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    // A date is retained once even if several selected matching assignments report it. The map's
    // ordering is civil-date ordering, not encoded YYYYMMDD arithmetic.
    let mut days = BTreeMap::<Date, &ResolvedShift>::new();
    for shift in matching {
        budget.step()?;
        if let std::collections::btree_map::Entry::Vacant(entry) = days.entry(shift.reporting_date)
        {
            budget.reserve(0, 2, 32)?;
            entry.insert(*shift);
        }
    }

    let mut previous_date: Option<Date> = None;
    let mut run_start = None;
    let mut run_length = 0_u64;
    for (date, shift) in days {
        budget.step()?;
        let contiguous = match previous_date {
            None => false,
            Some(previous) => {
                previous
                    .checked_add(Span::new().days(1))
                    .map_err(|_| arithmetic())?
                    == date
            }
        };
        if contiguous {
            run_length = add(run_length, 1)?;
        } else {
            if let Some(first) = run_start {
                record_run(
                    person_id, rule_id, maximum, first, run_length, summary, budget,
                )?;
            }
            run_start = Some(shift);
            run_length = 1;
        }
        previous_date = Some(date);
    }
    if let Some(first) = run_start {
        record_run(
            person_id, rule_id, maximum, first, run_length, summary, budget,
        )?;
    }
    Ok(())
}

fn evaluate_assignments(
    person_id: PersonId,
    rule_id: RuleId,
    maximum: u32,
    break_minutes: u32,
    matching: &[&ResolvedShift],
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let maximum_break = SignedDuration::from_mins(i64::from(break_minutes));
    let mut previous = None;
    let mut run_start = None;
    let mut run_length = 0_u64;
    for &shift in matching {
        budget.step()?;
        let joins = previous.is_some_and(|prior: &ResolvedShift| {
            prior
                .interval
                .ends_at
                .instant
                .as_timestamp()
                .duration_until(shift.interval.starts_at.instant.as_timestamp())
                <= maximum_break
        });
        if joins {
            run_length = add(run_length, 1)?;
        } else {
            if let Some(first) = run_start {
                record_run(
                    person_id, rule_id, maximum, first, run_length, summary, budget,
                )?;
            }
            run_start = Some(shift);
            run_length = 1;
        }
        previous = Some(shift);
    }
    if let Some(first) = run_start {
        record_run(
            person_id, rule_id, maximum, first, run_length, summary, budget,
        )?;
    }
    Ok(())
}

fn record_run(
    person_id: PersonId,
    rule_id: RuleId,
    maximum: u32,
    first: &ResolvedShift,
    actual: u64,
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
    let mut witness = Witness::pair(
        AssignmentPair {
            person_id,
            shift_id: first.id,
        },
        EntityId::from_uuid(rule_id.as_uuid()),
        "rule",
        "maximum_consecutive",
    );
    witness.lower = Some(0);
    witness.upper = Some(u64::from(maximum));
    witness.actual = Some(actual);
    witness.interval = Some(predicates::interval(first));
    summary.predicate(actual > u64::from(maximum), &witness, budget)
}

#[cfg(test)]
mod tests {
    use crate::{assignment_rules::AssignmentRuleEvaluation, model::AssignmentPair, test_support};
    use eutheto_domain_ir::{RuleEvaluation, VerificationFactId, VerificationValue};
    use eutheto_types::ScenarioDocument;
    use serde_json::{Value, json};
    use std::error::Error;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn base() -> TestResult<Value> {
        let mut value = serde_json::to_value(test_support::fixture()?)?;
        value["domain"]["entities"]
            .as_object_mut()
            .ok_or("entities")?
            .remove(&test_support::id(6));
        value["domain"]["lockedAssignments"] = json!({});
        value["settings"]["timeZone"] = json!("UTC");
        value["settings"]["horizon"] =
            json!({"start":"2026-10-31T00:00:00Z","end":"2026-11-04T00:00:00Z"});
        times(
            &mut value["domain"]["entities"][test_support::id(8)],
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        Ok(value)
    }

    fn times(shift: &mut Value, start: &str, end: &str) {
        shift["startsAt"] = json!({
            "instant": format!("{start}Z"),
            "local": start,
            "offsetSeconds": 0
        });
        shift["endsAt"] = json!({
            "instant": format!("{end}Z"),
            "local": end,
            "offsetSeconds": 0
        });
    }

    fn zoned_times(
        shift: &mut Value,
        start_local: &str,
        start_instant: &str,
        start_offset: i32,
        end_local: &str,
        end_instant: &str,
        end_offset: i32,
    ) {
        shift["startsAt"] = json!({
            "instant": format!("{start_instant}Z"),
            "local": start_local,
            "offsetSeconds": start_offset
        });
        shift["endsAt"] = json!({
            "instant": format!("{end_instant}Z"),
            "local": end_local,
            "offsetSeconds": end_offset
        });
    }

    fn add_shift(value: &mut Value, index: u32, start: &str, end: &str) {
        let mut shift = value["domain"]["entities"][test_support::id(8)].clone();
        shift["id"] = json!(test_support::id(index));
        times(&mut shift, start, end);
        value["domain"]["entities"][test_support::id(index)] = shift;
    }

    fn rule(value: &mut Value, mode: &Value, maximum: u32) {
        value["domain"]["rules"][test_support::id(20)] = json!({
            "id": test_support::id(20),
            "kind": "maximumConsecutive",
            "active": true,
            "strength": "required",
            "scope": {"people": {"kind": "all"}},
            "mode": mode,
            "maximum": maximum
        });
    }

    fn worked_rule(value: &mut Value, maximum: u32) {
        rule(value, &json!({"kind": "workedDays"}), maximum);
    }

    fn assignments_rule(value: &mut Value, maximum: u32, break_minutes: u32) {
        rule(
            value,
            &json!({"kind": "assignments", "breakMinutes": break_minutes}),
            maximum,
        );
    }

    fn evaluate(value: &Value, pairs: &[(u32, u32)]) -> TestResult<AssignmentRuleEvaluation> {
        let document: ScenarioDocument = serde_json::from_value(value.clone())?;
        let selected = pairs
            .iter()
            .map(|(person, shift)| {
                Ok(AssignmentPair {
                    person_id: test_support::id(*person).parse()?,
                    shift_id: test_support::id(*shift).parse()?,
                })
            })
            .collect::<TestResult<Vec<_>>>()?;
        Ok(crate::assignment_rules::evaluate_assignment_rules(
            &document, &selected, None,
        )?)
    }

    fn record(value: &Value, pairs: &[(u32, u32)]) -> TestResult<RuleEvaluation> {
        let result = evaluate(value, pairs)?;
        let rule_id: eutheto_types::RuleId = test_support::id(20).parse()?;
        result
            .evaluations
            .into_iter()
            .find(|value| value.rule_id == rule_id)
            .ok_or_else(|| "maximum-consecutive evaluation missing".into())
    }

    fn observed(value: &RuleEvaluation, key: &str) -> TestResult<i64> {
        match value
            .observed
            .get(&VerificationFactId::new(key)?)
            .ok_or("missing observed fact")?
        {
            VerificationValue::Integer(value) => Ok(*value),
            _ => Err("observed fact is not an integer".into()),
        }
    }

    #[test]
    fn worked_days_are_distinct_and_use_civil_adjacency_across_month_and_dst() -> TestResult {
        let mut value = base()?;
        value["settings"]["timeZone"] = json!("America/New_York");
        value["settings"]["horizon"] =
            json!({"start":"2026-10-31T04:00:00Z","end":"2026-11-04T05:00:00Z"});
        zoned_times(
            &mut value["domain"]["entities"][test_support::id(8)],
            "2026-11-01T01:30:00",
            "2026-11-01T05:30:00",
            -14_400,
            "2026-11-01T02:30:00",
            "2026-11-01T07:30:00",
            -18_000,
        );
        add_shift(&mut value, 30, "2026-10-31T08:00:00", "2026-10-31T10:00:00");
        zoned_times(
            &mut value["domain"]["entities"][test_support::id(30)],
            "2026-10-31T08:00:00",
            "2026-10-31T12:00:00",
            -14_400,
            "2026-10-31T10:00:00",
            "2026-10-31T14:00:00",
            -14_400,
        );
        add_shift(&mut value, 31, "2026-11-01T11:00:00", "2026-11-01T12:00:00");
        zoned_times(
            &mut value["domain"]["entities"][test_support::id(31)],
            "2026-11-01T11:00:00",
            "2026-11-01T16:00:00",
            -18_000,
            "2026-11-01T12:00:00",
            "2026-11-01T17:00:00",
            -18_000,
        );
        add_shift(&mut value, 32, "2026-11-02T08:00:00", "2026-11-02T10:00:00");
        zoned_times(
            &mut value["domain"]["entities"][test_support::id(32)],
            "2026-11-02T08:00:00",
            "2026-11-02T13:00:00",
            -18_000,
            "2026-11-02T10:00:00",
            "2026-11-02T15:00:00",
            -18_000,
        );
        worked_rule(&mut value, 2);
        let result = record(&value, &[(1, 30), (1, 8), (1, 31), (1, 32)])?;
        assert!(!result.satisfied);
        assert_eq!(
            observed(&result, "official.workforce.fact.witness_count")?,
            3
        );
        Ok(())
    }

    #[test]
    fn worked_days_do_not_bridge_sparse_civil_dates() -> TestResult {
        let mut value = base()?;
        add_shift(&mut value, 30, "2026-10-31T08:00:00", "2026-10-31T10:00:00");
        add_shift(&mut value, 32, "2026-11-02T08:00:00", "2026-11-02T10:00:00");
        worked_rule(&mut value, 1);
        let result = record(&value, &[(1, 30), (1, 32)])?;
        assert!(result.satisfied);
        assert_eq!(
            observed(&result, "official.workforce.fact.checked_predicate_count")?,
            2
        );
        Ok(())
    }

    #[test]
    fn assignments_join_at_exact_break_but_not_one_nanosecond_over() -> TestResult {
        let mut value = base()?;
        times(
            &mut value["domain"]["entities"][test_support::id(8)],
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        add_shift(&mut value, 30, "2026-11-01T10:30:00", "2026-11-01T11:30:00");
        assignments_rule(&mut value, 1, 30);
        let exact = record(&value, &[(1, 8), (1, 30)])?;
        assert!(!exact.satisfied);
        assert_eq!(
            observed(&exact, "official.workforce.fact.witness_count")?,
            2
        );

        times(
            &mut value["domain"]["entities"][test_support::id(30)],
            "2026-11-01T10:30:00.000000001",
            "2026-11-01T11:30:00.000000001",
        );
        let over = record(&value, &[(1, 8), (1, 30)])?;
        assert!(over.satisfied);
        Ok(())
    }

    #[test]
    fn assignments_ignore_nonmatching_selected_shifts_and_join_overlaps_and_ties() -> TestResult {
        let mut value = base()?;
        times(
            &mut value["domain"]["entities"][test_support::id(8)],
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        let mut other_type = value["domain"]["entities"][test_support::id(4)].clone();
        other_type["id"] = json!(test_support::id(40));
        other_type["category"] = json!("other");
        value["domain"]["entities"][test_support::id(40)] = other_type;
        value["domain"]["entities"][test_support::id(1)]["eligibleAssignmentTypeIds"] =
            json!([test_support::id(4), test_support::id(40)]);
        add_shift(&mut value, 30, "2026-11-01T10:30:00", "2026-11-01T11:30:00");
        add_shift(&mut value, 31, "2026-11-01T10:05:00", "2026-11-01T10:10:00");
        value["domain"]["entities"][test_support::id(31)]["assignmentTypeId"] =
            json!(test_support::id(40));
        value["domain"]["rules"][test_support::id(20)] = json!({
            "id": test_support::id(20), "kind": "maximumConsecutive", "active": true,
            "strength": "required", "scope": {"people": {"kind": "all"}, "categories": ["clinic"]},
            "mode": {"kind": "assignments", "breakMinutes": 30}, "maximum": 1
        });
        let result = record(&value, &[(1, 31), (1, 30), (1, 8)])?;
        assert!(!result.satisfied);
        assert_eq!(
            observed(&result, "official.workforce.fact.witness_count")?,
            2
        );

        add_shift(&mut value, 32, "2026-11-01T08:00:00", "2026-11-01T10:00:00");
        let tied = record(&value, &[(1, 32), (1, 8)])?;
        assert!(!tied.satisfied);
        assert!(tied.affected_entities.iter().any(|entity| {
            entity.kind.as_str() == "official.workforce.shift"
                && entity.id.as_str() == format!("official.workforce.{}", test_support::id(8))
        }));
        Ok(())
    }

    #[test]
    fn zero_maximum_rejects_a_match_but_empty_selection_is_vacuous() -> TestResult {
        let mut value = base()?;
        worked_rule(&mut value, 0);
        assert!(!record(&value, &[(1, 8)])?.satisfied);
        let empty = record(&value, &[])?;
        assert!(empty.satisfied);
        assert_eq!(
            observed(&empty, "official.workforce.fact.checked_predicate_count")?,
            0
        );
        Ok(())
    }

    #[test]
    fn original_evaluation_rejects_a_mutated_candidate_without_compiler_population() -> TestResult {
        let mut value = base()?;
        value["domain"]["entities"][test_support::id(1)]["eligibleAssignmentTypeIds"] = json!([]);
        worked_rule(&mut value, 0);
        let result = record(&value, &[(1, 8)])?;
        assert!(!result.satisfied);
        assert_eq!(
            observed(&result, "official.workforce.fact.witness_count")?,
            1
        );
        Ok(())
    }
}
