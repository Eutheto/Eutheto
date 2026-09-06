mod support;

use eutheto_domain_ir::{RuleEvaluation, VerificationFactId, VerificationValue};
use eutheto_types::{CancellationToken, RuleId, ScenarioDocument};
use eutheto_workforce::{
    assignment_rules::{
        AssignmentRuleError, AssignmentRuleEvaluation, AssignmentRuleLimit, SelectionIssueKind,
        evaluate_assignment_rules,
    },
    model::AssignmentPair,
};
use serde_json::{Value, json};
use std::error::Error;
use support::id;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn base() -> Result<Value> {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(6));
    value["domain"]["lockedAssignments"] = json!({});
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    times(
        &mut value["domain"]["entities"][id(8)],
        "2026-11-01T08:00:00",
        "2026-11-01T10:00:00",
    );
    Ok(value)
}

fn times(shift: &mut Value, start: &str, end: &str) {
    shift["startsAt"] = json!({"instant":format!("{start}Z"),"local":start,"offsetSeconds":0});
    shift["endsAt"] = json!({"instant":format!("{end}Z"),"local":end,"offsetSeconds":0});
}

fn rule(value: &mut Value, index: u32, kind: &str) {
    value["domain"]["rules"][id(index)] = json!({"id":id(index),"kind":kind,"active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
    if kind == "noOverlap" {
        value["domain"]["rules"][id(index)]["compatibleCategoryPairs"] = json!([]);
    }
}

fn rest_rule(value: &mut Value, index: u32, minimum_minutes: u32) {
    rule(value, index, "minimumRest");
    value["domain"]["rules"][id(index)]["afterScope"] = json!({"people":{"kind":"all"}});
    value["domain"]["rules"][id(index)]["beforeScope"] = json!({"people":{"kind":"all"}});
    value["domain"]["rules"][id(index)]["minimumMinutes"] = json!(minimum_minutes);
}

fn rest_evidence(
    result: &AssignmentRuleEvaluation,
    index: u32,
    source: u32,
    target: u32,
    minimum_minutes: u32,
    seconds: i64,
    nanos: i64,
) -> Result {
    let record = evaluation(result, index)?;
    assert_eq!(
        record.observed.get(&VerificationFactId::new(
            "official.workforce.fact.witness_reason"
        )?),
        Some(&VerificationValue::Text("minimum_rest".to_owned())),
    );
    for (key, shift) in [
        ("official.workforce.fact.rest_source_shift", source),
        ("official.workforce.fact.rest_target_shift", target),
    ] {
        let Some(VerificationValue::Entity(entity)) =
            record.observed.get(&VerificationFactId::new(key)?)
        else {
            return Err("missing typed rest role".into());
        };
        assert_eq!(entity.kind.as_str(), "official.workforce.shift");
        assert_eq!(
            entity.id.as_str(),
            format!("official.workforce.{}", id(shift))
        );
        assert!(record.affected_entities.contains(entity));
    }
    assert_eq!(
        record.expected.get(&VerificationFactId::new(
            "official.workforce.fact.required_rest_minutes"
        )?),
        Some(&VerificationValue::Integer(i64::from(minimum_minutes))),
    );
    for (key, value) in [
        ("official.workforce.fact.actual_rest_seconds", seconds),
        (
            "official.workforce.fact.actual_rest_subsecond_nanoseconds",
            nanos,
        ),
    ] {
        assert_eq!(
            record.observed.get(&VerificationFactId::new(key)?),
            Some(&VerificationValue::Integer(value)),
        );
    }
    assert_eq!(record.affected_entities.len(), 3);
    assert_eq!(record.expected.len(), 2);
    assert_eq!(record.observed.len(), 7);
    assert!(record.evidence.is_empty());
    Ok(())
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

fn run(value: &Value, pairs: &[(u32, u32)]) -> Result<AssignmentRuleEvaluation> {
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let selected = pairs
        .iter()
        .map(|(person, shift)| pair(*person, *shift))
        .collect::<Result<Vec<_>>>()?;
    Ok(evaluate_assignment_rules(&document, &selected, None)?)
}

fn evaluation(result: &AssignmentRuleEvaluation, index: u32) -> Result<&RuleEvaluation> {
    let rule_id: RuleId = id(index).parse()?;
    result
        .evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == rule_id)
        .ok_or_else(|| "missing evaluation".into())
}

fn totals(result: &AssignmentRuleEvaluation, index: u32) -> Result<(i64, i64)> {
    let record = evaluation(result, index)?;
    record.validate()?;
    let checked = record
        .observed
        .get(&VerificationFactId::new(
            "official.workforce.fact.checked_predicate_count",
        )?)
        .ok_or("checked")?;
    let violations = record
        .observed
        .get(&VerificationFactId::new(
            "official.workforce.fact.violation_count",
        )?)
        .ok_or("violations")?;
    let (VerificationValue::Integer(checked), VerificationValue::Integer(violations)) =
        (checked, violations)
    else {
        return Err("non-integer totals".into());
    };
    assert_eq!(record.satisfied, *violations == 0);
    assert_eq!(
        record.expected.get(&VerificationFactId::new(
            "official.workforce.fact.violation_count"
        )?),
        Some(&VerificationValue::Integer(0))
    );
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
    if let Some(person) = person.as_object_mut() {
        person.remove("externalId");
    }
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
    for (index, kind) in [(20, "eligibility"), (21, "availability"), (22, "noOverlap")] {
        rule(&mut value, index, kind);
    }
    let empty = run(&value, &[])?;
    for index in [1, 20, 21, 22] {
        assert_eq!(totals(&empty, index)?, (0, 0));
    }
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([]);
    let active = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&active, 20)?, (2, 2));
    value["domain"]["rules"][id(20)]["scope"]["people"] =
        json!({"kind":"filter","allTags":["absent"],"anyTags":[]});
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
    for (field, replacement) in [
        ("teamIds", json!([id(31)])),
        ("categories", json!(["other"])),
        ("weekdays", json!(["monday"])),
    ] {
        value["domain"]["rules"][id(20)]["scope"] = full_scope.clone();
        value["domain"]["rules"][id(20)]["scope"][field] = replacement;
        assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (0, 0));
    }
    value["domain"]["rules"][id(20)]["scope"] = full_scope;
    value["domain"]["entities"][id(4)]["locationBehavior"] = json!({"kind":"optional"});
    value["domain"]["entities"][id(8)]
        .as_object_mut()
        .ok_or("shift")?
        .remove("locationId");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (0, 0));
    Ok(())
}

