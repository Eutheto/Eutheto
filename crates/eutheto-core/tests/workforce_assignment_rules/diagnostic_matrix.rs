#![allow(clippy::too_many_lines)]

//! Independent diagnostic-compiler coverage.
//!
//! The ordinary `WorkforcePack` compiler and the diagnostic compiler deliberately have different
//! contracts. These tests build small source documents, inspect the diagnostic IR, and use the
//! source verifier as a second semantic authority. The ignored native test is intentionally
//! fail-closed when an approved `OR-Tools` artifact is not installed; it must not turn a missing
//! worker into a skipped claim.

use super::{
    authority, context, fixture_value, id, original_accepts, pair, solve_with_real_worker,
};
use eutheto_domain_api::{DomainPack, InfeasibilityCompileContext};
use eutheto_domain_ir::{
    AssignmentValue, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
    DomainAssignmentId,
};
use eutheto_planning_ir::{
    Constraint, MetadataKey, ObjectivePlan, PlanningIrLimitsV1, PlanningProblem,
    ProjectionExpression, ProvenanceId, ProvenanceParameter, ProvenanceSourceKind,
    canonical_ir_hash, validate,
};
use eutheto_solver_api::{BackendSolveResult, BackendTerminationReason};
use eutheto_types::{
    CancellationToken, DurationMillis, ParentSolveBudget, RuleId, ScenarioDocument,
    SystemMonotonicClock,
};
use eutheto_workforce::WorkforcePack;
use serde_json::{Value, json};
use std::{error::Error, path::PathBuf, sync::Arc};

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

#[derive(Clone)]
struct FamilyCase {
    name: &'static str,
    value: Value,
    rules: Vec<RuleId>,
    /// Assumption sets to remove for a native feasible trial. `FixedAssignment` has two useful
    /// one-rule drops, one for each polarity; all other cases have one target rule.
    drop_sets: Vec<Vec<RuleId>>,
}

fn rule(number: u32) -> Result<RuleId> {
    Ok(id(number).parse()?)
}

fn clean_value() -> Result<Value> {
    let mut value = fixture_value()?;
    value["domain"]["rules"] = json!({});
    value["domain"]["lockedAssignments"] = json!({});
    Ok(value)
}

fn all_scope() -> Value {
    json!({"people":{"kind":"all"}})
}

fn selected_scope(person: u32) -> Value {
    json!({"people":{"kind":"selected","personIds":[id(person)]}})
}

fn required_rule(kind: &str, number: u32, scope: &Value) -> Value {
    json!({
        "kind": kind,
        "id": id(number),
        "active": true,
        "strength": "required",
        "scope": scope,
    })
}

fn add_lock(value: &mut Value, number: u32, person: u32, shift: u32) {
    value["domain"]["lockedAssignments"][id(number)] = json!({
        "id": id(number),
        "personId": id(person),
        "shiftId": id(shift),
        "state": {"kind":"hard"},
    });
}

fn add_fixed(value: &mut Value, number: u32, person: u32, shift: u32, assigned: bool) {
    let mut fixed = required_rule("fixedAssignment", number, &all_scope());
    fixed["personId"] = json!(id(person));
    fixed["shiftId"] = json!(id(shift));
    fixed["assigned"] = json!(assigned);
    value["domain"]["rules"][id(number)] = fixed;
}

fn unavailable_record(number: u32, person: u32) -> Value {
    json!({
        "kind":"availability",
        "id":id(number),
        "personId":id(person),
        "availabilityKind":"unavailable",
        "timeWindow": {
            "kind":"instant",
            "startsAt":"2026-11-01T07:00:00Z",
            "endsAt":"2026-11-01T09:00:00Z"
        },
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"diagnostic-matrix",
        "note":"",
    })
}

fn add_rule(value: &mut Value, number: u32, rule_value: Value) {
    value["domain"]["rules"][id(number)] = rule_value;
}

