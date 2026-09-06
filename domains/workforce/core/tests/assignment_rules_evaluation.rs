mod support;

use eutheto_domain_ir::{RuleEvaluation, VerificationFactId, VerificationValue};
use eutheto_types::{CancellationToken, RuleId, ScenarioDocument};
use eutheto_workforce::{
    assignment_rules::{AssignmentRuleError, AssignmentRuleEvaluation, AssignmentRuleLimit, SelectionIssueKind, evaluate_assignment_rules},
    model::AssignmentPair,
};
use serde_json::{Value, json};
use std::error::Error;
use support::id;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn base() -> Result<Value> {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["domain"]["entities"].as_object_mut().ok_or("entities")?.remove(&id(6));
    value["domain"]["lockedAssignments"] = json!({});
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] = json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    times(&mut value["domain"]["entities"][id(8)], "2026-11-01T08:00:00", "2026-11-01T10:00:00");
    Ok(value)
}

fn times(shift: &mut Value, start: &str, end: &str) {
    shift["startsAt"] = json!({"instant":format!("{start}Z"),"local":start,"offsetSeconds":0});
    shift["endsAt"] = json!({"instant":format!("{end}Z"),"local":end,"offsetSeconds":0});
}

fn rule(value: &mut Value, index: u32, kind: &str) {
    value["domain"]["rules"][id(index)] = json!({"id":id(index),"kind":kind,"active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
    if kind == "noOverlap" { value["domain"]["rules"][id(index)]["compatibleCategoryPairs"] = json!([]); }
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair { person_id: id(person).parse()?, shift_id: id(shift).parse()? })
}

fn run(value: &Value, pairs: &[(u32, u32)]) -> Result<AssignmentRuleEvaluation> {
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let selected = pairs.iter().map(|(person, shift)| pair(*person, *shift)).collect::<Result<Vec<_>>>()?;
    Ok(evaluate_assignment_rules(&document, &selected, None)?)
}

fn evaluation(result: &AssignmentRuleEvaluation, index: u32) -> Result<&RuleEvaluation> {
    let rule_id: RuleId = id(index).parse()?;
    result.evaluations.iter().find(|evaluation| evaluation.rule_id == rule_id).ok_or_else(|| "missing evaluation".into())
}

fn totals(result: &AssignmentRuleEvaluation, index: u32) -> Result<(i64, i64)> {
    let record = evaluation(result, index)?;
    record.validate()?;
    let checked = record.observed.get(&VerificationFactId::new("official.workforce.fact.checked_predicate_count")?).ok_or("checked")?;
    let violations = record.observed.get(&VerificationFactId::new("official.workforce.fact.violation_count")?).ok_or("violations")?;
    let (VerificationValue::Integer(checked), VerificationValue::Integer(violations)) = (checked, violations) else { return Err("non-integer totals".into()); };
    assert_eq!(record.satisfied, *violations == 0);
    assert_eq!(record.expected.get(&VerificationFactId::new("official.workforce.fact.violation_count")?), Some(&VerificationValue::Integer(0)));
    Ok((*checked, *violations))
}

fn availability(value: &mut Value, index: u32, kind: &str, start: &str, end: &str) {
    value["domain"]["entities"][id(index)] = json!({"id":id(index),"kind":"availability","personId":id(1),"availabilityKind":kind,
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "timeWindow":{"kind":"instant","startsAt":start,"endsAt":end},"source":"private source","note":"private note"});
}

fn another_person(value: &mut Value, index: u32) {
    let mut person = value["domain"]["entities"][id(1)].clone();
    person["id"] = json!(id(index));
    if let Some(person) = person.as_object_mut() { person.remove("externalId"); }
    value["domain"]["entities"][id(index)] = person;
}

fn another_shift(value: &mut Value, index: u32, start: &str, end: &str) {
    let mut shift = value["domain"]["entities"][id(8)].clone();
    shift["id"] = json!(id(index));
    times(&mut shift, start, end);
    value["domain"]["entities"][id(index)] = shift;
}

#[test]
fn empty_selection_and_toggled_rules_have_truthful_vacuous_summaries() -> Result {
    let mut value = base()?;
    for (index, kind) in [(20, "eligibility"), (21, "availability"), (22, "noOverlap")] { rule(&mut value, index, kind); }
    let empty = run(&value, &[])?;
    for index in [1, 20, 21, 22] { assert_eq!(totals(&empty, index)?, (0, 0)); }
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([]);
    let active = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&active, 20)?, (2, 2));
    value["domain"]["rules"][id(20)]["scope"]["people"] = json!({"kind":"filter","allTags":["absent"],"anyTags":[]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (0, 0));
    value["domain"]["rules"][id(20)]["active"] = json!(false);
    let disabled = run(&value, &[(1, 8)])?;
    assert!(!disabled.obligations.handled.contains(&id(20).parse()?));
    assert!(!disabled.obligations.remaining.contains(&id(20).parse()?));
    Ok(())
}