#[test]
fn malformed_selections_are_rejected_even_without_authored_rules() -> Result {
    let document: ScenarioDocument = serde_json::from_value(base()?)?;
    for (selected, expected) in [
        (
            vec![pair(1, 8)?, pair(1, 8)?],
            SelectionIssueKind::DuplicatePair,
        ),
        (vec![pair(999, 8)?], SelectionIssueKind::MissingPerson),
        (vec![pair(4, 8)?], SelectionIssueKind::WrongKindPerson),
        (vec![pair(1, 999)?], SelectionIssueKind::UnresolvedShift),
    ] {
        assert!(
            matches!(evaluate_assignment_rules(&document, &selected, None), Err(AssignmentRuleError::InvalidSelection { kind, .. }) if kind == expected)
        );
    }
    Ok(())
}

#[test]
fn dormant_excluded_occurrence_is_not_a_resolved_selection() -> Result {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["domain"]["lockedAssignments"] = json!({});
    value["domain"]["entities"][id(6)]["recurrence"]["excludedDates"] = json!(["2026-11-01"]);
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert!(matches!(
        evaluate_assignment_rules(&document, &[pair(1, 7)?], None),
        Err(AssignmentRuleError::InvalidSelection {
            kind: SelectionIssueKind::UnresolvedShift,
            ..
        })
    ));
    Ok(())
}

#[test]
fn unconditional_activity_and_approved_leave_survive_rule_toggles() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(1)]["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    availability(
        &mut value,
        30,
        "approvedTimeOff",
        "2026-11-01T09:00:00Z",
        "2026-11-01T09:30:00Z",
    );
    availability(
        &mut value,
        31,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
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
    value["domain"]["entities"][id(8)]
        .as_object_mut()
        .ok_or("shift")?
        .remove("locationId");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 30)?, (0, 0));
    Ok(())
}

#[test]
fn activity_covers_the_entire_post_horizon_shift_tail() -> Result {
    let mut value = base()?;
    value["domain"]["entities"][id(1)]["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-11-01","endDateExclusive":"2026-11-02"});
    times(
        &mut value["domain"]["entities"][id(8)],
        "2026-11-01T23:00:00",
        "2026-11-02T00:00:00",
    );
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 1)?, (1, 0));
    times(
        &mut value["domain"]["entities"][id(8)],
        "2026-11-01T23:00:00",
        "2026-11-02T00:00:00.000000001",
    );
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
    value["domain"]["entities"][id(1)]["qualificationGrants"][0]["effectiveFrom"] =
        json!("2026-11-01T09:00:00.000000001Z");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    value["domain"]["entities"][id(1)]["qualificationGrants"] =
        json!([{ "qualificationId":id(11),"expiresAt":"2026-11-01T08:00:00Z" }]);
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    Ok(())
}

