mod support;

use eutheto_domain_api::CompileContext;
use eutheto_planning_ir::{Constraint, PlanningIrLimitsV1};
use eutheto_types::{CancellationToken, OverlapPolicy, ScenarioDocument};
use eutheto_workforce::{
    assignment_rules::{
        AssignmentRuleCompilation, AssignmentRuleError, AssignmentRuleLimit, RejectionCause,
        analyze_assignments, compile_assignment_rules,
    },
    model::AssignmentPair,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
};
use support::{fixture, id};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn document() -> Result<ScenarioDocument> {
    let mut value = fixture()?;
    value.settings.overlap_policy = OverlapPolicy::Earlier;
    value.domain.locked_assignments.clear();
    Ok(value)
}

fn entity(document: &mut ScenarioDocument, index: u32) -> Result<&mut Value> {
    document
        .domain
        .entities
        .get_mut(&id(index).parse()?)
        .ok_or_else(|| "missing fixture entity".into())
}

fn rule(document: &mut ScenarioDocument, index: u32, kind: &str) -> Result {
    let mut value = json!({"kind":kind,"id":id(index),"active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
    if kind == "noOverlap" {
        value["compatibleCategoryPairs"] = json!([]);
    }
    document.domain.rules.insert(id(index).parse()?, value);
    Ok(())
}

fn context(limits: PlanningIrLimitsV1) -> CompileContext {
    CompileContext {
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        cancellation: CancellationToken::new(),
        planning_limits: limits,
    }
}

fn compile(document: &ScenarioDocument) -> Result<AssignmentRuleCompilation> {
    Ok(compile_assignment_rules(
        document,
        &context(PlanningIrLimitsV1::DEFAULT),
    )?)
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

// Observe the emitted Boolean mathematics; never consult the independent evaluator here.
fn allows(model: &AssignmentRuleCompilation, selected: &[AssignmentPair]) -> Result<bool> {
    if selected
        .iter()
        .any(|pair| !model.variables.iter().any(|entry| entry.pair == *pair))
    {
        return Ok(false);
    }
    let values: BTreeMap<_, _> = model
        .variables
        .iter()
        .map(|entry| (&entry.variable.id, selected.contains(&entry.pair)))
        .collect();
    for record in &model.constraints {
        if !record.enforcement.is_empty() {
            return Err("unexpected enforcement in four-rule contribution".into());
        }
        let (literals, min, max) = match &record.body {
            Constraint::BoolOr { literals } => (literals, 1, u64::MAX),
            Constraint::AtMostOne { literals } => (literals, 0, 1),
            Constraint::CardinalityRange { literals, min, max } => (literals, *min, *max),
            _ => return Err("unexpected primitive in four-rule contribution".into()),
        };
        let mut count = 0;
        for literal in literals {
            let value = values
                .get(&literal.variable)
                .ok_or("undeclared Boolean literal")?;
            count += u64::from(*value == literal.positive);
        }
        if count < min || count > max {
            return Ok(false);
        }
    }
    Ok(true)
}

fn availability(
    document: &mut ScenarioDocument,
    index: u32,
    kind: &str,
    start: &str,
    end: &str,
) -> Result {
    document.domain.entities.insert(id(index).parse()?, json!({"kind":"availability","id":id(index),"personId":id(1),"availabilityKind":kind,
        "timeWindow":{"kind":"instant","startsAt":start,"endsAt":end},
        "effectiveRange":{"startDate":"2026-01-01","endDateExclusive":"2027-01-01"},"source":"private source","note":"private note"}));
    Ok(())
}

#[test]
fn eligibility_activation_and_scope_own_both_predicates() -> Result {
    let mut value = document()?;
    entity(&mut value, 1)?["eligibleAssignmentTypeIds"] = json!([]);
    entity(&mut value, 1)?["qualificationGrants"] = json!([]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?])?);
    rule(&mut value, 20, "eligibility")?;
    let analysis = analyze_assignments(&value, None, PlanningIrLimitsV1::DEFAULT)?;
    assert!(analysis.candidates.is_empty());
    assert_eq!(analysis.estimate.after_activity_pruning, 2);
    assert_eq!(analysis.estimate.after_assignment_type_pruning, 0);
    assert_eq!(analysis.rejections.len(), 4);
    assert!(analysis.rejections.iter().any(|rejection| matches!(
        rejection.cause,
        RejectionCause::AssignmentTypeNotAllowed { .. }
    )));
    assert!(analysis.rejections.iter().any(|rejection| matches!(
        rejection.cause,
        RejectionCause::QualificationExpression { .. }
    )));
    value
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("missing rule")?["scope"]["people"] =
        json!({"kind":"filter","allTags":["absent"],"anyTags":[]});
    assert!(allows(&compile(&value)?, &[pair(1, 7)?])?);
    value
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("missing rule")?["active"] = json!(false);
    let result = compile(&value)?;
    assert!(!result.obligations.handled.contains(&id(20).parse()?));
    assert!(allows(&result, &[pair(1, 7)?])?);
    Ok(())
}

#[test]
fn adjacent_qualification_renewals_cover_but_nanosecond_gap_and_expiry_do_not() -> Result {
    let mut value = document()?;
    rule(&mut value, 20, "eligibility")?;
    entity(&mut value, 1)?["qualificationGrants"] = json!([
        {"qualificationId":id(11),"expiresAt":"2026-11-01T06:00:00Z"},
        {"qualificationId":id(11),"effectiveFrom":"2026-11-01T06:00:00Z","expiresAt":"2026-11-01T07:30:00Z"}]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    entity(&mut value, 1)?["qualificationGrants"][1]["effectiveFrom"] =
        json!("2026-11-01T06:00:00.000000001Z");
    assert!(compile(&value)?.variables.is_empty());
    entity(&mut value, 1)?["qualificationGrants"] =
        json!([{"qualificationId":id(11),"expiresAt":"2026-11-01T05:30:00Z"}]);
    assert!(compile(&value)?.variables.is_empty());
    Ok(())
}

#[test]
fn any_qualification_does_not_allow_alternating_partial_credentials() -> Result {
    let mut value = document()?;
    value.domain.entities.insert(
        id(12).parse()?,
        json!({"kind":"qualification","id":id(12),"name":"Other","description":""}),
    );
    entity(&mut value, 4)?["qualifications"] =
        json!({"kind":"matches","allQualificationIds":[],"anyQualificationIds":[id(11),id(12)]});
    entity(&mut value, 1)?["qualificationGrants"] = json!([
        {"qualificationId":id(11),"expiresAt":"2026-11-01T06:00:00Z"},
        {"qualificationId":id(12),"effectiveFrom":"2026-11-01T06:00:00Z"}]);
    rule(&mut value, 20, "eligibility")?;
    assert!(compile(&value)?.variables.is_empty());
    entity(&mut value, 1)?["qualificationGrants"][1] = json!({"qualificationId":id(12)});
    assert!(allows(&compile(&value)?, &[pair(1, 7)?])?);
    Ok(())
}

#[test]
fn activity_covers_the_complete_post_horizon_tail_without_authored_rules() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(8).parse()?);
    entity(&mut value, 6)?["timing"] =
        json!({"kind":"localWindow","startTime":"23:00:00","endTime":"02:00:00","endDayOffset":1});
    entity(&mut value, 1)?["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-01-01","endDateExclusive":"2026-11-02"});
    let analysis = analyze_assignments(&value, None, PlanningIrLimitsV1::DEFAULT)?;
    assert!(analysis.candidates.is_empty());
    assert!(
        matches!(analysis.rejections[0].cause, RejectionCause::OutsideActiveRange { outside, .. } if outside.start == "2026-11-02T05:00:00Z".parse()? && outside.end == "2026-11-02T07:00:00Z".parse()?)
    );
    assert_eq!(analysis.obligations.handled, vec![id(1).parse()?]);
    Ok(())
}