#[test]
fn all_scope_dimensions_intersect_without_broadening_empty_matches() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(30)] = json!({"kind":"team","id":id(30),"name":"Team"});
    value["domain"]["entities"][id(31)] = json!({"kind":"team","id":id(31),"name":"Other"});
    value["domain"]["entities"][id(1)]["teamIds"] = json!([id(30)]);
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    rule(&mut value, 20, "eligibility");
    let full_scope = json!({"people":{"kind":"selected","personIds":[id(1)]},"teamIds":[id(30)],"assignmentTypeIds":[id(4)],"categories":["clinic"],"weekdays":["sunday"],"locationIds":[id(5)]});
    value["domain"]["rules"][id(20)]["scope"] = full_scope.clone();
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    for (field, replacement) in [("teamIds", json!([id(31)])), ("categories", json!(["other"])), ("weekdays", json!(["monday"]))] {
        value["domain"]["rules"][id(20)]["scope"] = full_scope.clone();
        value["domain"]["rules"][id(20)]["scope"][field] = replacement;
        assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (0, 0));
    }
    value["domain"]["rules"][id(20)]["scope"] = full_scope;
    value["domain"]["entities"][id(4)]["locationBehavior"] = json!({"kind":"optional"});
    value["domain"]["entities"][id(8)].as_object_mut().ok_or("shift")?.remove("locationId");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (0, 0));
    Ok(())
}

#[test]
fn malformed_selections_are_rejected_even_without_authored_rules() -> Result {
    let document: ScenarioDocument = serde_json::from_value(base()?)?;
    for (selected, expected) in [
        (vec![pair(1, 8)?, pair(1, 8)?], SelectionIssueKind::DuplicatePair),
        (vec![pair(999, 8)?], SelectionIssueKind::MissingPerson),
        (vec![pair(4, 8)?], SelectionIssueKind::WrongKindPerson),
        (vec![pair(1, 999)?], SelectionIssueKind::UnresolvedShift),
    ] {
        assert!(matches!(evaluate_assignment_rules(&document, &selected, None), Err(AssignmentRuleError::InvalidSelection { kind, .. }) if kind == expected));
    }
    Ok(())
}

#[test]
fn dormant_excluded_occurrence_is_not_a_resolved_selection() -> Result {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["domain"]["lockedAssignments"] = json!({});
    value["domain"]["entities"][id(6)]["recurrence"]["excludedDates"] = json!(["2026-11-01"]);
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert!(matches!(evaluate_assignment_rules(&document, &[pair(1, 7)?], None), Err(AssignmentRuleError::InvalidSelection { kind: SelectionIssueKind::UnresolvedShift, .. })));
    Ok(())
}