#[test]
fn different_partial_qualifications_cannot_alternate_for_any_expression() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "eligibility");
    value["domain"]["entities"][id(12)] =
        json!({"kind":"qualification","id":id(12),"name":"Second","description":""});
    value["domain"]["entities"][id(4)]["qualifications"] =
        json!({"kind":"matches","allQualificationIds":[],"anyQualificationIds":[id(11),id(12)]});
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([
        {"qualificationId":id(11),"expiresAt":"2026-11-01T09:00:00Z"},
        {"qualificationId":id(12),"effectiveFrom":"2026-11-01T09:00:00Z"}
    ]);
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    value["domain"]["entities"][id(1)]["qualificationGrants"][1]
        .as_object_mut()
        .ok_or("grant")?
        .remove("effectiveFrom");
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 0));
    value["domain"]["entities"][id(4)]["qualifications"] =
        json!({"kind":"matches","allQualificationIds":[id(11),id(12)],"anyQualificationIds":[]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    Ok(())
}

#[test]
fn distinct_available_only_records_are_conjunctive_and_windows_union() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(
        &mut value,
        30,
        "availableOnly",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    value["domain"]["entities"][id(30)]["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["sunday"],"startTime":"09:00:00","endTime":"10:00:00","endDayOffset":0},
        {"weekdays":["sunday"],"startTime":"08:00:00","endTime":"09:00:00","endDayOffset":0}
    ]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 0));
    availability(
        &mut value,
        31,
        "availableOnly",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    let failed = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&failed, 20)?, (2, 1));
    let encoded = serde_json::to_string(evaluation(&failed, 20)?)?;
    assert!(!encoded.contains("private"));
    value["domain"]["entities"][id(31)]["effectiveRange"] =
        json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 0));
    Ok(())
}

#[test]
fn effective_range_clips_pre_effective_overnight_windows_and_full_shift_tails() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    times(
        &mut value["domain"]["entities"][id(8)],
        "2026-11-01T23:30:00",
        "2026-11-02T02:00:00",
    );
    availability(
        &mut value,
        30,
        "availableOnly",
        "2026-11-02T00:00:00Z",
        "2026-11-02T02:00:00Z",
    );
    value["domain"]["entities"][id(30)]["effectiveRange"] =
        json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 0));
    value["domain"]["entities"][id(30)]["availabilityKind"] = json!("unavailable");
    value["domain"]["entities"][id(30)]["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["sunday"],"startTime":"23:00:00","endTime":"01:00:00","endDayOffset":1}
    ]});
    let result = run(&value, &[(1, 8)])?;
    assert_eq!(totals(&result, 20)?, (1, 1));
    assert_eq!(
        evaluation(&result, 20)?
            .observed
            .get(&VerificationFactId::new(
                "official.workforce.fact.witness_start"
            )?),
        Some(&VerificationValue::Text("2026-11-02T00:00:00Z".to_owned()))
    );
    Ok(())
}

#[test]
fn unavailable_touching_endpoints_does_not_overlap() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(
        &mut value,
        30,
        "unavailable",
        "2026-11-01T06:00:00Z",
        "2026-11-01T08:00:00Z",
    );
    availability(
        &mut value,
        31,
        "unavailable",
        "2026-11-01T10:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 0));
    value["domain"]["entities"][id(30)]["timeWindow"]["endsAt"] =
        json!("2026-11-01T08:00:00.000000001Z");
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
    value["domain"]["rules"][id(20)]["scope"]["people"] =
        json!({"kind":"filter","allTags":["night"],"anyTags":[]});
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
    value["domain"]["entities"][id(12)] =
        json!({"kind":"qualification","id":id(12),"name":"Second","description":""});
    value["domain"]["entities"][id(1)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    let minimum = json!({"qualifications":{"allQualificationIds":[id(11),id(12)],"anyQualificationIds":[]},"minimum":1});
    let mut reversed = minimum.clone();
    reversed["qualifications"]["allQualificationIds"] = json!([id(12), id(11)]);
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] =
        json!([minimum, reversed]);
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
    value["domain"]["entities"][id(12)] =
        json!({"kind":"qualification","id":id(12),"name":"Second","description":""});
    value["domain"]["entities"][id(1)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] = json!([
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":1},
        {"qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},"minimum":1}
    ]);
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (3, 0));
    value["domain"]["entities"][id(1)]["qualificationGrants"][1]["expiresAt"] =
        json!("2026-11-01T09:00:00Z");
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
    value["domain"]["entities"][id(8)]["coverage"] =
        json!({"kind":"exact","count":0,"qualificationMinimums":[]});
    assert_eq!(totals(&run(&value, &[])?, 20)?, (1, 0));
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 1));
    value["domain"]["entities"][id(8)]["coverage"] =
        json!({"kind":"atLeast","minimum":2,"qualificationMinimums":[]});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 1));
    Ok(())
}