#[test]
fn ordinary_records_are_conjunctive_and_approved_leave_is_unconditional() -> Result {
    let mut value = document()?;
    availability(
        &mut value,
        30,
        "availableOnly",
        "2026-11-01T05:00:00Z",
        "2026-11-01T07:00:00Z",
    )?;
    availability(
        &mut value,
        31,
        "availableOnly",
        "2026-11-01T06:00:00Z",
        "2026-11-01T08:00:00Z",
    )?;
    assert!(allows(&compile(&value)?, &[pair(1, 7)?])?);
    rule(&mut value, 21, "availability")?;
    assert!(compile(&value)?.variables.is_empty());
    value.domain.rules.clear();
    availability(
        &mut value,
        32,
        "requestedTimeOff",
        "2026-11-01T05:30:00Z",
        "2026-11-01T06:30:00Z",
    )?;
    assert!(allows(&compile(&value)?, &[pair(1, 7)?])?);
    entity(&mut value, 32)?["availabilityKind"] = json!("approvedTimeOff");
    let result = compile(&value)?;
    assert!(result.variables.is_empty());
    assert!(result.obligations.handled.contains(&id(32).parse()?));
    let leave_id = id(32).parse()?;
    assert!(
        result
            .rejections
            .iter()
            .all(|rejection| rejection.binding_id == leave_id)
    );
    entity(&mut value, 32)?["locationIds"] = json!([id(5)]);
    entity(&mut value, 32)?["effectiveRange"] =
        json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    assert!(allows(&compile(&value)?, &[pair(1, 7)?])?);
    Ok(())
}

#[test]
fn weekly_window_union_includes_pre_effective_overnight_starts() -> Result {
    let mut value = document()?;
    rule(&mut value, 21, "availability")?;
    availability(
        &mut value,
        30,
        "availableOnly",
        "2026-11-01T05:00:00Z",
        "2026-11-01T08:00:00Z",
    )?;
    entity(&mut value, 30)?["effectiveRange"] =
        json!({"startDate":"2026-11-01","endDateExclusive":"2026-11-02"});
    entity(&mut value, 30)?["timeWindow"] = json!({"kind":"weekly","windows":[
        {"weekdays":["saturday"],"startTime":"23:00:00","endTime":"02:00:00","endDayOffset":1},
        {"weekdays":["sunday"],"startTime":"02:00:00","endTime":"03:00:00","endDayOffset":0}]});
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    entity(&mut value, 30)?["timeWindow"]["windows"][1]["startTime"] = json!("02:00:00.000000001");
    let result = compile(&value)?;
    assert!(allows(&result, &[pair(1, 7)?])?);
    assert!(!allows(&result, &[pair(1, 8)?])?);
    Ok(())
}

#[test]
fn all_source_owned_rejection_causes_survive_earlier_failures() -> Result {
    let mut value = document()?;
    rule(&mut value, 20, "eligibility")?;
    rule(&mut value, 21, "availability")?;
    entity(&mut value, 1)?["activeRange"] =
        json!({"kind":"dateRange","startDate":"2026-11-02","endDateExclusive":"2026-12-01"});
    entity(&mut value, 1)?["eligibleAssignmentTypeIds"] = json!([]);
    entity(&mut value, 1)?["qualificationGrants"] = json!([]);
    availability(
        &mut value,
        30,
        "unavailable",
        "2026-11-01T05:00:00Z",
        "2026-11-01T08:00:00Z",
    )?;
    availability(
        &mut value,
        31,
        "availableOnly",
        "2026-11-01T08:00:00Z",
        "2026-11-01T09:00:00Z",
    )?;
    availability(
        &mut value,
        32,
        "approvedTimeOff",
        "2026-11-01T05:00:00Z",
        "2026-11-01T08:00:00Z",
    )?;
    let result = compile(&value)?;
    assert!(result.variables.is_empty());
    assert_eq!(result.rejections.len(), 12);
    assert_eq!(result.estimate.rejection_facts, 12);
    assert!(result.provenance.is_empty());
    let mut counts = BTreeMap::new();
    for index in [1, 20, 21, 32] {
        let binding = id(index).parse()?;
        counts.insert(
            index,
            result
                .rejections
                .iter()
                .filter(|rejection| rejection.binding_id == binding)
                .count(),
        );
    }
    assert_eq!(counts, BTreeMap::from([(1, 2), (20, 4), (21, 4), (32, 2)]));
    Ok(())
}

#[test]
fn coverage_normalizes_empty_and_excess_populations_into_valid_primitives() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(8).parse()?);
    rule(&mut value, 22, "coverage")?;
    for (coverage, none, one, false_primitive) in [
        (
            json!({"kind":"exact","count":0,"qualificationMinimums":[]}),
            true,
            false,
            false,
        ),
        (
            json!({"kind":"exact","count":1,"qualificationMinimums":[]}),
            false,
            true,
            false,
        ),
        (
            json!({"kind":"exact","count":2,"qualificationMinimums":[]}),
            false,
            false,
            true,
        ),
        (
            json!({"kind":"atLeast","minimum":0,"maximumCount":9,"qualificationMinimums":[]}),
            true,
            true,
            false,
        ),
        (
            json!({"kind":"atLeast","minimum":1,"qualificationMinimums":[]}),
            false,
            true,
            false,
        ),
        (
            json!({"kind":"atLeast","minimum":2,"qualificationMinimums":[]}),
            false,
            false,
            true,
        ),
    ] {
        entity(&mut value, 6)?["coverage"] = coverage;
        let result = compile(&value)?;
        assert_eq!(allows(&result, &[])?, none);
        assert_eq!(allows(&result, &[pair(1, 7)?])?, one);
        assert_eq!(
            matches!(&result.constraints[0].body, Constraint::BoolOr { literals } if literals.is_empty()),
            false_primitive
        );
    }
    value
        .domain
        .rules
        .get_mut(&id(22).parse()?)
        .ok_or("missing rule")?["scope"]["people"] =
        json!({"kind":"filter","allTags":["absent"],"anyTags":[]});
    entity(&mut value, 6)?["coverage"] =
        json!({"kind":"exact","count":0,"qualificationMinimums":[]});
    assert!(allows(&compile(&value)?, &[])?);
    entity(&mut value, 6)?["coverage"]["count"] = json!(1);
    assert!(!allows(&compile(&value)?, &[])?);
    Ok(())
}