#[test]
fn unconditional_activity_and_approved_leave_survive_rule_toggles() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(1)]["activeRange"] = json!({"kind":"dateRange","startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    availability(&mut value, 30, "approvedTimeOff", "2026-11-01T09:00:00Z", "2026-11-01T09:30:00Z");
    availability(&mut value, 31, "requestedTimeOff", "2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z");
    rule(&mut value, 20, "eligibility");
    rule(&mut value, 21, "availability");
    value["domain"]["rules"][id(20)]["active"] = json!(false);
    value["domain"]["rules"][id(21)]["scope"]["categories"] = json!(["other"]);
    let result = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&result, 1)?, (1, 1));
    assert_eq!(totals(&result, 30)?, (1, 1));
    assert_eq!(totals(&result, 21)?, (0, 0));
    assert!(!result.obligations.handled.contains(&id(31).parse()?));
    value["domain"]["entities"][id(30)]["locationIds"] = json!([id(5)]);
    value["domain"]["entities"][id(4)]["locationBehavior"] = json!({"kind":"optional"});
    value["domain"]["entities"][id(8)].as_object_mut().ok_or("shift")?.remove("locationId");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 30)?, (0, 0));
    Ok(())
}

#[test]
fn activity_covers_the_entire_post_horizon_shift_tail() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(1)]["activeRange"] = json!({"kind":"dateRange","startDate":"2026-11-01","endDateExclusive":"2026-11-02"});
    times(&mut value["domain"]["entities"][id(8)], "2026-11-01T23:00:00", "2026-11-02T00:00:00");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 1)?, (1, 0));
    times(&mut value["domain"]["entities"][id(8)], "2026-11-01T23:00:00", "2026-11-02T00:00:00.000000001");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 1)?, (1, 1));
    Ok(())
}

#[test]
fn same_qualification_renewals_union_but_nanosecond_gaps_do_not() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "eligibility");
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([
        {"qualificationId":id(11),"effectiveFrom":"2026-11-01T09:00:00Z","expiresAt":"2026-11-01T10:00:00Z"},
        {"qualificationId":id(11),"effectiveFrom":"2026-11-01T08:00:00Z","expiresAt":"2026-11-01T09:00:00Z"}
    ]);
    let renewal = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&renewal, 20)?, (2, 0));
    value["domain"]["entities"][id(1)]["qualificationGrants"][0]["effectiveFrom"] = json!("2026-11-01T09:00:00.000000001Z");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([{ "qualificationId":id(11),"expiresAt":"2026-11-01T08:00:00Z" }]);
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    Ok(())
}

#[test]
fn different_partial_qualifications_cannot_alternate_for_any_expression() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "eligibility");
    value["domain"]["entities"][id(12)] = json!({"kind":"qualification","id":id(12),"name":"Second","description":""});
    value["domain"]["entities"][id(4)]["qualifications"] = json!({"kind":"matches","allQualificationIds":[],"anyQualificationIds":[id(11),id(12)]});
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([
        {"qualificationId":id(11),"expiresAt":"2026-11-01T09:00:00Z"},
        {"qualificationId":id(12),"effectiveFrom":"2026-11-01T09:00:00Z"}
    ]);
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    value["domain"]["entities"][id(1)]["qualificationGrants"][1].as_object_mut().ok_or("grant")?.remove("effectiveFrom");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 0));
    value["domain"]["entities"][id(4)]["qualifications"] = json!({"kind":"matches","allQualificationIds":[id(11),id(12)],"anyQualificationIds":[]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    Ok(())
}

#[test]
fn distinct_available_only_records_are_conjunctive_and_windows_union() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(&mut value, 30, "availableOnly", "2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z");
    value["domain"]["entities"][id(30)]["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["sunday"],"startTime":"09:00:00","endTime":"10:00:00","endDayOffset":0},
        {"weekdays":["sunday"],"startTime":"08:00:00","endTime":"09:00:00","endDayOffset":0}
    ]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 0));
    availability(&mut value, 31, "availableOnly", "2026-11-01T08:00:00Z", "2026-11-01T09:00:00Z");
    let failed = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&failed, 20)?, (2, 1));
    let encoded = serde_json::to_string(evaluation(&failed, 20)?)?;
    assert!(!encoded.contains("private"));
    value["domain"]["entities"][id(31)]["effectiveRange"] = json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 0));
    Ok(())
}