#[test]
fn coverage_requirement_dates_use_start_date_not_reporting_date() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "coverage");
    times(
        &mut value["domain"]["entities"][id(8)],
        "2026-11-01T23:00:00",
        "2026-11-02T02:00:00",
    );
    value["domain"]["entities"][id(8)]["reportingAttribution"] = json!("endLocalDate");
    value["domain"]["rules"][id(20)]["scope"]["weekdays"] = json!(["monday"]);
    value["domain"]["entities"][id(30)] = json!({"kind":"coverageRequirement","id":id(30),"active":true,"scope":{"kind":"filter","startDateRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"}},"coverage":{"kind":"exact","count":2,"qualificationMinimums":[]}});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (2, 1));
    value["domain"]["entities"][id(30)]["scope"]["startDateRange"] =
        json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (1, 0));
    Ok(())
}

#[test]
fn overlap_checks_both_scopes_and_half_open_boundaries() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    another_shift(&mut value, 30, "2026-11-01T10:00:00", "2026-11-01T12:00:00");
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (1, 0));
    times(
        &mut value["domain"]["entities"][id(30)],
        "2026-11-01T09:59:59.999999999",
        "2026-11-01T12:00:00",
    );
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
    value["domain"]["rules"][id(20)]["compatibleCategoryPairs"] =
        json!([{"firstCategory":"alpha","secondCategory":"clinic"}]);
    let result = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&result, 20)?, (1, 0));
    assert_eq!(totals(&result, 21)?, (1, 1));
    times(
        &mut value["domain"]["entities"][id(30)],
        "2026-11-01T07:00:00",
        "2026-11-01T09:00:00",
    );
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
    availability(
        &mut value,
        30,
        "approvedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    availability(
        &mut value,
        31,
        "requestedTimeOff",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    );
    for (index, state) in [(40, "hard"), (41, "soft"), (42, "unlocked")] {
        another_shift(
            &mut value,
            index + 100,
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        value["domain"]["lockedAssignments"][id(index)] = json!({"id":id(index),"personId":id(1),"shiftId":id(index + 100),"state":{"kind":state}});
        if state == "soft" {
            value["domain"]["lockedAssignments"][id(index)]["state"]["stabilityWeight"] = json!(1);
        }
    }
    let result = run(&value, &[])?;
    assert_eq!(
        result.obligations.handled,
        [1, 20, 30]
            .map(|index| id(index).parse())
            .into_iter()
            .collect::<std::result::Result<Vec<RuleId>, _>>()?
    );
    assert_eq!(
        result.obligations.remaining,
        [21, 40]
            .map(|index| id(index).parse())
            .into_iter()
            .collect::<std::result::Result<Vec<RuleId>, _>>()?
    );
    Ok(())
}

#[test]
fn one_large_rule_has_exact_totals_and_a_bounded_deterministic_witness() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "eligibility");
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([]);
    let mut pairs = vec![(1, 8)];
    for index in 1000..1320 {
        another_person(&mut value, index);
        pairs.push((index, 8));
    }
    let forward = run(&value, &pairs)?;
    assert_eq!(totals(&forward, 20)?, (642, 642));
    let rule_id: RuleId = id(20).parse()?;
    assert_eq!(
        forward
            .evaluations
            .iter()
            .filter(|record| record.rule_id == rule_id)
            .count(),
        1
    );
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
    availability(
        &mut value,
        30,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    value["domain"]["entities"][id(30)]["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["sunday"],"startTime":"09:00:00","endTime":"09:30:00","endDayOffset":0},
        {"weekdays":["sunday"],"startTime":"08:00:00","endTime":"08:30:00","endDayOffset":0}
    ]});
    let first = run(&value, &[(1, 8)])?;
    value["domain"]["entities"][id(30)]["timeWindow"]["windows"]
        .as_array_mut()
        .ok_or("windows")?
        .reverse();
    assert_eq!(first.evaluations, run(&value, &[(1, 8)])?.evaluations);
    rule(&mut value, 21, "coverage");
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] = json!([
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":3},
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":2}
    ]);
    let first = run(&value, &[(1, 8)])?;
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"]
        .as_array_mut()
        .ok_or("minima")?
        .reverse();
    assert_eq!(first.evaluations, run(&value, &[(1, 8)])?.evaluations);
    Ok(())
}