#[test]
fn qualification_minima_share_people_but_never_merge_distinct_owners() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(8).parse()?);
    rule(&mut value, 22, "coverage")?;
    value.domain.entities.insert(
        id(12).parse()?,
        json!({"kind":"qualification","id":id(12),"name":"Other","description":""}),
    );
    entity(&mut value, 1)?["qualificationGrants"] =
        json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    let minimum = |ids: Vec<String>| json!({"qualifications":{"allQualificationIds":ids,"anyQualificationIds":[]},"minimum":1});
    entity(&mut value, 6)?["coverage"] = json!({"kind":"exact","count":1,"qualificationMinimums":[minimum(vec![id(11)]),minimum(vec![id(12)])]});
    let result = compile(&value)?;
    assert!(allows(&result, &[pair(1, 7)?])?);
    assert_eq!(result.constraints.len(), 3);
    value.domain.entities.insert(id(40).parse()?, json!({"kind":"coverageRequirement","id":id(40),"active":true,"scope":{"kind":"all"},"coverage":{"kind":"exact","count":1,"qualificationMinimums":[minimum(vec![id(11)])]}}));
    let independent = compile(&value)?;
    assert_eq!(independent.constraints.len(), 5);
    assert!(allows(&independent, &[pair(1, 7)?])?);
    entity(&mut value, 1)?["qualificationGrants"] = json!([{"qualificationId":id(11)}]);
    let missing = compile(&value)?;
    assert!(!allows(&missing, &[pair(1, 7)?])?);
    assert!(
        missing
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.qualification_shortage")
    );
    Ok(())
}

#[test]
fn inert_preference_and_set_reordering_preserve_mathematics_and_provenance() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(8).parse()?);
    rule(&mut value, 22, "coverage")?;
    let minimum = json!({"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":1});
    entity(&mut value, 6)?["coverage"] = json!({"kind":"atLeast","minimum":1,"preferredCount":1,"maximumCount":4,"qualificationMinimums":[minimum.clone()]});
    let first = compile(&value)?;
    entity(&mut value, 6)?["coverage"]["preferredCount"] = json!(3);
    entity(&mut value, 6)?["coverage"]["qualificationMinimums"] = json!([minimum.clone(), minimum]);
    let second = compile(&value)?;
    assert_ne!(first.source_document_hash, second.source_document_hash);
    assert_eq!(first.variables, second.variables);
    assert_eq!(first.constraints, second.constraints);
    assert_eq!(first.provenance, second.provenance);
    assert_eq!(first.estimate, second.estimate);
    Ok(())
}

#[test]
fn overlap_compatibility_is_rule_owned_and_touching_is_not_overlap() -> Result {
    let mut value = document()?;
    rule(&mut value, 23, "noOverlap")?;
    assert!(!allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["compatibleCategoryPairs"] =
        json!([{"firstCategory":"clinic","secondCategory":"clinic"}]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    rule(&mut value, 24, "noOverlap")?;
    assert!(!allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    entity(&mut value, 8)?["startsAt"] = json!({"instant":"2026-11-01T06:30:00Z","local":"2026-11-01T01:30:00","offsetSeconds":-18000});
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    Ok(())
}

#[test]
fn coverage_intersects_reporting_weekdays_with_local_start_dates() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(8).parse()?);
    entity(&mut value, 6)?["timing"] =
        json!({"kind":"localWindow","startTime":"23:00:00","endTime":"02:00:00","endDayOffset":1});
    entity(&mut value, 6)?["reportingAttribution"] = json!("endLocalDate");
    entity(&mut value, 6)?["coverage"] =
        json!({"kind":"exact","count":0,"qualificationMinimums":[]});
    rule(&mut value, 22, "coverage")?;
    value
        .domain
        .rules
        .get_mut(&id(22).parse()?)
        .ok_or("missing rule")?["scope"]["weekdays"] = json!(["monday"]);
    value.domain.entities.insert(id(40).parse()?, json!({"kind":"coverageRequirement","id":id(40),"active":true,
        "scope":{"kind":"filter","startDateRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"}},
        "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]}}));
    let result = compile(&value)?;
    assert_eq!(result.constraints.len(), 2);
    assert!(!allows(&result, &[])?);
    assert!(!allows(&result, &[pair(1, 7)?])?);
    assert!(
        result
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.contradictory_coverage_bounds")
    );
    Ok(())
}

#[test]
fn hard_lock_readiness_and_partition_exclude_soft_and_unlocked_states() -> Result {
    let mut value = document()?;
    rule(&mut value, 23, "noOverlap")?;
    for (index, shift) in [(50, 7), (51, 8)] {
        value.domain.locked_assignments.insert(
            id(index).parse()?,
            json!({"id":id(index),"personId":id(1),"shiftId":id(shift),"state":{"kind":"hard"}}),
        );
    }
    let hard = compile(&value)?;
    assert_eq!(
        hard.obligations.remaining,
        vec![id(50).parse()?, id(51).parse()?]
    );
    assert!(
        hard.validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_lock_overlap")
    );
    value
        .domain
        .locked_assignments
        .get_mut(&id(50).parse()?)
        .ok_or("missing lock")?["state"] = json!({"kind":"soft","stabilityWeight":1});
    value
        .domain
        .locked_assignments
        .get_mut(&id(51).parse()?)
        .ok_or("missing lock")?["state"] = json!({"kind":"unlocked"});
    let nonhard = compile(&value)?;
    assert!(nonhard.obligations.remaining.is_empty());
    assert!(
        !nonhard
            .validation
            .issues
            .iter()
            .any(|issue| issue.code.starts_with("official.workforce.hard_lock"))
    );
    value
        .domain
        .locked_assignments
        .get_mut(&id(50).parse()?)
        .ok_or("missing lock")?["state"] = json!({"kind":"hard"});
    entity(&mut value, 6)?["recurrence"]["excludedDates"] = json!(["2026-11-01"]);
    let dormant = compile(&value)?;
    assert!(
        dormant
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_lock_unresolved_shift")
    );
    Ok(())
}

#[test]
fn rejected_hard_locks_and_excess_hard_coverage_are_readiness_only() -> Result {
    let mut value = document()?;
    rule(&mut value, 20, "eligibility")?;
    rule(&mut value, 22, "coverage")?;
    entity(&mut value, 6)?["coverage"] =
        json!({"kind":"exact","count":0,"qualificationMinimums":[]});
    entity(&mut value, 1)?["qualificationGrants"] = json!([]);
    value.domain.locked_assignments.insert(
        id(50).parse()?,
        json!({"id":id(50),"personId":id(1),"shiftId":id(7),"state":{"kind":"hard"}}),
    );
    let result = compile(&value)?;
    assert!(
        result
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_lock_rejected_pair")
    );
    assert!(
        result
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_locked_coverage_excess")
    );
    assert_eq!(result.obligations.remaining, vec![id(50).parse()?]);
    Ok(())
}

#[test]
fn provenance_is_reachable_source_owned_and_has_explicit_pair_association() -> Result {
    let mut value = document()?;
    rule(&mut value, 22, "coverage")?;
    rule(&mut value, 23, "noOverlap")?;
    let result = compile(&value)?;
    let mut reachable: BTreeSet<_> = result
        .variables
        .iter()
        .map(|entry| entry.variable.provenance.clone())
        .chain(
            result
                .constraints
                .iter()
                .map(|record| record.provenance.clone()),
        )
        .collect();
    for record in &result.provenance {
        if let Some(parent) = &record.parent {
            reachable.insert(parent.clone());
        }
    }
    assert_eq!(
        reachable,
        result
            .provenance
            .iter()
            .map(|record| record.id.clone())
            .collect()
    );
    assert!(
        result
            .variables
            .windows(2)
            .all(|pair| pair[0].variable.id < pair[1].variable.id)
    );
    assert_eq!(
        result
            .variables
            .iter()
            .map(|entry| entry.pair)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([pair(1, 7)?, pair(1, 8)?])
    );
    assert_eq!(result.estimate.variables, result.variables.len() as u64);
    assert_eq!(result.estimate.constraints, result.constraints.len() as u64);
    assert_eq!(
        result.estimate.provenance_records,
        result.provenance.len() as u64
    );
    let serialized = serde_json::to_string(&result.provenance)?;
    assert!(!serialized.contains("River"));
    assert!(!serialized.contains("Sunday Clinic"));
    Ok(())
}

#[test]
fn malformed_cancellation_and_tight_variable_constraint_limits_fail_atomically() -> Result {
    let mut value = document()?;
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_variables = 1;
    assert!(matches!(
        compile_assignment_rules(&value, &context(limits)),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Variables
        ))
    ));
    rule(&mut value, 22, "coverage")?;
    limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_constraints = 1;
    assert!(matches!(
        compile_assignment_rules(&value, &context(limits)),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Constraints
        ))
    ));
    let cancelled = context(PlanningIrLimitsV1::DEFAULT);
    cancelled.cancellation.cancel();
    assert_eq!(
        compile_assignment_rules(&value, &cancelled),
        Err(AssignmentRuleError::Cancelled)
    );
    assert_eq!(
        analyze_assignments(
            &value,
            Some(&cancelled.cancellation),
            PlanningIrLimitsV1::DEFAULT
        ),
        Err(AssignmentRuleError::Cancelled)
    );
    value
        .domain
        .rules
        .get_mut(&id(22).parse()?)
        .ok_or("missing rule")?["scope"]["people"] = json!({"kind":"selected","personIds":[]});
    assert!(matches!(
        compile_assignment_rules(&value, &context(PlanningIrLimitsV1::DEFAULT)),
        Err(AssignmentRuleError::InvalidDocument(_))
    ));
    Ok(())
}