#[test]
fn effective_range_clips_pre_effective_overnight_windows_and_full_shift_tails() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    times(&mut value["domain"]["entities"][id(8)], "2026-11-01T23:30:00", "2026-11-02T02:00:00");
    availability(&mut value, 30, "availableOnly", "2026-11-02T00:00:00Z", "2026-11-02T02:00:00Z");
    value["domain"]["entities"][id(30)]["effectiveRange"] = json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 0));
    value["domain"]["entities"][id(30)]["availabilityKind"] = json!("unavailable");
    value["domain"]["entities"][id(30)]["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["sunday"],"startTime":"23:00:00","endTime":"01:00:00","endDayOffset":1}
    ]});
    let result = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&result, 20)?, (1, 1));
    assert_eq!(evaluation(&result, 20)?.observed.get(&VerificationFactId::new("official.workforce.fact.witness_start")?), Some(&VerificationValue::Text("2026-11-02T00:00:00Z".to_owned())));
    Ok(())
}

#[test]
fn unavailable_touching_endpoints_does_not_overlap() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(&mut value, 30, "unavailable", "2026-11-01T06:00:00Z", "2026-11-01T08:00:00Z");
    availability(&mut value, 31, "unavailable", "2026-11-01T10:00:00Z", "2026-11-01T12:00:00Z");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 0));
    value["domain"]["entities"][id(30)]["timeWindow"]["endsAt"] = json!("2026-11-01T08:00:00.000000001Z");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    Ok(())
}

#[test]
fn coverage_counts_scoped_selected_people_without_filtering_invalid_assignments() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "coverage");
    rule(&mut value, 21, "eligibility");
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    let result = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&result, 20)?, (1, 0));
    assert_eq!(totals(&result, 21)?, (2, 1));
    another_person(&mut value, 30);
    value["domain"]["entities"][id(30)]["tags"] = json!([]);
    value["domain"]["rules"][id(20)]["scope"]["people"] = json!({"kind":"filter","allTags":["night"],"anyTags":[]});
    assert_eq!(totals(&run(&value, &[(30, 8)])?, 20)?, (1, 1));
    assert_eq!(totals(&run(&value, &[(1, 8), (30, 8)])?, 20)?, (1, 0));
    value["domain"]["rules"][id(20)]["scope"]["people"] = json!({"kind":"all"});
    assert_eq!(totals(&run(&value, &[(1, 8), (30, 8)])?, 20)?, (1, 1));
    Ok(())
}

#[test]
fn coverage_minima_deduplicate_within_owner_but_not_across_owners() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "coverage");
    value["domain"]["entities"][id(12)] = json!({"kind":"qualification","id":id(12),"name":"Second","description":""});
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    let minimum = json!({"qualifications":{"allQualificationIds":[id(11),id(12)],"anyQualificationIds":[]},"minimum":1});
    let mut reversed = minimum.clone();
    reversed["qualifications"]["allQualificationIds"] = json!([id(12),id(11)]);
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] = json!([minimum, reversed]);
    value["domain"]["entities"][id(30)] = json!({"kind":"coverageRequirement","id":id(30),"active":true,"scope":{"kind":"all"},"coverage":value["domain"]["entities"][id(8)]["coverage"].clone()});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (4, 0));
    assert_eq!(totals(&run(&value, &[])?, 20)?, (4, 4));
    value["domain"]["entities"][id(30)]["active"] = json!(false);
    assert_eq!(totals(&run(&value, &[])?, 20)?, (2, 2));
    Ok(())
}