#[test]
fn nonoverlap_bulk_count_exceeds_record_limits_without_emitting_pair_records() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "noOverlap");
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(8));
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
    assert_eq!(
        evaluate_assignment_rules(&malformed, &selected, None),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::SelectedPairs
        ))
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        evaluate_assignment_rules(&malformed, &selected, Some(&cancellation)),
        Err(AssignmentRuleError::Cancelled)
    );
    let document: ScenarioDocument = serde_json::from_value(base()?)?;
    let cancellation = CancellationToken::new();
    assert_eq!(
        evaluate_assignment_rules(&document, &[pair(1, 8)?], Some(&cancellation))?.evaluations,
        evaluate_assignment_rules(&document, &[pair(1, 8)?], None)?.evaluations
    );
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
        another_shift(
            &mut value,
            index,
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        selected.push(pair(1, index)?);
    }
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert_eq!(
        evaluate_assignment_rules(&document, &selected, None),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::WorkSteps
        ))
    );
    Ok(())
}

#[test]
fn repeated_instant_availability_expansion_has_an_explicit_operation_limit() -> Result {
    let mut value = base()?;
    rule(&mut value, 20, "availability");
    availability(
        &mut value,
        30,
        "unavailable",
        "2026-11-01T08:00:00Z",
        "2026-11-01T10:00:00Z",
    );
    for index in 1000..1200 {
        availability(
            &mut value,
            index,
            "unavailable",
            "2026-11-01T08:00:00Z",
            "2026-11-01T10:00:00Z",
        );
    }
    let mut selected = Vec::new();
    for index in 2000..2200 {
        another_shift(
            &mut value,
            index,
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        selected.push(pair(1, index)?);
    }
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert_eq!(
        evaluate_assignment_rules(&document, &selected, None),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::ExpandedIntervals
        ))
    );
    Ok(())
}

#[test]
fn rest_uses_exact_elapsed_threshold_and_directional_evidence() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 600);
    another_shift(&mut value, 30, "2026-11-01T20:00:00", "2026-11-01T21:00:00");
    for (start, violations) in [
        ("2026-11-01T20:00:00", 0),
        ("2026-11-01T19:59:59.999999999", 1),
        ("2026-11-01T20:00:00.000000001", 0),
    ] {
        times(
            &mut value["domain"]["entities"][id(30)],
            start,
            "2026-11-01T21:00:00",
        );
        let result = run(&value, &[(1, 30), (1, 8)])?;
        assert_eq!(totals(&result, 20)?, (1, violations));
        assert_eq!(
            result.evaluations,
            run(&value, &[(1, 8), (1, 30)])?.evaluations
        );
        if violations != 0 {
            rest_evidence(&result, 20, 8, 30, 600, 35_999, 999_999_999)?;
        }
    }
    Ok(())
}

#[test]
fn rest_role_chronology_is_not_uuid_or_selection_order() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 600);
    value["domain"]["entities"][id(31)] = value["domain"]["entities"][id(4)].clone();
    value["domain"]["entities"][id(31)]["id"] = json!(id(31));
    value["domain"]["entities"][id(31)]["category"] = json!("call");
    another_shift(&mut value, 30, "2026-11-01T06:00:00", "2026-11-01T07:00:00");
    value["domain"]["entities"][id(30)]["assignmentTypeId"] = json!(id(31));
    value["domain"]["rules"][id(20)]["afterScope"]["categories"] = json!(["call"]);
    value["domain"]["rules"][id(20)]["beforeScope"]["categories"] = json!(["clinic"]);
    let forward = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&forward, 20)?, (1, 1));
    rest_evidence(&forward, 20, 30, 8, 600, 3600, 0)?;
    assert_eq!(
        forward.evaluations,
        run(&value, &[(1, 30), (1, 8)])?.evaluations
    );
    value["domain"]["rules"][id(20)]["afterScope"]["categories"] = json!(["clinic"]);
    value["domain"]["rules"][id(20)]["beforeScope"]["categories"] = json!(["call"]);
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (0, 0));

    times(
        &mut value["domain"]["entities"][id(30)],
        "2026-11-01T08:00:00",
        "2026-11-01T09:00:00",
    );
    let equal = run(&value, &[(1, 30), (1, 8)])?;
    assert_eq!(totals(&equal, 20)?, (1, 1));
    rest_evidence(&equal, 20, 8, 30, 600, -7200, 0)?;
    value["domain"]["rules"][id(20)]["afterScope"]["categories"] = json!(["call"]);
    value["domain"]["rules"][id(20)]["beforeScope"]["categories"] = json!(["clinic"]);
    let reversed = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&reversed, 20)?, (1, 1));
    rest_evidence(&reversed, 20, 30, 8, 600, -3600, 0)?;
    rest_rule(&mut value, 20, 600);
    let both = run(&value, &[(1, 30), (1, 8)])?;
    assert_eq!(totals(&both, 20)?, (2, 2));
    rest_evidence(&both, 20, 8, 30, 600, -7200, 0)?;
    assert_eq!(totals(&run(&value, &[(1, 8)])?, 20)?, (0, 0));
    Ok(())
}