#[test]
fn aggregate_byte_record_and_reference_boundaries_do_not_truncate_rejections() -> Result {
    let mut value = document()?;
    rule(&mut value, 21, "availability")?;
    for index in 30..38 {
        availability(
            &mut value,
            index,
            "unavailable",
            "2026-11-01T05:00:00Z",
            "2026-11-01T08:00:00Z",
        )?;
    }
    for field in 0..3 {
        let set = |cap| {
            let mut limits = PlanningIrLimitsV1::DEFAULT;
            match field {
                0 => limits.max_ir_bytes = cap,
                1 => limits.max_provenance_records = cap,
                _ => limits.max_total_refs = cap,
            }
            limits
        };
        let mut low = 0;
        let mut high = match field {
            0 => PlanningIrLimitsV1::DEFAULT.max_ir_bytes,
            1 => PlanningIrLimitsV1::DEFAULT.max_provenance_records,
            _ => PlanningIrLimitsV1::DEFAULT.max_total_refs,
        };
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if compile_assignment_rules(&value, &context(set(middle))).is_ok() {
                high = middle;
            } else {
                low = middle;
            }
        }
        let exact = compile_assignment_rules(&value, &context(set(high)))?;
        assert!(exact.variables.is_empty());
        assert_eq!(exact.rejections.len(), 16);
        assert!(matches!(
            compile_assignment_rules(&value, &context(set(high - 1))),
            Err(AssignmentRuleError::LimitExceeded(_))
        ));
    }
    Ok(())
}

#[test]
fn overlap_requires_both_shifts_to_match_every_filter() -> Result {
    let mut value = document()?;
    let mut other_type = entity(&mut value, 4)?.clone();
    other_type["id"] = json!(id(41));
    other_type["category"] = json!("other");
    value.domain.entities.insert(id(41).parse()?, other_type);
    entity(&mut value, 8)?["assignmentTypeId"] = json!(id(41));
    rule(&mut value, 23, "noOverlap")?;
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["scope"]["assignmentTypeIds"] = json!([id(4)]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["scope"]["assignmentTypeIds"] = json!([id(4), id(41)]);
    assert!(!allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["scope"]["categories"] = json!(["clinic"]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    Ok(())
}

#[test]
fn raw_cartesian_limit_precedes_candidate_allocation() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(6).parse()?);
    let person = entity(&mut value, 1)?.clone();
    let shift = entity(&mut value, 8)?.clone();
    for index in 0..1000 {
        let mut record = person.clone();
        record["id"] = json!(id(1000 + index));
        record["externalId"] = json!(format!("staff-{index}"));
        value
            .domain
            .entities
            .insert(id(1000 + index).parse()?, record);
    }
    for index in 0..999 {
        let mut record = shift.clone();
        record["id"] = json!(id(3000 + index));
        value
            .domain
            .entities
            .insert(id(3000 + index).parse()?, record);
    }
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_variables = 0;
    assert_eq!(
        analyze_assignments(&value, None, limits),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::InspectedPairs
        ))
    );
    Ok(())
}