#[test]
fn dual_qualified_people_count_independently_in_each_minimum() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "coverage");
    value["domain"]["entities"][id(12)] = json!({"kind":"qualification","id":id(12),"name":"Second","description":""});
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] = json!([
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":1},
        {"qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},"minimum":1}
    ]);
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (3, 0));
    value["domain"]["entities"][id(1)]["qualificationGrants"][1]["expiresAt"] = json!("2026-11-01T09:00:00Z");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (3, 1));
    Ok(())
}

#[test]
fn coverage_zero_bounds_and_preferred_counts_have_correct_hard_meaning() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "coverage");
    value["domain"]["entities"][id(8)]["coverage"] = json!({"kind":"atLeast","minimum":0,"preferredCount":5,"maximumCount":10,"qualificationMinimums":[]});
    let empty = run(&value, &[])?;
    assert_eq!(totals(&empty, 20)?, (1, 0));
    value["domain"]["entities"][id(8)]["coverage"]["preferredCount"] = json!(9);
    assert_eq!(empty.evaluations, run(&value, &[])?.evaluations);
    value["domain"]["entities"][id(8)]["coverage"] = json!({"kind":"exact","count":0,"qualificationMinimums":[]});
    assert_eq!(totals(&run(&value, &[])?, 20)?, (1, 0));
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 1));
    value["domain"]["entities"][id(8)]["coverage"] = json!({"kind":"atLeast","minimum":2,"qualificationMinimums":[]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 1));
    Ok(())
}

#[test]
fn coverage_requirement_dates_use_start_date_not_reporting_date() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "coverage");
    times(&mut value["domain"]["entities"][id(8)], "2026-11-01T23:00:00", "2026-11-02T02:00:00");
    value["domain"]["entities"][id(8)]["reportingAttribution"] = json!("endLocalDate");
    value["domain"]["rules"][id(20)]["scope"]["weekdays"] = json!(["monday"]);
    value["domain"]["entities"][id(30)] = json!({"kind":"coverageRequirement","id":id(30),"active":true,"scope":{"kind":"filter","startDateRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"}},"coverage":{"kind":"exact","count":2,"qualificationMinimums":[]}});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    value["domain"]["entities"][id(30)]["scope"]["startDateRange"] = json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 0));
    Ok(())
}

#[test]
fn overlap_checks_both_scopes_and_half_open_boundaries() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    another_shift(&mut value, 30, "2026-11-01T10:00:00", "2026-11-01T12:00:00");
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (1, 0));
    times(&mut value["domain"]["entities"][id(30)], "2026-11-01T09:59:59.999999999", "2026-11-01T12:00:00");
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (1, 1));
    value["domain"]["entities"][id(31)] = value["domain"]["entities"][id(4)].clone();
    value["domain"]["entities"][id(31)]["id"] = json!(id(31));
    value["domain"]["entities"][id(31)]["category"] = json!("other");
    value["domain"]["entities"][id(30)]["assignmentTypeId"] = json!(id(31));
    value["domain"]["rules"][id(20)]["scope"]["categories"] = json!(["clinic"]);
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (0, 0));
    Ok(())
}

#[test]
fn compatibility_is_symmetric_and_exempts_only_its_own_rule() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    rule(&mut value, 21, "noOverlap");
    another_shift(&mut value, 30, "2026-11-01T09:00:00", "2026-11-01T11:00:00");
    value["domain"]["entities"][id(31)] = value["domain"]["entities"][id(4)].clone();
    value["domain"]["entities"][id(31)]["id"] = json!(id(31));
    value["domain"]["entities"][id(31)]["category"] = json!("alpha");
    value["domain"]["entities"][id(30)]["assignmentTypeId"] = json!(id(31));
    value["domain"]["rules"][id(20)]["compatibleCategoryPairs"] = json!([{"firstCategory":"alpha","secondCategory":"clinic"}]);
    let result = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&result, 20)?, (1, 0));
    assert_eq!(totals(&result, 21)?, (1, 1));
    times(&mut value["domain"]["entities"][id(30)], "2026-11-01T07:00:00", "2026-11-01T09:00:00");
    assert_eq!(totals(&run(&value, &[(1, 30), (1, 8)])?, 20)?, (1, 0));
    Ok(())
}

