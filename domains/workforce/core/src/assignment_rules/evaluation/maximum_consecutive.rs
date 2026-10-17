use super::super::{
    AssignmentRuleError,
    budget::{OperationBudget, add},
    input::AssignmentInput,
};
use super::{InstantInterval, Selected, Summary, Witness, arithmetic, invalid, predicates};
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
                evaluate_worked_days(
                    input, selected, scope, *person_id, *id, *maximum, &matching, summary, budget,
                )?;
            }
            ConsecutiveMode::Assignments { break_minutes } => evaluate_assignments(
                input,
                selected,
                scope,
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

#[allow(clippy::too_many_arguments)]
fn evaluate_worked_days(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    person_id: PersonId,
    rule_id: RuleId,
    maximum: u32,
    matching: &[&ResolvedShift],
    summary: &mut Summary,
    budget: &mut OperationBudget<'_>,
) -> Result<(), AssignmentRuleError> {
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
    let mut run_start: Option<(Date, &ResolvedShift)> = None;
    let mut run_end: Option<(Date, &ResolvedShift)> = None;
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
            if let (Some((start, first)), Some((end, last))) = (run_start, run_end) {
                record_run(
                    input,
                    selected,
                    scope,
                    person_id,
                    rule_id,
                    maximum,
                    first,
                    last,
                    &[],
                    Some((start, end)),
                    run_length,
                    summary,
                    budget,
                )?;
            }
            run_start = Some((date, shift));
            run_length = 1;
        }
        run_end = Some((date, shift));
        previous_date = Some(date);
    }
    if let (Some((start, first)), Some((end, last))) = (run_start, run_end) {
        record_run(
            input,
            selected,
            scope,
            person_id,
            rule_id,
            maximum,
            first,
            last,
            &[],
            Some((start, end)),
            run_length,
            summary,
            budget,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn evaluate_assignments(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
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
    let mut run_start_index = 0;
    let mut run_length = 0_u64;
    for (index, &shift) in matching.iter().enumerate() {
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
            if let (Some(first), Some(last)) = (run_start, previous) {
                record_run(
                    input,
                    selected,
                    scope,
                    person_id,
                    rule_id,
                    maximum,
                    first,
                    last,
                    &matching[run_start_index..index],
                    None,
                    run_length,
                    summary,
                    budget,
                )?;
            }
            run_start = Some(shift);
            run_start_index = index;
            run_length = 1;
        }
        previous = Some(shift);
    }
    if let (Some(first), Some(last)) = (run_start, previous) {
        record_run(
            input,
            selected,
            scope,
            person_id,
            rule_id,
            maximum,
            first,
            last,
            &matching[run_start_index..],
            None,
            run_length,
            summary,
            budget,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_run(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    person_id: PersonId,
    rule_id: RuleId,
    maximum: u32,
    first: &ResolvedShift,
    last: &ResolvedShift,
    run_members: &[&ResolvedShift],
    dates: Option<(Date, Date)>,
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
    let failed = actual > u64::from(maximum);
    if failed && summary.collecting_repair() {
        let members = association_members(
            input,
            selected,
            scope,
            person_id,
            run_members,
            dates,
            budget,
        )?;
        let mut observation = witness;
        observation.interval = if dates.is_some() {
            None
        } else {
            let first_interval = predicates::interval(first);
            let mut end = predicates::interval(last).end;
            for member in run_members {
                budget.step()?;
                let member_end = predicates::interval(member).end;
                if member_end > end {
                    end = member_end;
                }
            }
            Some(InstantInterval {
                start: first_interval.start,
                end,
            })
        };
        observation.consecutive_start_date = dates.map(|(start, _)| start);
        observation.consecutive_end_date = dates.map(|(_, end)| end);
        let association_kind = if dates.is_some() {
            "maximum_consecutive_worked_days"
        } else {
            "maximum_consecutive_assignments"
        };
        summary.predicate_with_observation(
            failed,
            &witness,
            &observation,
            Some(members),
            association_kind,
            budget,
        )
    } else {
        summary.predicate(failed, &witness, budget)
    }
}

fn association_members(
    input: &AssignmentInput,
    selected: &Selected<'_>,
    scope: &Scope,
    person_id: PersonId,
    run_members: &[&ResolvedShift],
    dates: Option<(Date, Date)>,
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
            let metadata = input.metadata(shift)?;
            if !predicates::shift_matches(shift, &metadata, scope, budget)? {
                continue;
            }
            let in_run = if let Some((start, end)) = dates {
                (start..=end).contains(&shift.reporting_date)
            } else {
                let mut related = false;
                for member in run_members {
                    budget.step()?;
                    if member.id == pair.shift_id {
                        related = true;
                        break;
                    }
                }
                related
            };
            if in_run {
                budget.reserve(0, 1, 48)?;
                members.push(*pair);
            }
        }
    }
    Ok(members)
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
    fn repair_keeps_distinct_full_runs_and_nonfirst_changed_members() -> TestResult {
        use super::super::super::{budget::OperationBudget, input::AssignmentInput};
        use super::super::evaluate_repair_selection;

        let mut value = base()?;
        value["settings"]["horizon"] =
            json!({"start":"2026-10-31T00:00:00Z","end":"2026-11-06T00:00:00Z"});
        add_shift(&mut value, 30, "2026-11-02T08:00:00", "2026-11-02T10:00:00");
        add_shift(&mut value, 32, "2026-11-04T08:00:00", "2026-11-04T10:00:00");
        add_shift(&mut value, 33, "2026-11-05T08:00:00", "2026-11-05T10:00:00");
        worked_rule(&mut value, 1);
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let pair = |shift: u32| -> TestResult<AssignmentPair> {
            Ok(AssignmentPair {
                person_id: test_support::id(1).parse()?,
                shift_id: test_support::id(shift).parse()?,
            })
        };
        let selected = [pair(8)?, pair(30)?, pair(32)?, pair(33)?];
        let associations = [pair(30)?, pair(33)?];
        let mut budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (_, _, observations) =
            evaluate_repair_selection(&document, &input, &selected, &associations, &mut budget)?;
        let rule_id: eutheto_types::RuleId = test_support::id(20).parse()?;
        let runs: Vec<_> = observations
            .iter()
            .filter(|value| value.evaluation.rule_id == rule_id)
            .collect();
        assert_eq!(runs.len(), 2);
        for (observation, member, start, end) in [
            (runs[0], pair(30)?, "2026-11-01", "2026-11-02"),
            (runs[1], pair(33)?, "2026-11-04", "2026-11-05"),
        ] {
            assert_eq!(observation.affected_pairs, [member]);
            assert_eq!(
                match observation
                    .evaluation
                    .observed
                    .get(&VerificationFactId::new(
                        "official.workforce.fact.consecutive_run_start_date",
                    )?)
                    .ok_or("run start missing")?
                {
                    VerificationValue::Text(value) => value,
                    _ => return Err("run start is not text".into()),
                },
                start
            );
            assert_eq!(
                match observation
                    .evaluation
                    .observed
                    .get(&VerificationFactId::new(
                        "official.workforce.fact.consecutive_run_end_date",
                    )?)
                    .ok_or("run end missing")?
                {
                    VerificationValue::Text(value) => value,
                    _ => return Err("run end is not text".into()),
                },
                end
            );
        }
        Ok(())
    }
    #[test]
    fn repair_assignment_run_includes_selected_nested_ending_member() -> TestResult {
        use super::super::super::{budget::OperationBudget, input::AssignmentInput};
        use super::super::evaluate_repair_selection;

        let mut value = base()?;
        add_shift(&mut value, 30, "2026-11-01T10:20:00", "2026-11-01T11:00:00");
        add_shift(&mut value, 31, "2026-11-01T11:20:00", "2026-11-01T12:00:00");
        add_shift(&mut value, 32, "2026-11-01T10:30:00", "2026-11-01T12:30:00");
        assignments_rule(&mut value, 1, 30);
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let pair = |shift: u32| -> TestResult<AssignmentPair> {
            Ok(AssignmentPair {
                person_id: test_support::id(1).parse()?,
                shift_id: test_support::id(shift).parse()?,
            })
        };
        let selected = [pair(8)?, pair(30)?, pair(32)?, pair(31)?];
        let associations = [pair(32)?];
        let mut budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (_, _, observations) =
            evaluate_repair_selection(&document, &input, &selected, &associations, &mut budget)?;
        let rule_id: eutheto_types::RuleId = test_support::id(20).parse()?;
        let run = observations
            .iter()
            .find(|value| value.evaluation.rule_id == rule_id)
            .ok_or("assignment-run observation missing")?;
        assert_eq!(run.affected_pairs, associations);
        assert!(matches!(
            run.evaluation
                .observed
                .get(&VerificationFactId::new("official.workforce.fact.witness_end")?),
            Some(VerificationValue::Text(value)) if value == "2026-11-01T12:30:00Z"
        ));
        Ok(())
    }

    #[test]
    fn repair_assignment_groups_follow_native_preceding_end_order() -> TestResult {
        use super::super::super::{budget::OperationBudget, input::AssignmentInput};
        use super::super::evaluate_repair_selection;

        let mut value = base()?;
        times(
            &mut value["domain"]["entities"][test_support::id(8)],
            "2026-11-01T00:00:00",
            "2026-11-01T10:00:00",
        );
        add_shift(&mut value, 30, "2026-11-01T01:00:00", "2026-11-01T02:00:00");
        add_shift(&mut value, 31, "2026-11-01T08:00:00", "2026-11-01T09:00:00");
        assignments_rule(&mut value, 1, 30);
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let pair = |shift: u32| -> TestResult<AssignmentPair> {
            Ok(AssignmentPair {
                person_id: test_support::id(1).parse()?,
                shift_id: test_support::id(shift).parse()?,
            })
        };
        let selected = [pair(8)?, pair(30)?, pair(31)?];
        let associations = [pair(31)?];
        let mut budget = OperationBudget::evaluation(None);
        let input = AssignmentInput::new(&document, &mut budget)?;
        let (_, _, observations) =
            evaluate_repair_selection(&document, &input, &selected, &associations, &mut budget)?;
        let rule_id: eutheto_types::RuleId = test_support::id(20).parse()?;
        let failures: Vec<_> = observations
            .iter()
            .filter(|value| value.evaluation.rule_id == rule_id)
            .collect();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].affected_pairs.is_empty());
        Ok(())
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