#[test]
fn rest_overlap_violates_zero_even_when_no_overlap_is_compatible() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 0);
    rule(&mut value, 21, "noOverlap");
    value["domain"]["rules"][id(21)]["compatibleCategoryPairs"] =
        json!([{"firstCategory":"clinic","secondCategory":"clinic"}]);
    another_shift(
        &mut value,
        30,
        "2026-11-01T09:59:59.999999999",
        "2026-11-01T11:00:00",
    );
    let result = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&result, 20)?, (1, 1));
    assert_eq!(totals(&result, 21)?, (1, 0));
    rest_evidence(&result, 20, 8, 30, 0, 0, -1)?;
    times(
        &mut value["domain"]["entities"][id(30)],
        "2026-11-01T10:00:00",
        "2026-11-01T11:00:00",
    );
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (1, 0));
    Ok(())
}

#[test]
fn rest_counts_nonadjacent_pairs_without_an_intervening_reset() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 120);
    times(
        &mut value["domain"]["entities"][id(8)],
        "2026-11-01T08:00:00",
        "2026-11-01T18:00:00",
    );
    another_shift(&mut value, 30, "2026-11-01T09:00:00", "2026-11-01T10:00:00");
    another_shift(&mut value, 31, "2026-11-01T19:00:00", "2026-11-01T20:00:00");
    let result = run(&value, &[(1, 30), (1, 31), (1, 8)])?;
    assert_eq!(totals(&result, 20)?, (3, 2));
    value["domain"]["entities"][id(32)] = value["domain"]["entities"][id(4)].clone();
    value["domain"]["entities"][id(32)]["id"] = json!(id(32));
    value["domain"]["entities"][id(32)]["category"] = json!("other");
    value["domain"]["entities"][id(30)]["assignmentTypeId"] = json!(id(32));
    value["domain"]["rules"][id(20)]["afterScope"]["categories"] = json!(["clinic"]);
    value["domain"]["rules"][id(20)]["beforeScope"]["categories"] = json!(["clinic"]);
    let scoped = run(&value, &[(1, 8), (1, 30), (1, 31)])?;
    assert_eq!(totals(&scoped, 20)?, (1, 1));
    rest_evidence(&scoped, 20, 8, 31, 120, 3600, 0)?;
    Ok(())
}

#[test]
fn rest_intersects_every_common_and_directional_scope_dimension() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 600);
    another_shift(&mut value, 40, "2026-11-01T11:00:00", "2026-11-01T12:00:00");
    another_person(&mut value, 32);
    value["domain"]["entities"][id(30)] = json!({"kind":"team","id":id(30),"name":"Team"});
    value["domain"]["entities"][id(31)] = json!({"kind":"team","id":id(31),"name":"Other"});
    value["domain"]["entities"][id(1)]["teamIds"] = json!([id(30)]);
    value["domain"]["entities"][id(34)] = value["domain"]["entities"][id(4)].clone();
    value["domain"]["entities"][id(34)]["id"] = json!(id(34));
    value["domain"]["entities"][id(35)] =
        json!({"kind":"location","id":id(35),"name":"Other","transitions":[]});
    let full_scope = json!({"people":{"kind":"selected","personIds":[id(1)]},"teamIds":[id(30)],"assignmentTypeIds":[id(4)],"categories":["clinic"],"weekdays":["sunday"],"locationIds":[id(5)]});
    for role in ["scope", "afterScope", "beforeScope"] {
        value["domain"]["rules"][id(20)][role] = full_scope.clone();
    }
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 40)])?, 20)?, (1, 1));
    for role in ["scope", "afterScope", "beforeScope"] {
        for (field, replacement) in [
            ("people", json!({"kind":"selected","personIds":[id(32)]})),
            (
                "people",
                json!({"kind":"filter","allTags":["absent"],"anyTags":[]}),
            ),
            (
                "people",
                json!({"kind":"filter","allTags":[],"anyTags":["absent"]}),
            ),
            ("teamIds", json!([id(31)])),
            ("assignmentTypeIds", json!([id(34)])),
            ("categories", json!(["other"])),
            ("weekdays", json!(["monday"])),
            ("locationIds", json!([id(35)])),
        ] {
            value["domain"]["rules"][id(20)][role][field] = replacement;
            let result = run(&value, &[(1, 8), (1, 40)])?;
            assert_eq!(totals(&result, 20)?, (0, 0), "{role}.{field}");
            assert!(result.obligations.handled.contains(&id(20).parse()?));
            value["domain"]["rules"][id(20)][role] = full_scope.clone();
        }
    }
    // Common scope must match both ends, not merely the source.
    value["domain"]["entities"][id(40)]["assignmentTypeId"] = json!(id(34));
    for role in ["afterScope", "beforeScope"] {
        value["domain"]["rules"][id(20)][role] = json!({"people":{"kind":"all"}});
    }
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 40)])?, 20)?, (0, 0));
    value["domain"]["rules"][id(20)]["scope"] =
        json!({"people":{"kind":"all"},"assignmentTypeIds":[id(34)]});
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 40)])?, 20)?, (0, 0));
    value["domain"]["rules"][id(20)]["active"] = json!(false);
    value["domain"]["rules"][id(20)]["afterScope"]["people"] =
        json!({"kind":"selected","personIds":[]});
    let inactive = run(&value, &[(1, 8), (1, 40)])?;
    assert!(evaluation(&inactive, 20).is_err());
    assert!(!inactive.obligations.handled.contains(&id(20).parse()?));
    assert!(!inactive.obligations.remaining.contains(&id(20).parse()?));
    value["domain"]["rules"][id(20)]["active"] = json!(true);
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert!(matches!(
        evaluate_assignment_rules(&document, &[], None),
        Err(AssignmentRuleError::InvalidDocument(_))
    ));
    Ok(())
}