#[test]
fn required_partition_retains_other_active_families_and_only_hard_locks() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "eligibility");
    rule(&mut value, 21, "maximumConsecutive");
    value["domain"]["rules"][id(21)]["mode"] = json!({"kind":"workedDays"});
    value["domain"]["rules"][id(21)]["maximum"] = json!(3);
    rule(&mut value, 22, "availability");
    value["domain"]["rules"][id(22)]["active"] = json!(false);
    availability(&mut value, 30, "approvedTimeOff", "2026-11-01T08:00:00Z", "2026-11-01T09:00:00Z");
    availability(&mut value, 31, "requestedTimeOff", "2026-11-01T08:00:00Z", "2026-11-01T09:00:00Z");
    for (index, state) in [(40, "hard"), (41, "soft"), (42, "unlocked")] {
        value["domain"]["lockedAssignments"][id(index)] = json!({"id":id(index),"personId":id(1),"shiftId":id(8),"state":{"kind":state}});
        if state == "soft" { value["domain"]["lockedAssignments"][id(index)]["state"]["stabilityWeight"] = json!(1); }
    }
    let result = run(&value, &[])?;
    assert_eq!(result.obligations.handled, [1, 20, 30].map(|index| id(index).parse()).into_iter().collect::<std::result::Result<Vec<RuleId>, _>>()?);
    assert_eq!(result.obligations.remaining, [21, 40].map(|index| id(index).parse()).into_iter().collect::<std::result::Result<Vec<RuleId>, _>>()?);
    Ok(())
}

#[test]
fn one_large_rule_has_exact_totals_and_a_bounded_deterministic_witness() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "eligibility");
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([]);
    let mut pairs = vec![(1, 8)];
    for index in 1000..1320 { another_person(&mut value, index); pairs.push((index, 8)); }
    let forward = run(&value, &pairs)?;
    assert_eq!(totals(&forward, 20)?, (642, 642));
    let rule_id: RuleId = id(20).parse()?;
    assert_eq!(forward.evaluations.iter().filter(|record| record.rule_id == rule_id).count(), 1);
    let record = evaluation(&forward, 20)?;
    assert!(record.affected_entities.len() <= 3);
    assert!(record.observed.len() <= 4);
    assert!(record.evidence.is_empty());
    pairs.reverse();
    assert_eq!(forward.evaluations, run(&value, &pairs)?.evaluations);
    Ok(())
}

#[test]
fn canonical_first_witness_ignores_window_and_minimum_order() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(&mut value, 30, "unavailable", "2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z");
    value["domain"]["entities"][id(30)]["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["sunday"],"startTime":"09:00:00","endTime":"09:30:00","endDayOffset":0},
        {"weekdays":["sunday"],"startTime":"08:00:00","endTime":"08:30:00","endDayOffset":0}
    ]});
    let first = run(&value, &[(1, 8)])?;
    value["domain"]["entities"][id(30)]["timeWindow"]["windows"].as_array_mut().ok_or("windows")?.reverse();
    assert_eq!(first.evaluations, run(&value, &[(1, 8)])?.evaluations);
    rule(&mut value, 21, "coverage");
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] = json!([
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":3},
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":2}
    ]);
    let first = run(&value, &[(1, 8)])?;
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"].as_array_mut().ok_or("minima")?.reverse();
    assert_eq!(first.evaluations, run(&value, &[(1, 8)])?.evaluations);
    Ok(())
}