fn family_cases() -> Result<Vec<FamilyCase>> {
    let mut cases = Vec::new();

    // Eligibility: authored type rejection is restored into the diagnostic graph and is guarded
    // by its own assumption; the fixed pair makes the source contradiction observable.
    let mut value = clean_value()?;
    for person in [1, 20, 21] {
        value["domain"]["entities"][id(person)]["eligibleAssignmentTypeIds"] = json!([]);
    }
    add_rule(
        &mut value,
        30,
        required_rule("eligibility", 30, &all_scope()),
    );
    add_fixed(&mut value, 40, 1, 8, true);
    cases.push(FamilyCase {
        name: "eligibility",
        value,
        rules: vec![rule(30)?, rule(40)?],
        drop_sets: vec![vec![rule(30)?]],
    });

    // Availability: the same exact pair is rejected by an authored unavailable interval, not by
    // approved leave.  A fixed assignment forces the pair while the family is active.
    let mut value = clean_value()?;
    for person in [1, 20, 21] {
        value["domain"]["entities"][id(person)]["eligibleAssignmentTypeIds"] = json!([id(4)]);
        value["domain"]["entities"][id(person)]["qualificationGrants"] =
            json!([{"qualificationId":id(11)}]);
        value["domain"]["entities"][id(50 + person)] = unavailable_record(50 + person, person);
    }
    add_rule(
        &mut value,
        31,
        required_rule("availability", 31, &all_scope()),
    );
    add_fixed(&mut value, 40, 1, 8, true);
    cases.push(FamilyCase {
        name: "availability",
        value,
        rules: vec![rule(31)?, rule(40)?],
        drop_sets: vec![vec![rule(31)?]],
    });

    // Coverage: four required assignees are impossible in the three-person source universe.  The
    // hard lock retains one assignment after the coverage assumption is dropped.
    let mut value = clean_value()?;
    value["domain"]["entities"][id(8)]["coverage"] = json!({
        "kind":"exact", "count":4, "qualificationMinimums":[]
    });
    add_rule(&mut value, 32, required_rule("coverage", 32, &all_scope()));
    add_lock(&mut value, 50, 1, 8);
    cases.push(FamilyCase {
        name: "coverage",
        value,
        rules: vec![rule(32)?],
        drop_sets: vec![vec![rule(32)?]],
    });

    // NoOverlap: activity facts leave one person in the candidate universe while two hard locks
    // force that person onto overlapping shifts.
    let mut value = clean_value()?;
    for person in [20, 21] {
        value["domain"]["entities"][id(person)]["activeRange"] = json!({
            "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
        });
    }
    value["domain"]["entities"][id(22)]["startsAt"] = super::endpoint("06:30:00");
    value["domain"]["entities"][id(22)]["endsAt"] = super::endpoint("08:30:00");
    let mut no_overlap = required_rule("noOverlap", 33, &all_scope());
    no_overlap["compatibleCategoryPairs"] = json!([]);
    add_rule(&mut value, 33, no_overlap);
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 1, 22);
    cases.push(FamilyCase {
        name: "no-overlap",
        value,
        rules: vec![rule(33)?],
        drop_sets: vec![vec![rule(33)?]],
    });

    // Minimum rest: a call ending at 10:00 followed by a clinic at 18:00 leaves only eight hours
    // against a ten-hour minimum.  Hard locks make the violating person unavoidable.
    let mut value = clean_value()?;
    let mut call = value["domain"]["entities"][id(4)].clone();
    call["id"] = json!(id(24));
    call["name"] = json!("Call");
    call["category"] = json!("call");
    value["domain"]["entities"][id(24)] = call;
    for person in [1, 20, 21] {
        value["domain"]["entities"][id(person)]["eligibleAssignmentTypeIds"] =
            json!([id(4), id(24)]);
    }
    value["domain"]["entities"][id(8)]["assignmentTypeId"] = json!(id(24));
    value["domain"]["entities"][id(8)]["startsAt"] = super::endpoint("08:00:00");
    value["domain"]["entities"][id(8)]["endsAt"] = super::endpoint("10:00:00");
    value["domain"]["entities"][id(22)]["startsAt"] = super::endpoint("18:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = super::endpoint("20:00:00");
    add_rule(
        &mut value,
        34,
        json!({
            "kind":"minimumRest", "id":id(34), "active":true, "strength":"required",
            "scope":all_scope(),
            "afterScope":{"people":{"kind":"all"},"categories":["call"]},
            "beforeScope":{"people":{"kind":"all"},"categories":["clinic"]},
            "minimumMinutes":600,
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 1, 22);
    cases.push(FamilyCase {
        name: "minimum-rest",
        value,
        rules: vec![rule(34)?],
        drop_sets: vec![vec![rule(34)?]],
    });

    // Maximum assignment count: one locked assignment conflicts with a zero cap.
    let mut value = clean_value()?;
    add_rule(
        &mut value,
        35,
        json!({
            "kind":"maximumAssignmentCount", "id":id(35), "active":true, "strength":"required",
            "scope":all_scope(), "calendarId":id(2), "maximum":0,
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    cases.push(FamilyCase {
        name: "maximum-assignment-count",
        value,
        rules: vec![rule(35)?],
        drop_sets: vec![vec![rule(35)?]],
    });

    // Workload quota: two hard-locked assignments on one day violate an exact count of one for
    // person 1.  This exercises the quota cardinality channel, not just its source evaluator.
    let mut value = clean_value()?;
    add_rule(
        &mut value,
        36,
        json!({
            "kind":"workloadQuota", "id":id(36), "active":true, "strength":"required",
            "scope":selected_scope(1), "calendarId":id(2), "membership":"reportingDate",
            "measure":{"kind":"assignmentCount"}, "minimum":1, "maximum":1,
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 1, 22);
    cases.push(FamilyCase {
        name: "workload-quota",
        value,
        rules: vec![rule(36)?],
        drop_sets: vec![vec![rule(36)?]],
    });

    // Maximum hours: the same pair of locks contributes 240 elapsed minutes against a 120 minute
    // calendar cap.  The diagnostic helper variables remain present and are guarded.
    let mut value = clean_value()?;
    add_rule(
        &mut value,
        37,
        json!({
            "kind":"maximumHours", "id":id(37), "active":true, "strength":"required",
            "scope":selected_scope(1), "bucketId":id(3),
            "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
            "maximumMinutes":120,
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 1, 22);
    cases.push(FamilyCase {
        name: "maximum-hours",
        value,
        rules: vec![rule(37)?],
        drop_sets: vec![vec![rule(37)?]],
    });

    // Maximum consecutive assignments: touching locked assignments form a run of two while the
    // authored limit is one.
    let mut value = clean_value()?;
    value["domain"]["entities"][id(8)]["startsAt"] = super::endpoint("08:00:00");
    value["domain"]["entities"][id(8)]["endsAt"] = super::endpoint("09:00:00");
    value["domain"]["entities"][id(22)]["startsAt"] = super::endpoint("09:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = super::endpoint("10:00:00");
    add_rule(
        &mut value,
        38,
        json!({
            "kind":"maximumConsecutive", "id":id(38), "active":true, "strength":"required",
            "scope":all_scope(), "mode":{"kind":"assignments","breakMinutes":0}, "maximum":1,
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 1, 22);
    cases.push(FamilyCase {
        name: "maximum-consecutive",
        value,
        rules: vec![rule(38)?],
        drop_sets: vec![vec![rule(38)?]],
    });

    // Maximum worked weekends: November 1, 2026 is a Sunday, so a zero weekend cap conflicts with
    // one immutable assignment.
    let mut value = clean_value()?;
    add_rule(
        &mut value,
        39,
        json!({
            "kind":"maximumWorkedWeekends", "id":id(39), "active":true, "strength":"required",
            "scope":all_scope(), "maximum":0,
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    cases.push(FamilyCase {
        name: "maximum-worked-weekends",
        value,
        rules: vec![rule(39)?],
        drop_sets: vec![vec![rule(39)?]],
    });

    // Required skill mix: the shift asks for qualification 12, while its hard-locked assignee has
    // only qualification 11.  Qualification rows are helper-family inputs, not assumptions.
    let mut value = clean_value()?;
    value["domain"]["entities"][id(12)] = json!({
        "kind":"qualification", "id":id(12), "name":"Specialist", "description":""
    });
    value["domain"]["entities"][id(8)]["coverage"]["qualificationMinimums"] = json!([
        {"qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},"minimum":1}
    ]);
    add_rule(
        &mut value,
        40,
        json!({
            "kind":"requiredSkillMix", "id":id(40), "active":true, "strength":"required",
            "scope":all_scope(), "qualificationMinimums":[
                {"qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},"minimum":1}
            ],
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    cases.push(FamilyCase {
        name: "required-skill-mix",
        value,
        rules: vec![rule(40)?],
        drop_sets: vec![vec![rule(40)?]],
    });

    // FixedAssignment polarity: one positive and one exact negative pin on the same pair are
    // contradictory.  The native drops below independently exercise each polarity.
    let mut value = clean_value()?;
    add_fixed(&mut value, 41, 1, 8, true);
    add_fixed(&mut value, 42, 1, 8, false);
    cases.push(FamilyCase {
        name: "fixed-assignment-polarity",
        value,
        rules: vec![rule(41)?, rule(42)?],
        drop_sets: vec![vec![rule(41)?], vec![rule(42)?]],
    });

    // Mutual restriction: two overlapping assignments are hard facts, while the authored rule
    // forbids that pair of people from covering overlapping shifts together.
    let mut value = clean_value()?;
    value["domain"]["entities"][id(22)]["startsAt"] = super::endpoint("06:30:00");
    value["domain"]["entities"][id(22)]["endsAt"] = super::endpoint("08:30:00");
    add_rule(
        &mut value,
        43,
        json!({
            "kind":"mutualAssignmentRestriction", "id":id(43), "active":true, "strength":"required",
            "scope":all_scope(), "personIds":[id(1),id(20)], "mode":"overlappingShifts",
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 20, 22);
    cases.push(FamilyCase {
        name: "mutual-assignment-restriction",
        value,
        rules: vec![rule(43)?],
        drop_sets: vec![vec![rule(43)?]],
    });

    // TransitionTime: a directed 30-minute edge is shorter than the 15-minute gap between two
    // hard-locked assignments for one person.
    let mut value = clean_value()?;
    value["domain"]["entities"][id(40)] = json!({
        "kind":"location", "id":id(40), "name":"South", "transitions":[]
    });
    value["domain"]["entities"][id(5)]["transitions"] = json!([
        {"locationId":id(40),"minutes":30}
    ]);
    value["domain"]["entities"][id(22)]["locationId"] = json!(id(40));
    value["domain"]["entities"][id(22)]["startsAt"] = super::endpoint("10:15:00");
    value["domain"]["entities"][id(22)]["endsAt"] = super::endpoint("12:15:00");
    add_rule(
        &mut value,
        44,
        json!({
            "kind":"transitionTime", "id":id(44), "active":true, "strength":"required",
            "scope":all_scope(), "locationIds":[id(5),id(40)],
        }),
    );
    add_lock(&mut value, 50, 1, 8);
    add_lock(&mut value, 51, 1, 22);
    cases.push(FamilyCase {
        name: "transition-time",
        value,
        rules: vec![rule(44)?],
        drop_sets: vec![vec![rule(44)?]],
    });

    Ok(cases)
}

fn diagnostic_problem(
    document: &ScenarioDocument,
    condition: Option<&CounterfactualConditionV1>,
) -> Result<PlanningProblem> {
    let compile_context = context();
    let mut base = WorkforcePack.compile(document, &compile_context)?;
    if let Some(condition) = condition {
        base.metadata.compile_metadata.insert(
            MetadataKey::new("eutheto.counterfactual.condition_hash")?,
            ProvenanceParameter::Text(condition.checksum.clone()),
        );
    }
    let parent = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    Ok(WorkforcePack.compile_infeasibility(
        document,
        &InfeasibilityCompileContext {
            base_problem: &base,
            compile_context: &compile_context,
            condition,
            budget: parent.phase_view(),
        },
    )?)
}

fn assumption(problem: &PlanningProblem, rule: RuleId) -> Result<&eutheto_planning_ir::Assumption> {
    problem
        .assumptions
        .iter()
        .find(|assumption| assumption.required_rules == vec![rule])
        .ok_or_else(|| format!("missing diagnostic assumption for {rule}").into())
}

fn source_parent(problem: &PlanningProblem, rule: RuleId) -> Result<ProvenanceId> {
    problem
        .provenance
        .iter()
        .find(|record| {
            record.source_kind == ProvenanceSourceKind::RequiredRule
                && record.source_id == rule.to_string()
                && record.parent.is_none()
        })
        .map(|record| record.id.clone())
        .ok_or_else(|| format!("missing source provenance for {rule}").into())
}

fn provenance_descends(
    problem: &PlanningProblem,
    child: &ProvenanceId,
    ancestor: &ProvenanceId,
) -> bool {
    let mut current = child.clone();
    for _ in 0..problem.provenance.len() {
        if &current == ancestor {
            return true;
        }
        let Some(record) = problem
            .provenance
            .iter()
            .find(|record| record.id == current)
        else {
            return false;
        };
        let Some(parent) = &record.parent else {
            return false;
        };
        current = parent.clone();
    }
    false
}

fn selected_pairs(
    problem: &PlanningProblem,
    result: &BackendSolveResult,
) -> Result<Vec<eutheto_workforce::model::AssignmentPair>> {
    let candidate = result
        .candidates
        .first()
        .ok_or("native diagnostic feasibility result had no candidate")?;
    let mut selected = Vec::new();
    for projection in &problem.projections {
        let ProjectionExpression::Boolean(variable) = &projection.expression else {
            continue;
        };
        if !candidate
            .values
            .booleans
            .get(variable)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        let entity = projection.entity.id.as_str();
        for person in [1, 20, 21] {
            for shift in [8, 22] {
                if entity == format!("{}.{}", id(person), id(shift)) {
                    selected.push(pair(person, shift)?);
                }
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    Ok(selected)
}

fn source_rule_satisfied(
    document: &ScenarioDocument,
    selected: &[eutheto_workforce::model::AssignmentPair],
    rule: RuleId,
) -> Result<bool> {
    let report = authority::verify_selection(document, selected)?;
    report
        .required_rule_results
        .iter()
        .find(|entry| entry.rule_id == rule)
        .map(|entry| entry.satisfied)
        .ok_or_else(|| format!("source verifier omitted rule {rule}").into())
}
fn baseline_selection(name: &str) -> Result<Vec<eutheto_workforce::model::AssignmentPair>> {
    match name {
        "eligibility"
        | "availability"
        | "coverage"
        | "maximum-assignment-count"
        | "maximum-worked-weekends"
        | "required-skill-mix"
        | "fixed-assignment-polarity" => Ok(vec![pair(1, 8)?]),
        "no-overlap"
        | "minimum-rest"
        | "maximum-consecutive"
        | "maximum-hours"
        | "transition-time"
        | "workload-quota" => Ok(vec![pair(1, 8)?, pair(1, 22)?]),
        "mutual-assignment-restriction" => Ok(vec![pair(1, 8)?, pair(20, 22)?]),
        other => Err(format!("missing independent baseline for {other}").into()),
    }
}

fn drop_assumptions(problem: &PlanningProblem, dropped: &[RuleId]) -> Result<PlanningProblem> {
    let mut result = problem.clone();
    result.assumptions.retain(|assumption| {
        !assumption
            .required_rules
            .iter()
            .any(|rule| dropped.contains(rule))
    });
    result.canonicalize()?;
    validate(&result, PlanningIrLimitsV1::DEFAULT)?;
    Ok(result)
}

#[test]
fn diagnostic_matrix_compiles_all_required_families_with_independent_guard_ownership() -> Result {
    for case in family_cases()? {
        let document: ScenarioDocument = serde_json::from_value(case.value.clone())?;
        let ordinary = WorkforcePack.compile(&document, &context())?;
        let ordinary_hash = canonical_ir_hash(&ordinary, PlanningIrLimitsV1::DEFAULT)?;
        let diagnostic = diagnostic_problem(&document, None)?;
        let ordinary_again = WorkforcePack.compile(&document, &context())?;
        assert_eq!(
            diagnostic.objectives,
            ObjectivePlan::default(),
            "{}",
            case.name
        );
        assert_eq!(
            diagnostic.assumptions.len(),
            case.rules.len(),
            "{}",
            case.name
        );
        assert_eq!(
            canonical_ir_hash(&ordinary_again, PlanningIrLimitsV1::DEFAULT)?,
            ordinary_hash,
            "ordinary model changed while compiling {}",
            case.name
        );

        for &rule in &case.rules {
            let assumption = assumption(&diagnostic, rule)?;
            assert!(
                assumption.literal.positive,
                "{}: guard must be positive",
                case.name
            );
            let parent = source_parent(&diagnostic, rule)?;
            let owned = diagnostic
                .constraints
                .iter()
                .filter(|constraint| {
                    provenance_descends(&diagnostic, &constraint.provenance, &parent)
                })
                .collect::<Vec<_>>();
            assert!(
                !owned.is_empty(),
                "{}: rule {rule} emitted no constraint",
                case.name
            );
            assert!(
                owned
                    .iter()
                    .all(|constraint| constraint.enforcement.contains(&assumption.literal)),
                "{}: rule {rule} has an unguarded owned constraint",
                case.name
            );
        }

        // Hard locks are facts, never diagnostic groups.  Every emitted hard-lock record remains
        // unconditional even in a source that also has relaxable authored rules.
        for fact in diagnostic
            .provenance
            .iter()
            .filter(|record| record.message_key == "official.workforce.hard_lock")
        {
            assert!(
                diagnostic
                    .constraints
                    .iter()
                    .filter(|constraint| provenance_descends(
                        &diagnostic,
                        &constraint.provenance,
                        &fact.id
                    ))
                    .all(|constraint| constraint.enforcement.is_empty()),
                "{}: hard-lock fact became relaxable",
                case.name
            );
        }

        // This is deliberately a separate source authority check: it rejects a compiler that
        // merely copied labels/guards while changing which source family owns the contradiction.
        for &rule in &case.rules {
            assert!(
                diagnostic.provenance.iter().any(|record| {
                    record.source_kind == ProvenanceSourceKind::RequiredRule
                        && record.source_id == rule.to_string()
                }),
                "{}: source rule {rule} absent from diagnostic provenance",
                case.name
            );
        }
    }
    Ok(())
}

#[test]
fn diagnostic_restores_mixed_authored_rejections_but_retains_fact_pruned_pairs() -> Result {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(1)]["activeRange"] = json!({
        "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
    });
    value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([]);
    value["domain"]["entities"][id(20)]["eligibleAssignmentTypeIds"] = json!([]);
    value["domain"]["entities"][id(20)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    value["domain"]["entities"][id(50)] = unavailable_record(50, 20);
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let diagnostic = diagnostic_problem(&document, None)?;

    assert!(
        !diagnostic
            .projections
            .iter()
            .any(|projection| { projection.entity.id.as_str() == format!("{}.{}", id(1), id(8)) }),
        "activity fact must not be restored by authored rejection"
    );
    assert_eq!(
        diagnostic
            .projections
            .iter()
            .filter(|projection| {
                projection.entity.id.as_str() == format!("{}.{}", id(20), id(8))
            })
            .count(),
        1,
        "pair rejected by two authored rules must be restored exactly once"
    );
    let rejection_records = diagnostic
        .provenance
        .iter()
        .filter(|record| record.message_key == "official.workforce.diagnostic.candidate_rejection")
        .collect::<Vec<_>>();
    assert!(
        rejection_records
            .iter()
            .any(|record| record.source_id == id(30))
    );
    assert!(
        rejection_records
            .iter()
            .any(|record| record.source_id == id(31))
    );
    assert!(
        rejection_records.iter().all(|record| {
            diagnostic
                .constraints
                .iter()
                .find(|constraint| constraint.provenance == record.id)
                .is_some_and(|constraint| constraint.enforcement.len() == 1)
        }),
        "restored authored rejection must stay guarded"
    );
    // The independently authored activity fact is not a Required rule and therefore cannot become
    // an assumption merely because the same pair also had an Eligibility rejection.
    let person_rule: RuleId = id(1).parse()?;
    assert!(
        diagnostic
            .assumptions
            .iter()
            .all(|assumption| !assumption.required_rules.contains(&person_rule))
    );
    Ok(())
}

#[test]
fn diagnostic_immutable_activity_leave_and_lock_channels_are_not_assumptions() -> Result {
    let mut value = clean_value()?;
    value["domain"]["entities"][id(50)] = json!({
        "kind":"availability", "id":id(50), "personId":id(1),
        "availabilityKind":"approvedTimeOff",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T07:00:00Z","endsAt":"2026-11-01T09:00:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"diagnostic-matrix", "note":"",
    });
    value["domain"]["entities"][id(1)]["activeRange"] = json!({
        "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
    });
    add_lock(&mut value, 51, 20, 22);
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let diagnostic = diagnostic_problem(&document, None)?;
    assert!(diagnostic.assumptions.is_empty());
    assert!(
        !diagnostic
            .projections
            .iter()
            .any(|projection| { projection.entity.id.as_str() == format!("{}.{}", id(1), id(8)) })
    );
    let lock_fact = diagnostic
        .provenance
        .iter()
        .find(|record| record.message_key == "official.workforce.hard_lock")
        .ok_or("missing immutable lock fact")?;
    assert!(diagnostic.constraints.iter().any(|constraint| {
        provenance_descends(&diagnostic, &constraint.provenance, &lock_fact.id)
            && constraint.enforcement.is_empty()
    }));
    Ok(())
}

fn assignment_id(person: u32, shift: u32) -> Result<DomainAssignmentId> {
    Ok(DomainAssignmentId::new(format!(
        "official.workforce.assignment.{}.{}",
        id(person),
        id(shift)
    ))?)
}

fn condition_force(person: u32, shift: u32, value: bool) -> Result<CounterfactualConditionV1> {
    Ok(CounterfactualConditionV1::new(
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: assignment_id(person, shift)?,
            value: AssignmentValue::Boolean(value),
        },
    )?)
}

fn condition_forbid(person: u32, shift: u32, value: bool) -> Result<CounterfactualConditionV1> {
    Ok(CounterfactualConditionV1::new(
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id: assignment_id(person, shift)?,
            value: AssignmentValue::Boolean(value),
        },
    )?)
}

#[test]
fn diagnostic_conditions_preserve_signed_polarity_and_replacement_pins() -> Result {
    let document: ScenarioDocument = serde_json::from_value(clean_value()?)?;
    let mut conditions = Vec::new();
    for value in [false, true] {
        conditions.push((condition_force(1, 8, value)?, value));
        conditions.push((condition_forbid(1, 8, value)?, !value));
    }
    let replacement =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: assignment_id(1, 8)?,
            replaced_assignment_id: assignment_id(20, 8)?,
        })?;
    for (condition, expected) in conditions {
        let diagnostic = diagnostic_problem(&document, Some(&condition))?;
        let pins = diagnostic
            .provenance
            .iter()
            .filter(|record| record.message_key == "official.workforce.diagnostic.condition_pin")
            .collect::<Vec<_>>();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].source_id, condition.checksum);
        let pin = diagnostic
            .constraints
            .iter()
            .find(|constraint| constraint.provenance == pins[0].id)
            .ok_or("missing signed condition constraint")?;
        assert!(pin.enforcement.is_empty());
        assert!(matches!(
            &pin.body,
            Constraint::BoolOr { literals }
                if literals.len() == 1 && literals[0].positive == expected
        ));
    }
    let diagnostic = diagnostic_problem(&document, Some(&replacement))?;
    let pins = diagnostic
        .provenance
        .iter()
        .filter(|record| record.message_key == "official.workforce.diagnostic.condition_pin")
        .collect::<Vec<_>>();
    assert_eq!(pins.len(), 2);
    for pin in pins {
        let constraint = diagnostic
            .constraints
            .iter()
            .find(|constraint| constraint.provenance == pin.id)
            .ok_or("missing replacement pin")?;
        assert!(constraint.enforcement.is_empty());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires the approved installed OR-Tools worker; invocation is documented in the test handoff"]
async fn real_worker_diagnostic_family_matrix_and_signed_conditions() -> Result {
    let root = PathBuf::from(
        std::env::var_os("EUTHETO_TEST_ORTOOLS_ARTIFACT")
            .ok_or("EUTHETO_TEST_ORTOOLS_ARTIFACT must name the approved worker")?,
    );
    let manifest = std::fs::read(root.join("solver-manifest.json"))?;
    let digest_hex = eutheto_export::sha256_hex(&manifest);
    assert_eq!(
        digest_hex, "c8a177ac1405cb0edd364ba213ecbe8ac12d6ad05d9f474609d350158a106f31",
        "diagnostic matrix requires the pinned approved worker manifest"
    );
    // No environment fallback or skip is intentional: invoking this test without the approved
    // artifact must fail in solve_with_real_worker rather than produce unverified evidence.
    let cases = family_cases()?;
    for case in cases {
        let document: ScenarioDocument = serde_json::from_value(case.value.clone())?;
        let baseline = baseline_selection(case.name)?;
        let mut source_violation = false;
        for &rule in &case.rules {
            if !source_rule_satisfied(&document, &baseline, rule)? {
                source_violation = true;
                break;
            }
        }
        assert!(
            source_violation,
            "{}: fixture baseline does not independently violate any authored Required rule",
            case.name
        );
        let diagnostic = diagnostic_problem(&document, None)?;
        let impossible = solve_with_real_worker(diagnostic.clone()).await?;
        assert_eq!(
            impossible.outcome.termination,
            BackendTerminationReason::InfeasibilityClaimed,
            "{}: all diagnostic assumptions must retain the source contradiction",
            case.name
        );
        assert!(impossible.candidates.is_empty(), "{}", case.name);

        for dropped in &case.drop_sets {
            let relaxed = drop_assumptions(&diagnostic, dropped)?;
            let feasible = solve_with_real_worker(relaxed.clone()).await?;
            assert!(
                matches!(
                    feasible.outcome.termination,
                    BackendTerminationReason::OptimalityClaimed
                        | BackendTerminationReason::CandidateFound
                ),
                "{}: dropping {dropped:?} did not admit a candidate: {:?}",
                case.name,
                feasible.outcome.termination
            );
            let selected = selected_pairs(&relaxed, &feasible)?;
            // The normal source verifier is deliberately independent of diagnostic provenance and
            // guards.  At least one dropped source family must now be the sole violated family in
            // the candidate admitted by the native model.
            for target in dropped {
                assert!(
                    !source_rule_satisfied(&document, &selected, *target)?,
                    "{}: dropped rule {target} unexpectedly satisfied",
                    case.name
                );
            }
            assert!(!original_accepts(&document, &selected)?);
        }
    }
    // Exercise two helper boundaries with the native worker as well as the strict contradictions
    // above: an exact directed travel allowance and an exact elapsed-hours cap are admitted.
    let mut transition_boundary = family_cases()?
        .into_iter()
        .find(|case| case.name == "transition-time")
        .ok_or("missing transition boundary case")?
        .value;
    transition_boundary["domain"]["entities"][id(22)]["startsAt"] = super::endpoint("10:30:00");
    transition_boundary["domain"]["entities"][id(22)]["endsAt"] = super::endpoint("12:30:00");
    let transition_document: ScenarioDocument = serde_json::from_value(transition_boundary)?;
    let transition_problem = diagnostic_problem(&transition_document, None)?;
    let transition_result = solve_with_real_worker(transition_problem.clone()).await?;
    assert!(matches!(
        transition_result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed | BackendTerminationReason::CandidateFound
    ));
    let transition_selected = selected_pairs(&transition_problem, &transition_result)?;
    assert!(original_accepts(
        &transition_document,
        &transition_selected
    )?);

    let mut hours_boundary = family_cases()?
        .into_iter()
        .find(|case| case.name == "maximum-hours")
        .ok_or("missing hours boundary case")?
        .value;
    hours_boundary["domain"]["rules"][id(37)]["maximumMinutes"] = json!(240);
    let hours_document: ScenarioDocument = serde_json::from_value(hours_boundary)?;
    let hours_problem = diagnostic_problem(&hours_document, None)?;
    let hours_result = solve_with_real_worker(hours_problem.clone()).await?;
    assert!(matches!(
        hours_result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed | BackendTerminationReason::CandidateFound
    ));
    let hours_selected = selected_pairs(&hours_problem, &hours_result)?;
    assert!(original_accepts(&hours_document, &hours_selected)?);

    let document: ScenarioDocument = serde_json::from_value(clean_value()?)?;
    for condition in [
        condition_force(1, 8, false)?,
        condition_force(1, 8, true)?,
        condition_forbid(1, 8, false)?,
        condition_forbid(1, 8, true)?,
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: assignment_id(1, 8)?,
            replaced_assignment_id: assignment_id(20, 8)?,
        })?,
    ] {
        let diagnostic = diagnostic_problem(&document, Some(&condition))?;
        let result = solve_with_real_worker(diagnostic.clone()).await?;
        assert!(
            matches!(
                result.outcome.termination,
                BackendTerminationReason::OptimalityClaimed
                    | BackendTerminationReason::CandidateFound
            ),
            "signed condition {:?} was not admitted by the native worker",
            condition.condition
        );
        let selected = selected_pairs(&diagnostic, &result)?;
        let target_person = id(1).parse()?;
        let target_shift = id(8).parse()?;
        let replacement_person = id(20).parse()?;
        match condition.condition {
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                value: AssignmentValue::Boolean(value),
                ..
            } => {
                let target = selected
                    .iter()
                    .any(|pair| pair.person_id == target_person && pair.shift_id == target_shift);
                assert_eq!(target, value);
            }
            CounterfactualConditionPayloadV1::ForbidAssignmentValue {
                value: AssignmentValue::Boolean(value),
                ..
            } => {
                let target = selected
                    .iter()
                    .any(|pair| pair.person_id == target_person && pair.shift_id == target_shift);
                assert_eq!(target, !value);
            }
            CounterfactualConditionPayloadV1::ForbidAssignmentValue { .. } => {
                return Err("unexpected non-boolean forbid condition".into());
            }
            CounterfactualConditionPayloadV1::ReplaceAssignment { .. } => {
                assert!(
                    selected.iter().any(
                        |pair| pair.person_id == target_person && pair.shift_id == target_shift
                    )
                );
                assert!(
                    !selected
                        .iter()
                        .any(|pair| pair.person_id == replacement_person
                            && pair.shift_id == target_shift)
                );
            }
            CounterfactualConditionPayloadV1::ForceAssignmentValue { .. } => {
                return Err("unexpected non-boolean force condition".into());
            }
        }
    }
    Ok(())
}