#[test]
fn qualification_lower_bound_conflicts_with_an_independent_owner_upper_bound() -> Result {
    let mut value = document()?;
    value.domain.entities.remove(&id(8).parse()?);
    rule(&mut value, 22, "coverage")?;
    entity(&mut value, 6)?["coverage"] = json!({"kind":"atLeast","minimum":0,"qualificationMinimums":[
        {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":1}]});
    value.domain.entities.insert(
        id(40).parse()?,
        json!({"kind":"coverageRequirement","id":id(40),"active":true,"scope":{"kind":"all"},
        "coverage":{"kind":"exact","count":0,"qualificationMinimums":[]}}),
    );
    let result = compile(&value)?;
    assert!(!allows(&result, &[])?);
    assert!(!allows(&result, &[pair(1, 7)?])?);
    assert!(result.validation.issues.iter().any(|issue| issue.code
        == "official.workforce.qualification_above_headcount"
        && issue.message.contains(&id(6))
        && issue.message.contains(&id(40))));
    Ok(())
}

fn amplification_document() -> Result<ScenarioDocument> {
    let mut value = document()?;
    value.domain.entities.remove(&id(6).parse()?);
    let shift = entity(&mut value, 8)?.clone();
    // One large source expression is amplified across a valid 100-person/100-shift graph.
    for index in 0..99 {
        let mut record = shift.clone();
        record["id"] = json!(id(1000 + index));
        value
            .domain
            .entities
            .insert(id(1000 + index).parse()?, record);
    }
    let mut person = entity(&mut value, 1)?.clone();
    person["tags"] = json!([]);
    person["teamIds"] = json!([]);
    person["qualificationGrants"] = json!([]);
    person.as_object_mut().ok_or("person")?.remove("externalId");
    value.domain.entities.insert(id(1).parse()?, person.clone());
    for index in 200..299 {
        let mut record = person.clone();
        record["id"] = json!(id(index));
        value.domain.entities.insert(id(index).parse()?, record);
    }
    rule(&mut value, 20, "eligibility")?;
    Ok(value)
}

fn add_leaf_entities(value: &mut ScenarioDocument, kind: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for index in 0..3_000 {
        let identity = id(100_000 + index);
        let mut record = json!({"kind":kind,"id":identity,"name":"Reference"});
        if kind == "qualification" {
            record["description"] = json!("");
        }
        value.domain.entities.insert(identity.parse()?, record);
        ids.push(identity);
    }
    Ok(ids)
}

#[test]
fn empty_grants_cannot_bypass_work_limits_for_all_or_any_expression_leaves() -> Result {
    let mut value = amplification_document()?;
    entity(&mut value, 1)?["qualificationGrants"] = json!([]);
    let ids = add_leaf_entities(&mut value, "qualification")?;
    entity(&mut value, 4)?["qualifications"] = json!({
        "kind":"matches","allQualificationIds":ids,"anyQualificationIds":[],
    });
    // 100 × 100 × 3,000 leaves exceed the work ceiling with zero grants, while
    // source entity/node limits remain satisfied.
    assert_eq!(
        analyze_assignments(&value, None, PlanningIrLimitsV1::DEFAULT).err(),
        Some(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::WorkSteps
        ))
    );
    entity(&mut value, 4)?["qualifications"] = json!({
        "kind":"matches","allQualificationIds":[],"anyQualificationIds":ids,
    });
    assert_eq!(
        compile_assignment_rules(&value, &context(PlanningIrLimitsV1::DEFAULT)).err(),
        Some(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::WorkSteps
        ))
    );
    Ok(())
}

#[test]
fn empty_person_tags_and_teams_cannot_bypass_outer_filter_work_limits() -> Result {
    let mut value = amplification_document()?;
    entity(&mut value, 1)?["tags"] = json!([]);
    entity(&mut value, 1)?["teamIds"] = json!([]);
    let tags: Vec<_> = (0..3_000).map(|index| format!("tag-{index}")).collect();
    for field in ["allTags", "anyTags"] {
        let mut people = json!({"kind":"filter","allTags":[],"anyTags":[]});
        people[field] = json!(tags);
        value
            .domain
            .rules
            .get_mut(&id(20).parse()?)
            .ok_or("missing rule")?["scope"] = json!({"people":people});
        assert_eq!(
            analyze_assignments(&value, None, PlanningIrLimitsV1::DEFAULT).err(),
            Some(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::WorkSteps
            ))
        );
    }
    let teams = add_leaf_entities(&mut value, "team")?;
    value
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("missing rule")?["scope"] = json!({"people":{"kind":"all"},"teamIds":teams});
    assert_eq!(
        compile_assignment_rules(&value, &context(PlanningIrLimitsV1::DEFAULT)).err(),
        Some(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::WorkSteps
        ))
    );
    Ok(())
}

fn rest_rule(value: &mut ScenarioDocument, minutes: u32) -> Result {
    value.domain.rules.insert(
        id(24).parse()?,
        json!({
            "kind":"minimumRest","id":id(24),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"afterScope":{"people":{"kind":"all"}},
            "beforeScope":{"people":{"kind":"all"}},"minimumMinutes":minutes,
        }),
    );
    Ok(())
}

fn rest_binding(value: &mut ScenarioDocument) -> Result<&mut Value> {
    value
        .domain
        .rules
        .get_mut(&id(24).parse()?)
        .ok_or_else(|| "rest rule".into())
}

fn utc_times(shift: &mut Value, start: &str, end: &str) {
    shift["startsAt"] = json!({"instant":format!("{start}Z"),"local":start,"offsetSeconds":0});
    shift["endsAt"] = json!({"instant":format!("{end}Z"),"local":end,"offsetSeconds":0});
}

fn rest_document(minutes: u32) -> Result<ScenarioDocument> {
    let mut value = serde_json::to_value(document()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-03T00:00:00Z"});
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(6));
    let mut value: ScenarioDocument = serde_json::from_value(value)?;
    utc_times(
        entity(&mut value, 8)?,
        "2026-11-01T10:00:00",
        "2026-11-01T12:00:00",
    );
    let mut source = entity(&mut value, 8)?.clone();
    source["id"] = json!(id(7));
    utc_times(&mut source, "2026-11-01T00:00:00", "2026-11-01T01:00:00");
    value.domain.entities.insert(id(7).parse()?, source);
    rest_rule(&mut value, minutes)?;
    Ok(value)
}

#[test]
fn minimum_rest_exact_nanoseconds_zero_overlap_and_extreme_minutes() -> Result {
    for (start, minutes, permitted) in [
        ("2026-11-01T10:59:59.999999999", 600, false),
        ("2026-11-01T11:00:00", 600, true),
        ("2026-11-01T11:00:00.000000001", 600, true),
        ("2026-11-01T00:59:59.999999999", 0, false),
        ("2026-11-01T01:00:00", 0, true),
        ("2026-11-01T11:00:00", u32::MAX, false),
    ] {
        let mut value = rest_document(minutes)?;
        utc_times(entity(&mut value, 8)?, start, "2026-11-01T12:00:00");
        let result = compile(&value)?;
        assert_eq!(allows(&result, &[pair(1, 7)?, pair(1, 8)?])?, permitted);
        assert_eq!(result.estimate.constraints, u64::from(!permitted));
        assert_eq!(result.estimate.variables, 2);
        assert!(result.rejections.is_empty());
        assert!(result.obligations.handled.contains(&id(24).parse()?));
        assert!(!result.obligations.remaining.contains(&id(24).parse()?));
    }
    // Extreme valid dates retain exact comparisons. Adding the maximum required
    // duration to the late positive endpoint would exceed Jiff's timestamp range.
    for (date, next) in [("-009000-12-30", "-009000-12-31"), ("9000-12-30", "9000-12-31")] {
        let mut value = serde_json::to_value(rest_document(u32::MAX)?)?;
        value["settings"]["horizon"] = json!({
            "start":format!("{date}T00:00:00Z"),"end":format!("{next}T00:00:00Z"),
        });
        for (shift, start, end) in [(7, "00:00:00", "01:00:00"), (8, "10:00:00", "12:00:00")] {
            utc_times(
                &mut value["domain"]["entities"][id(shift)],
                &format!("{date}T{start}"),
                &format!("{date}T{end}"),
            );
        }
        assert!(!allows(
            &compile(&serde_json::from_value(value)?)?,
            &[pair(1, 7)?, pair(1, 8)?]
        )?);
    }
    Ok(())
}