#[test]
fn rest_sees_identified_inadmissible_selections_and_empty_selections() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 600);
    rule(&mut value, 21, "eligibility");
    rule(&mut value, 22, "availability");
    another_shift(&mut value, 30, "2026-11-01T11:00:00", "2026-11-01T12:00:00");
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    availability(
        &mut value,
        31,
        "unavailable",
        "2026-11-01T11:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    availability(
        &mut value,
        32,
        "approvedTimeOff",
        "2026-11-01T11:00:00Z",
        "2026-11-01T12:00:00Z",
    );
    let result = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&result, 20)?, (1, 1));
    assert!(!evaluation(&result, 21)?.satisfied);
    assert!(!evaluation(&result, 22)?.satisfied);
    assert!(!evaluation(&result, 32)?.satisfied);
    let empty = run(&value, &[])?;
    assert_eq!(totals(&empty, 20)?, (0, 0));
    assert!(empty.obligations.handled.contains(&id(20).parse()?));
    assert!(!empty.obligations.remaining.contains(&id(20).parse()?));
    Ok(())
}

#[test]
fn rest_distinguishes_overnight_dst_elapsed_time_from_reporting_weekday() -> Result {
    for (
        date,
        next,
        source_start,
        source_end,
        target_start,
        target_end,
        horizon_start,
        horizon_end,
        offset,
        next_offset,
        violations,
        seconds,
    ) in [
        (
            "2026-03-07",
            "2026-03-08",
            "2026-03-08T04:00:00Z",
            "2026-03-08T06:00:00Z",
            "2026-03-08T15:00:00Z",
            "2026-03-08T16:00:00Z",
            "2026-03-07T05:00:00Z",
            "2026-03-09T04:00:00Z",
            -18000,
            -14400,
            1,
            32400,
        ),
        (
            "2026-10-31",
            "2026-11-01",
            "2026-11-01T03:00:00Z",
            "2026-11-01T05:00:00Z",
            "2026-11-01T16:00:00Z",
            "2026-11-01T17:00:00Z",
            "2026-10-31T04:00:00Z",
            "2026-11-02T05:00:00Z",
            -14400,
            -18000,
            0,
            39600,
        ),
    ] {
        let mut value = base()?;
        value["settings"]["timeZone"] = json!("America/New_York");
        value["settings"]["overlapPolicy"] = json!("earlier");
        value["settings"]["horizon"] = json!({"start":horizon_start,"end":horizon_end});
        rest_rule(&mut value, 20, 600);
        another_shift(&mut value, 30, "2026-11-01T11:00:00", "2026-11-01T12:00:00");
        for (shift, start, end, local_start, local_end, shift_offset) in [
            (
                8,
                source_start,
                source_end,
                format!("{date}T23:00:00"),
                format!("{next}T01:00:00"),
                offset,
            ),
            (
                30,
                target_start,
                target_end,
                format!("{next}T11:00:00"),
                format!("{next}T12:00:00"),
                next_offset,
            ),
        ] {
            value["domain"]["entities"][id(shift)]["startsAt"] =
                json!({"instant":start,"local":local_start,"offsetSeconds":shift_offset});
            value["domain"]["entities"][id(shift)]["endsAt"] =
                json!({"instant":end,"local":local_end,"offsetSeconds":shift_offset});
        }
        value["domain"]["entities"][id(8)]["reportingAttribution"] = json!("endLocalDate");
        for role in ["scope", "afterScope", "beforeScope"] {
            value["domain"]["rules"][id(20)][role]["weekdays"] = json!(["sunday"]);
        }
        let result = run(&value, &[(1, 8), (1, 30)])?;
        assert_eq!(totals(&result, 20)?, (1, violations));
        value["domain"]["rules"][id(20)]["minimumMinutes"] = json!(700);
        rest_evidence(
            &run(&value, &[(1, 8), (1, 30)])?,
            20,
            8,
            30,
            700,
            seconds,
            0,
        )?;
        value["domain"]["entities"][id(8)]["reportingAttribution"] = json!("startLocalDate");
        assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (0, 0));
    }
    Ok(())
}