#[test]
fn nonoverlap_bulk_count_exceeds_record_limits_without_emitting_pair_records() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    value["domain"]["entities"].as_object_mut().ok_or("entities")?.remove(&id(8));
    let prototype = base()?["domain"]["entities"][id(8)].clone();
    let mut pairs = Vec::new();
    for index in 0..800_u32 {
        let mut shift = prototype.clone();
        let minute = index;
        let start = format!("2026-11-01T{:02}:{:02}:00", minute / 60, minute % 60);
        let end = format!("2026-11-01T{:02}:{:02}:30", minute / 60, minute % 60);
        shift["id"] = json!(id(1000 + index));
        times(&mut shift, &start, &end);
        value["domain"]["entities"][id(1000 + index)] = shift;
        pairs.push((1, 1000 + index));
    }
    let result = run(&value, &pairs)?;
    assert_eq!(totals(&result, 20)?, (319_600, 0));
    assert_eq!(result.evaluations.len(), 2);
    Ok(())
}

#[test]
fn selected_limit_precedes_document_decode_and_real_cancellation_precedes_limit() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(1)]["name"] = json!(null);
    let malformed: ScenarioDocument = serde_json::from_value(value)?;
    let selected = vec![pair(1, 8)?; 100_001];
    assert_eq!(evaluate_assignment_rules(&malformed, &selected, None), Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::SelectedPairs)));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(evaluate_assignment_rules(&malformed, &selected, Some(&cancellation)), Err(AssignmentRuleError::Cancelled));
    let document: ScenarioDocument = serde_json::from_value(base()?)?;
    let cancellation = CancellationToken::new();
    assert_eq!(evaluate_assignment_rules(&document, &[pair(1, 8)?], Some(&cancellation))?.evaluations, evaluate_assignment_rules(&document, &[pair(1, 8)?], None)?.evaluations);
    Ok(())
}

#[test]
fn dense_overlap_obeys_the_common_work_budget_without_cancellation() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    value["domain"]["rules"][id(20)]["compatibleCategoryPairs"] = json!((0..40)
        .map(|index| json!({"firstCategory":format!("category{index:02}"),"secondCategory":"other"}))
        .collect::<Vec<_>>());
    let mut selected = vec![pair(1, 8)?];
    for index in 1000..2100 {
        another_shift(&mut value, index, "2026-11-01T08:00:00", "2026-11-01T10:00:00");
        selected.push(pair(1, index)?);
    }
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert_eq!(evaluate_assignment_rules(&document, &selected, None),
        Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)));
    Ok(())
}

#[test]
fn genuine_cancellation_is_observed_during_a_dense_operation() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    value["domain"]["rules"][id(20)]["compatibleCategoryPairs"] = json!((0..40)
        .map(|index| json!({"firstCategory":format!("category{index:02}"),"secondCategory":"other"}))
        .collect::<Vec<_>>());
    let mut selected = vec![pair(1, 8)?];
    for index in 1000..2100 {
        another_shift(&mut value, index, "2026-11-01T08:00:00", "2026-11-01T10:00:00");
        selected.push(pair(1, index)?);
    }
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let token = CancellationToken::new();
    let result = std::thread::scope(|scope| {
        let operation = scope.spawn(|| evaluate_assignment_rules(&document, &selected, Some(&token)));
        std::thread::sleep(std::time::Duration::from_millis(1));
        token.cancel();
        operation.join()
    }).map_err(|_| "evaluation thread panicked")?;
    assert_eq!(result, Err(AssignmentRuleError::Cancelled));
    Ok(())
}

#[test]
fn repeated_instant_availability_expansion_has_an_explicit_operation_limit() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(&mut value, 30, "unavailable", "2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z");
    for index in 1000..1200 {
        availability(&mut value, index, "unavailable", "2026-11-01T08:00:00Z", "2026-11-01T10:00:00Z");
    }
    let mut selected = Vec::new();
    for index in 2000..2200 {
        another_shift(&mut value, index, "2026-11-01T08:00:00", "2026-11-01T10:00:00");
        selected.push(pair(1, index)?);
    }
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert_eq!(evaluate_assignment_rules(&document, &selected, None),
        Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::ExpandedIntervals)));
    Ok(())
}