#[test]
fn minimum_rest_direction_equal_starts_and_input_order_preserve_roles() -> Result {
    let mut value = rest_document(600)?;
    let mut target_type = entity(&mut value, 4)?.clone();
    target_type["id"] = json!(id(41));
    value.domain.entities.insert(id(41).parse()?, target_type);
    entity(&mut value, 8)?["assignmentTypeId"] = json!(id(41));
    rest_binding(&mut value)?["afterScope"]["assignmentTypeIds"] = json!([id(4)]);
    rest_binding(&mut value)?["beforeScope"]["assignmentTypeIds"] = json!([id(41)]);
    let baseline = compile(&value)?;
    assert!(!allows(&baseline, &[pair(1, 7)?, pair(1, 8)?])?);
    let mut reordered: Value = serde_json::to_value(&value)?;
    let entries = reordered["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?;
    let reverse: Vec<_> = entries
        .iter()
        .rev()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    entries.clear();
    entries.extend(reverse);
    let reordered = compile(&serde_json::from_value(reordered)?)?;
    assert_eq!(baseline.constraints, reordered.constraints);
    assert_eq!(baseline.provenance, reordered.provenance);
    rest_binding(&mut value)?["afterScope"]["assignmentTypeIds"] = json!([id(41)]);
    rest_binding(&mut value)?["beforeScope"]["assignmentTypeIds"] = json!([id(4)]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?])?);
    utc_times(
        entity(&mut value, 8)?,
        "2026-11-01T00:00:00",
        "2026-11-01T12:00:00",
    );
    // Higher-UUID source at the same start must still see its lower-UUID target.
    let reverse_role = compile(&value)?;
    assert_eq!(reverse_role.constraints.len(), 1);
    assert!(!allows(&reverse_role, &[pair(1, 7)?, pair(1, 8)?])?);
    rest_binding(&mut value)?["afterScope"]["assignmentTypeIds"] = json!([id(4)]);
    rest_binding(&mut value)?["beforeScope"]["assignmentTypeIds"] = json!([id(41)]);
    let forward_role = compile(&value)?;
    assert_eq!(forward_role.constraints.len(), 1);
    assert_ne!(
        reverse_role.constraints[0].id,
        forward_role.constraints[0].id
    );
    rest_binding(&mut value)?["afterScope"] = json!({"people":{"kind":"all"}});
    rest_binding(&mut value)?["beforeScope"] = json!({"people":{"kind":"all"}});
    let both = compile(&value)?;
    assert_eq!(both.constraints.len(), 2);
    assert_ne!(both.constraints[0].id, both.constraints[1].id);
    Ok(())
}

#[test]
fn minimum_rest_all_pairs_survive_an_intervening_unrelated_assignment() -> Result {
    let mut value = rest_document(600)?;
    let mut intervening = entity(&mut value, 8)?.clone();
    intervening["id"] = json!(id(30));
    utc_times(
        &mut intervening,
        "2026-11-01T02:00:00",
        "2026-11-01T03:00:00",
    );
    value.domain.entities.insert(id(30).parse()?, intervening);
    utc_times(
        entity(&mut value, 7)?,
        "2026-11-01T00:00:00",
        "2026-11-01T10:00:00",
    );
    utc_times(
        entity(&mut value, 8)?,
        "2026-11-01T18:00:00",
        "2026-11-01T19:00:00",
    );
    let result = compile(&value)?;
    assert!(!allows(&result, &[pair(1, 7)?, pair(1, 8)?])?);
    assert!(allows(&result, &[pair(1, 30)?, pair(1, 8)?])?);
    assert!(!allows(&result, &[pair(1, 7)?, pair(1, 30)?, pair(1, 8)?])?);
    assert_eq!(result.constraints.len(), 2);
    Ok(())
}

#[test]
fn minimum_rest_intersects_every_common_and_directional_scope_filter() -> Result {
    let mut base = rest_document(1440)?;
    let mut other = entity(&mut base, 4)?.clone();
    other["id"] = json!(id(41));
    other["category"] = json!("other");
    base.domain.entities.insert(id(41).parse()?, other);
    base.domain.entities.insert(
        id(42).parse()?,
        json!({"kind":"location","id":id(42),"name":"South","transitions":[]}),
    );
    for index in [43, 44] {
        base.domain.entities.insert(
            id(index).parse()?,
            json!({"kind":"team","id":id(index),"name":"Team"}),
        );
    }
    entity(&mut base, 1)?["teamIds"] = json!([id(43)]);
    entity(&mut base, 8)?["assignmentTypeId"] = json!(id(41));
    entity(&mut base, 8)?["locationId"] = json!(id(42));
    utc_times(
        entity(&mut base, 8)?,
        "2026-11-02T00:00:00",
        "2026-11-02T01:00:00",
    );
    let selected = [pair(1, 7)?, pair(1, 8)?];
    for scope in ["scope", "afterScope", "beforeScope"] {
        let target = scope == "beforeScope";
        for (field, positive, negative) in [
            (
                "people",
                json!({"kind":"selected","personIds":[id(1)]}),
                json!({"kind":"filter","allTags":["absent"],"anyTags":[]}),
            ),
            ("teamIds", json!([id(43)]), json!([id(44)])),
            (
                "assignmentTypeIds",
                if scope == "scope" {
                    json!([id(4), id(41)])
                } else {
                    json!([id(if target { 41 } else { 4 })])
                },
                json!([id(if target { 4 } else { 41 })]),
            ),
            (
                "categories",
                if scope == "scope" {
                    json!(["clinic", "other"])
                } else if target {
                    json!(["other"])
                } else {
                    json!(["clinic"])
                },
                if target {
                    json!(["clinic"])
                } else {
                    json!(["other"])
                },
            ),
            (
                "weekdays",
                if scope == "scope" {
                    json!(["sunday", "monday"])
                } else if target {
                    json!(["monday"])
                } else {
                    json!(["sunday"])
                },
                if target {
                    json!(["sunday"])
                } else {
                    json!(["monday"])
                },
            ),
            (
                "locationIds",
                if scope == "scope" {
                    json!([id(5), id(42)])
                } else {
                    json!([id(if target { 42 } else { 5 })])
                },
                json!([id(if target { 5 } else { 42 })]),
            ),
        ] {
            let mut value = base.clone();
            rest_binding(&mut value)?[scope][field] = positive;
            assert!(!allows(&compile(&value)?, &selected)?, "{scope}/{field}");
            rest_binding(&mut value)?[scope][field] = negative;
            let result = compile(&value)?;
            assert!(allows(&result, &selected)?, "{scope}/{field}");
            assert!(result.constraints.is_empty());
            assert!(result.obligations.handled.contains(&id(24).parse()?));
        }
    }
    rest_binding(&mut base)?["active"] = json!(false);
    let inactive = compile(&base)?;
    assert!(inactive.constraints.is_empty());
    assert!(!inactive.obligations.handled.contains(&id(24).parse()?));
    assert!(!inactive.obligations.remaining.contains(&id(24).parse()?));
    Ok(())
}