#[test]
fn rest_maximum_minutes_and_extreme_instants_do_not_add_overflowing_endpoints() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, u32::MAX);
    value["settings"]["horizon"] =
        json!({"start":"-009000-01-01T00:00:00Z","end":"9001-01-01T00:00:00Z"});
    times(
        &mut value["domain"]["entities"][id(8)],
        "-009000-01-01T00:00:00",
        "-009000-01-02T00:00:00",
    );
    another_shift(&mut value, 30, "9000-12-31T20:00:00", "9000-12-31T21:00:00");
    assert_eq!(totals(&run(&value, &[(1, 8), (1, 30)])?, 20)?, (1, 0));
    // Near the timestamp ceiling, adding the required minutes to the source end would fail.
    times(
        &mut value["domain"]["entities"][id(8)],
        "9000-12-31T18:00:00",
        "9000-12-31T19:00:00",
    );
    let near_end = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&near_end, 20)?, (1, 1));
    rest_evidence(&near_end, 20, 8, 30, u32::MAX, 3600, 0)?;
    // A long overlapping interval must retain a negative gap whose total nanoseconds do not
    // fit i64. Whole seconds and signed subsecond nanoseconds remain exact.
    times(
        &mut value["domain"]["entities"][id(8)],
        "-009000-01-01T00:00:00",
        "9000-12-31T19:00:00.000000001",
    );
    times(
        &mut value["domain"]["entities"][id(30)],
        "-009000-01-02T00:00:00",
        "-009000-01-03T00:00:00",
    );
    let negative = run(&value, &[(1, 8), (1, 30)])?;
    assert_eq!(totals(&negative, 20)?, (1, 1));
    let seconds = "-009000-01-02T00:00:00Z"
        .parse::<jiff::Timestamp>()?
        .as_second()
        - "9000-12-31T19:00:00Z"
            .parse::<jiff::Timestamp>()?
            .as_second();
    rest_evidence(&negative, 20, 8, 30, u32::MAX, seconds, -1)?;
    Ok(())
}

#[test]
fn rest_safe_suffix_counts_quadratic_predicates_without_quadratic_work_or_output() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 1);
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(8));
    let prototype = base()?["domain"]["entities"][id(8)].clone();
    let mut people = vec![1];
    for person in 2000..2099 {
        another_person(&mut value, person);
        people.push(person);
    }
    let mut pairs = Vec::new();
    for index in 0..800_u32 {
        let mut shift = prototype.clone();
        let second = index * 90;
        let start = format!(
            "2026-11-01T{:02}:{:02}:{:02}",
            second / 3600,
            second / 60 % 60,
            second % 60
        );
        let end_second = second + 30;
        let end = format!(
            "2026-11-01T{:02}:{:02}:{:02}",
            end_second / 3600,
            end_second / 60 % 60,
            end_second % 60
        );
        shift["id"] = json!(id(1000 + index));
        times(&mut shift, &start, &end);
        value["domain"]["entities"][id(1000 + index)] = shift;
        for person in &people {
            pairs.push((*person, 1000 + index));
        }
    }
    let result = run(&value, &pairs)?;
    // Reuse a bounded original shift population across people: 80,000 selected pairs and
    // 31,960,000 chronological predicates, more than the entire work-step ceiling.
    assert_eq!(totals(&result, 20)?, (31_960_000, 0));
    assert_eq!(result.evaluations.len(), 101);
    Ok(())
}

#[test]
fn dense_equal_start_rest_counts_both_roles_with_one_canonical_witness() -> Result {
    let mut value = base()?;
    rest_rule(&mut value, 20, 0);
    let mut pairs = vec![(1, 8)];
    for index in 1000..1199 {
        another_shift(
            &mut value,
            index,
            "2026-11-01T08:00:00",
            "2026-11-01T10:00:00",
        );
        pairs.push((1, index));
    }
    let result = run(&value, &pairs)?;
    assert_eq!(totals(&result, 20)?, (39_800, 39_800));
    rest_evidence(&result, 20, 8, 1000, 0, -7200, 0)?;
    assert_eq!(result.evaluations.len(), 2);
    pairs.reverse();
    assert_eq!(result.evaluations, run(&value, &pairs)?.evaluations);
    Ok(())
}
