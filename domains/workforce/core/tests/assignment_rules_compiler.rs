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
fn allows(model: &AssignmentRuleCompilation, selected: &[AssignmentPair]) -> bool {
    if selected
        .iter()
        .any(|pair| !model.variables.iter().any(|entry| entry.pair == *pair))
    {
        return false;
    }
    let values: BTreeMap<_, _> = model
        .variables
        .iter()
        .map(|entry| (&entry.variable.id, selected.contains(&entry.pair)))
        .collect();
    model.constraints.iter().all(|record| {
        let satisfied = |literal: &eutheto_planning_ir::Literal| {
            values
                .get(&literal.variable)
                .is_some_and(|value| *value == literal.positive)
        };
        match &record.body {
            Constraint::BoolOr { literals } => literals.iter().any(satisfied),
            Constraint::AtMostOne { literals } => {
                literals.iter().filter(|literal| satisfied(literal)).count() <= 1
            }
            Constraint::CardinalityRange { literals, min, max } => {
                let count = literals.iter().filter(|literal| satisfied(literal)).count() as u64;
                *min <= count && count <= *max
            }
            _ => panic!("unexpected primitive in four-rule contribution"),
        }
    })
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?]));
    value
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("missing rule")?["active"] = json!(false);
    let result = compile(&value)?;
    assert!(!result.obligations.handled.contains(&id(20).parse()?));
    assert!(allows(&result, &[pair(1, 7)?]));
    Ok(())
}

#[test]
fn adjacent_qualification_renewals_cover_but_nanosecond_gap_and_expiry_do_not() -> Result {
    let mut value = document()?;
    rule(&mut value, 20, "eligibility")?;
    entity(&mut value, 1)?["qualificationGrants"] = json!([
        {"qualificationId":id(11),"expiresAt":"2026-11-01T06:00:00Z"},
        {"qualificationId":id(11),"effectiveFrom":"2026-11-01T06:00:00Z","expiresAt":"2026-11-01T07:30:00Z"}]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
    entity(&mut value, 30)?["timeWindow"]["windows"][1]["startTime"] = json!("02:00:00.000000001");
    let result = compile(&value)?;
    assert!(allows(&result, &[pair(1, 7)?]));
    assert!(!allows(&result, &[pair(1, 8)?]));
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
        assert_eq!(allows(&result, &[]), none);
        assert_eq!(allows(&result, &[pair(1, 7)?]), one);
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
    assert!(allows(&compile(&value)?, &[]));
    entity(&mut value, 6)?["coverage"]["count"] = json!(1);
    assert!(!allows(&compile(&value)?, &[]));
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
    assert!(allows(&result, &[pair(1, 7)?]));
    assert_eq!(result.constraints.len(), 3);
    value.domain.entities.insert(id(40).parse()?, json!({"kind":"coverageRequirement","id":id(40),"active":true,"scope":{"kind":"all"},"coverage":{"kind":"exact","count":1,"qualificationMinimums":[minimum(vec![id(11)])]}}));
    let independent = compile(&value)?;
    assert_eq!(independent.constraints.len(), 5);
    assert!(allows(&independent, &[pair(1, 7)?]));
    entity(&mut value, 1)?["qualificationGrants"] = json!([{"qualificationId":id(11)}]);
    let missing = compile(&value)?;
    assert!(!allows(&missing, &[pair(1, 7)?]));
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
    assert!(!allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["compatibleCategoryPairs"] =
        json!([{"firstCategory":"clinic","secondCategory":"clinic"}]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
    rule(&mut value, 24, "noOverlap")?;
    assert!(!allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
    entity(&mut value, 8)?["startsAt"] = json!({"instant":"2026-11-01T06:30:00Z","local":"2026-11-01T01:30:00","offsetSeconds":-18000});
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
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
    assert!(!allows(&result, &[]));
    assert!(!allows(&result, &[pair(1, 7)?]));
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
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["scope"]["assignmentTypeIds"] = json!([id(4), id(41)]);
    assert!(!allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
    value
        .domain
        .rules
        .get_mut(&id(23).parse()?)
        .ok_or("missing rule")?["scope"]["categories"] = json!(["clinic"]);
    assert!(allows(&compile(&value)?, &[pair(1, 7)?, pair(1, 8)?]));
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
    assert!(!allows(&result, &[]));
    assert!(!allows(&result, &[pair(1, 7)?]));
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
    // One source rule/large leaf list amplified across valid resolved assignments, not
    // across copied oversized rule records. The raw Cartesian product is only 2,048.
    for index in 0..2047 {
        let mut record = shift.clone();
        record["id"] = json!(id(1000 + index));
        value.domain.entities.insert(id(1000 + index).parse()?, record);
    }
    rule(&mut value, 20, "eligibility")?;
    Ok(value)
}

fn add_leaf_entities(value: &mut ScenarioDocument, kind: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for index in 0..10_000 {
        let identity = id(100_000 + index);
        let mut record = json!({"kind":kind,"id":identity,"name":"Reference"});
        if kind == "qualification" { record["description"] = json!(""); }
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
    // 2,048 × 10,000 outer leaves exceed the work ceiling even though each leaf
    // scans zero grants. Without the outer checkpoint this valid operation succeeds.
    assert_eq!(analyze_assignments(&value, None, PlanningIrLimitsV1::DEFAULT),
        Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)));
    entity(&mut value, 4)?["qualifications"] = json!({
        "kind":"matches","allQualificationIds":[],"anyQualificationIds":ids,
    });
    assert_eq!(compile_assignment_rules(&value, &context(PlanningIrLimitsV1::DEFAULT)),
        Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)));
    Ok(())
}

#[test]
fn empty_person_tags_and_teams_cannot_bypass_outer_filter_work_limits() -> Result {
    let mut value = amplification_document()?;
    entity(&mut value, 1)?["tags"] = json!([]);
    entity(&mut value, 1)?["teamIds"] = json!([]);
    let tags: Vec<_> = (0..10_000).map(|index| format!("tag-{index}")).collect();
    for field in ["allTags", "anyTags"] {
        let mut people = json!({"kind":"filter","allTags":[],"anyTags":[]});
        people[field] = json!(tags);
        value.domain.rules.get_mut(&id(20).parse()?).ok_or("missing rule")?["scope"] =
            json!({"people":people});
        assert_eq!(analyze_assignments(&value, None, PlanningIrLimitsV1::DEFAULT),
            Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)));
    }
    let teams = add_leaf_entities(&mut value, "team")?;
    value.domain.rules.get_mut(&id(20).parse()?).ok_or("missing rule")?["scope"] =
        json!({"people":{"kind":"all"},"teamIds":teams});
    assert_eq!(compile_assignment_rules(&value, &context(PlanningIrLimitsV1::DEFAULT)),
        Err(AssignmentRuleError::LimitExceeded(AssignmentRuleLimit::WorkSteps)));
    Ok(())
}