#[test]
fn minimum_rest_uses_elapsed_overnight_spring_and_fall_gaps() -> Result {
    for (horizon_start, horizon_end, source_end, end_local, end_offset, target_start, target_local, target_offset, allowed) in [
        ("2026-03-07T05:00:00Z", "2026-03-09T04:00:00Z", "2026-03-08T05:00:00Z", "2026-03-08T00:00:00", -18000,
            "2026-03-08T14:00:00Z", "2026-03-08T10:00:00", -14400, false),
        ("2026-10-31T04:00:00Z", "2026-11-02T05:00:00Z", "2026-11-01T04:00:00Z", "2026-11-01T00:00:00", -14400,
            "2026-11-01T15:00:00Z", "2026-11-01T10:00:00", -18000, true),
    ] {
        let mut value = serde_json::to_value(rest_document(600)?)?;
        value["settings"]["timeZone"] = json!("America/New_York");
        // Scenario-zone midnight boundaries include the source's prior-day start.
        value["settings"]["horizon"] = json!({"start":horizon_start,"end":horizon_end});
        let source = &mut value["domain"]["entities"][id(7)];
        let start: jiff::Timestamp = source_end.parse()?;
        let start = start
            .checked_sub(jiff::SignedDuration::from_hours(1))?
            .to_zoned(jiff::tz::TimeZone::get("America/New_York")?);
        source["startsAt"] = json!({"instant":start.timestamp().to_string(),"local":start.datetime().to_string(),"offsetSeconds":start.offset().seconds()});
        source["endsAt"] =
            json!({"instant":source_end,"local":end_local,"offsetSeconds":end_offset});
        source["reportingAttribution"] = json!("endLocalDate");
        let target = &mut value["domain"]["entities"][id(8)];
        target["startsAt"] =
            json!({"instant":target_start,"local":target_local,"offsetSeconds":target_offset});
        let end: jiff::Timestamp = target_start.parse()?;
        let end = end
            .checked_add(jiff::SignedDuration::from_hours(1))?
            .to_zoned(jiff::tz::TimeZone::get("America/New_York")?);
        target["endsAt"] = json!({"instant":end.timestamp().to_string(),"local":end.datetime().to_string(),"offsetSeconds":end.offset().seconds()});
        value["domain"]["rules"][id(24)]["afterScope"]["weekdays"] = json!(["sunday"]);
        assert_eq!(
            allows(
                &compile(&serde_json::from_value(value)?)?,
                &[pair(1, 7)?, pair(1, 8)?]
            )?,
            allowed
        );
    }
    Ok(())
}

fn rest_locks(value: &mut ScenarioDocument) -> Result {
    for (lock, shift) in [(50, 7), (51, 8)] {
        value.domain.locked_assignments.insert(
            id(lock).parse()?,
            json!({
                "id":id(lock),"personId":id(1),"shiftId":id(shift),"state":{"kind":"hard"},
            }),
        );
    }
    Ok(())
}

fn has_rest_lock_finding(result: &AssignmentRuleCompilation) -> bool {
    result
        .validation
        .issues
        .iter()
        .any(|issue| issue.code == "official.workforce.hard_lock_minimum_rest")
}

#[test]
fn minimum_rest_hard_lock_readiness_respects_scopes_activation_and_chronology() -> Result {
    let mut value = rest_document(600)?;
    rest_locks(&mut value)?;
    let positive = compile(&value)?;
    assert!(has_rest_lock_finding(&positive));
    let issue = positive
        .validation
        .issues
        .iter()
        .find(|issue| issue.code == "official.workforce.hard_lock_minimum_rest")
        .ok_or("rest finding")?;
    assert_eq!(issue.severity, eutheto_types::ValidationSeverity::Error);
    assert_eq!(
        issue.field_path,
        Some(format!("domain.lockedAssignments.{}", id(50)))
    );
    assert_eq!(
        issue.resource,
        Some(eutheto_types::ResourceRef::Assignment(id(50).parse()?))
    );
    assert!(
        [id(50), id(51), id(24)]
            .iter()
            .all(|id| issue.message.contains(id))
    );
    for scope in ["scope", "afterScope", "beforeScope"] {
        let mut excluded = value.clone();
        rest_binding(&mut excluded)?[scope]["people"] =
            json!({"kind":"filter","allTags":["absent"],"anyTags":[]});
        assert!(!has_rest_lock_finding(&compile(&excluded)?));
    }
    let mut inactive = value.clone();
    rest_binding(&mut inactive)?["active"] = json!(false);
    assert!(!has_rest_lock_finding(&compile(&inactive)?));
    let mut other = entity(&mut value, 4)?.clone();
    other["id"] = json!(id(41));
    value.domain.entities.insert(id(41).parse()?, other);
    entity(&mut value, 8)?["assignmentTypeId"] = json!(id(41));
    rest_binding(&mut value)?["afterScope"]["assignmentTypeIds"] = json!([id(41)]);
    rest_binding(&mut value)?["beforeScope"]["assignmentTypeIds"] = json!([id(4)]);
    assert!(!has_rest_lock_finding(&compile(&value)?));
    Ok(())
}

#[test]
fn minimum_rest_lock_findings_include_compatible_overlap_and_rejected_resolved_pairs() -> Result {
    let mut value = rest_document(0)?;
    utc_times(
        entity(&mut value, 8)?,
        "2026-11-01T00:30:00",
        "2026-11-01T02:00:00",
    );
    rule(&mut value, 23, "noOverlap")?;
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("overlap rule")?["compatibleCategoryPairs"] =
        json!([{"firstCategory":"clinic","secondCategory":"clinic"}]);
    rest_locks(&mut value)?;
    let overlap = compile(&value)?;
    assert!(has_rest_lock_finding(&overlap));
    assert!(
        !overlap
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_lock_overlap")
    );
    rule(&mut value, 21, "availability")?;
    availability(
        &mut value,
        30,
        "unavailable",
        "2026-11-01T00:00:00Z",
        "2026-11-01T00:15:00Z",
    )?;
    let rejected = compile(&value)?;
    assert!(has_rest_lock_finding(&rejected));
    assert!(
        rejected
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_lock_rejected_pair")
    );
    assert_eq!(rejected.estimate.variables, 1);
    for state in [
        json!({"kind":"soft","stabilityWeight":1}),
        json!({"kind":"unlocked"}),
    ] {
        let mut mixed = value.clone();
        mixed
            .domain
            .locked_assignments
            .get_mut(&id(50).parse()?)
            .ok_or("lock")?["state"] = state;
        assert!(!has_rest_lock_finding(&compile(&mixed)?));
    }
    let mut unresolved = document()?;
    rest_rule(&mut unresolved, 600)?;
    rest_locks(&mut unresolved)?;
    entity(&mut unresolved, 6)?["recurrence"]["excludedDates"] = json!(["2026-11-01"]);
    let unresolved = compile(&unresolved)?;
    assert!(!has_rest_lock_finding(&unresolved));
    assert!(
        unresolved
            .validation
            .issues
            .iter()
            .any(|issue| issue.code == "official.workforce.hard_lock_unresolved_shift")
    );
    Ok(())
}

#[test]
fn minimum_rest_exact_estimates_typed_provenance_and_caller_limits() -> Result {
    let value = rest_document(600)?;
    let result = compile(&value)?;
    assert_eq!(
        (
            result.estimate.variables,
            result.estimate.constraints,
            result.estimate.provenance_records,
            result.estimate.references
        ),
        (2, 1, 4, 16)
    );
    let fact = result
        .provenance
        .iter()
        .find(|fact| fact.id == result.constraints[0].provenance)
        .ok_or("rest fact")?;
    assert_eq!(fact.message_key, "official.workforce.minimum_rest");
    assert_eq!((fact.entity_refs.len(), fact.parameters.len()), (3, 3));
    let source = serde_json::to_value(&fact.parameters["source_shift"])?;
    let target = serde_json::to_value(&fact.parameters["target_shift"])?;
    assert!(source.to_string().contains(&id(7)));
    assert!(target.to_string().contains(&id(8)));
    assert_eq!(
        fact.parameters["minimum_minutes"],
        eutheto_planning_ir::ProvenanceParameter::Integer(600)
    );
    let parent = result
        .provenance
        .iter()
        .find(|record| Some(&record.id) == fact.parent.as_ref())
        .ok_or("parent")?;
    assert_eq!(parent.message_key, "official.workforce.minimum_rest");
    for (field, exact) in [(0, 1), (1, 3), (2, 3), (3, 600)] {
        let limits = |cap| {
            let mut limits = PlanningIrLimitsV1::DEFAULT;
            match field {
                0 => limits.max_constraints = cap,
                1 => limits.max_parameters_per_record = cap,
                2 => limits.max_entity_refs_per_record = cap,
                _ => limits.max_abs_value = cap as i64,
            }
            limits
        };
        assert_eq!(
            compile_assignment_rules(&value, &context(limits(exact)))?.constraints,
            result.constraints
        );
        assert!(matches!(
            compile_assignment_rules(&value, &context(limits(exact - 1))),
            Err(AssignmentRuleError::LimitExceeded(_))
        ));
    }
    // Aggregate ceilings include cumulative temporary storage as well as exact IR output.
    // Find the true operation boundary rather than incorrectly using final model bytes alone.
    for field in 0..3 {
        let limits = |cap| {
            let mut limits = PlanningIrLimitsV1::DEFAULT;
            match field {
                0 => limits.max_ir_bytes = cap,
                1 => limits.max_total_refs = cap,
                _ => limits.max_provenance_records = cap,
            }
            limits
        };
        let mut low = 0;
        let mut high = match field {
            0 => PlanningIrLimitsV1::DEFAULT.max_ir_bytes,
            1 => PlanningIrLimitsV1::DEFAULT.max_total_refs,
            _ => PlanningIrLimitsV1::DEFAULT.max_provenance_records,
        };
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if compile_assignment_rules(&value, &context(limits(middle))).is_ok() {
                high = middle;
            } else {
                low = middle;
            }
        }
        assert_eq!(
            compile_assignment_rules(&value, &context(limits(high)))?.constraints,
            result.constraints
        );
        assert!(matches!(
            compile_assignment_rules(&value, &context(limits(high - 1))),
            Err(AssignmentRuleError::LimitExceeded(_))
        ));
    }
    Ok(())
}

#[test]
fn minimum_rest_dense_equal_starts_retain_both_directions_at_exact_constraint_limit() -> Result {
    let mut value = rest_document(0)?;
    value.domain.entities.remove(&id(7).parse()?);
    let shift = entity(&mut value, 8)?.clone();
    for index in 1_000..1_031 {
        let mut record = shift.clone();
        record["id"] = json!(id(index));
        value.domain.entities.insert(id(index).parse()?, record);
    }
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_constraints = 32 * 31;
    let result = compile_assignment_rules(&value, &context(limits))?;
    assert_eq!(result.estimate.constraints, 32 * 31);
    assert_eq!(result.estimate.references, 32 * 3 + 32 * 31 * 10);
    assert!(!allows(&result, &[pair(1, 8)?, pair(1, 1_000)?])?);
    limits.max_constraints -= 1;
    assert_eq!(
        compile_assignment_rules(&value, &context(limits)).err(),
        Some(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Constraints
        ))
    );
    Ok(())
}

#[test]
fn minimum_rest_large_safe_suffix_does_not_scan_the_cartesian_population() -> Result {
    let mut value = rest_document(0)?;
    let shift = entity(&mut value, 8)?.clone();
    value.domain.entities.remove(&id(7).parse()?);
    value.domain.entities.remove(&id(8).parse()?);
    let mut person = entity(&mut value, 1)?.clone();
    person.as_object_mut().ok_or("person")?.remove("externalId");
    for index in 200..217 {
        let mut record = person.clone();
        record["id"] = json!(id(index));
        value.domain.entities.insert(id(index).parse()?, record);
    }
    let origin: jiff::Timestamp = "2026-11-01T00:00:00Z".parse()?;
    // Eighteen people each have 1,500 safe shifts: 20,236,500 applicable pairs
    // exceed the fixed work ceiling for a naive scan, without oversized input JSON.
    for index in 0..1_500_u32 {
        let mut record = shift.clone();
        record["id"] = json!(id(1_000 + index));
        let start = origin
            .checked_add(jiff::SignedDuration::from_secs(i64::from(index) * 2))?
            .to_zoned(jiff::tz::TimeZone::UTC)
            .datetime()
            .to_string();
        let end = origin
            .checked_add(jiff::SignedDuration::from_secs(i64::from(index) * 2 + 1))?
            .to_zoned(jiff::tz::TimeZone::UTC)
            .datetime()
            .to_string();
        utc_times(&mut record, &start, &end);
        value
            .domain
            .entities
            .insert(id(1_000 + index).parse()?, record);
    }
    let result = compile(&value)?;
    assert_eq!(result.estimate.variables, 18 * 1_500);
    assert_eq!(result.estimate.constraints, 0);
    assert!(allows(&result, &[pair(1, 1_000)?, pair(1, 2_499)?])?);
    Ok(())
}

#[test]
fn minimum_rest_extreme_signed_gaps_do_not_overflow_total_nanoseconds() -> Result {
    let mut value = serde_json::to_value(rest_document(u32::MAX)?)?;
    value["settings"]["horizon"] = json!({
        "start":"-009000-12-30T00:00:00Z","end":"9000-12-31T00:00:00Z",
    });
    utc_times(&mut value["domain"]["entities"][id(7)], "-009000-12-30T00:00:00", "-009000-12-30T01:00:00");
    utc_times(&mut value["domain"]["entities"][id(8)], "9000-12-30T10:00:00", "9000-12-30T12:00:00");
    assert!(allows(&compile(&serde_json::from_value(value.clone())?)?, &[pair(1, 7)?, pair(1, 8)?])?);
    value["domain"]["rules"][id(24)]["minimumMinutes"] = json!(0);
    utc_times(&mut value["domain"]["entities"][id(7)], "-009000-12-30T00:00:00", "9000-12-30T11:00:00");
    utc_times(&mut value["domain"]["entities"][id(8)], "-008999-12-30T10:00:00", "-008999-12-30T12:00:00");
    assert!(!allows(&compile(&serde_json::from_value(value)?)?, &[pair(1, 7)?, pair(1, 8)?])?);
    Ok(())
}
