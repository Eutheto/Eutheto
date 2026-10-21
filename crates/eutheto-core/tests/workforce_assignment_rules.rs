#![forbid(unsafe_code)]

//! Cross-layer WF-003 through WF-007 evidence; production pack registration remains out of scope.

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

#[path = "workforce_assignment_rules/authority.rs"]
mod authority;

#[path = "workforce_assignment_rules/diagnostic_matrix.rs"]
mod diagnostic_matrix;

#[path = "workforce_assignment_rules/diagnostic_copy.rs"]
mod diagnostic_copy;

use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::AssignmentValue;
use eutheto_planning_ir::{
    BoolVariableId, CompilerId, Constraint, Literal, ObjectivePlan, PLANNING_IR_SCHEMA_VERSION,
    PROJECTION_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningMetadata, PlanningProblem,
    ProvenanceParameter, ProvenanceSourceKind, Variable, canonical_ir_hash, feature_usage,
    project_candidate, summarize,
};
use eutheto_solver_api::{
    BackendSolveResult, BackendTerminationReason, BoundedBackendOutput, ProgressSink,
    SolveProgressEvent, SolveRequest, SolverApiLimits, validate_outcome,
};
use eutheto_solver_ortools::{
    ORTOOLS_ADAPTER_VERSION, ORTOOLS_BACKEND_ID, ORTOOLS_VERSION, VerifiedWorkerArtifact,
    registry_with_ortools,
};
use eutheto_types::{
    BackendSelection, CancellationToken, DurationMillis, ExplanationMode, ParentSolveBudget,
    PreservationPolicy, ReproducibilityMode, ResourceLimits, RuleId, ScenarioDocument, SolveMode,
    SolveOptions, SystemMonotonicClock, WorkerThreadPolicy,
};
use eutheto_workforce::{
    assignment_rules::{
        AssignmentRuleCompilation, analyze_assignments, compile_assignment_rules,
        compile_workforce, evaluate_assignment_rules,
    },
    model::AssignmentPair,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    path::PathBuf,
    sync::Arc,
};
use workforce_fixture::id;

type TestResult = Result<(), Box<dyn Error>>;

fn fixture_value() -> Result<Value, Box<dyn Error>> {
    let mut value = serde_json::to_value(workforce_fixture::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["lockedAssignments"] = json!({});
    let entities = value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?;
    entities.remove(&id(6));
    let mut second = entities[&id(1)].clone();
    second["id"] = json!(id(20));
    second["externalId"] = json!("staff-02");
    second["qualificationGrants"] = json!([
        {"qualificationId":id(11),"expiresAt":"2026-11-01T09:30:00Z"},
        {"qualificationId":id(11),"effectiveFrom":"2026-11-01T09:30:00Z","expiresAt":"2026-11-01T11:00:00Z"}
    ]);
    entities.insert(id(20), second);
    let mut third = entities[&id(1)].clone();
    third["id"] = json!(id(21));
    third["externalId"] = json!("staff-03");
    third["qualificationGrants"] = json!([]);
    entities.insert(id(21), third);
    let mut shift = entities[&id(8)].clone();
    shift["startsAt"] = endpoint("08:00:00");
    shift["endsAt"] = endpoint("10:00:00");
    entities.insert(id(8), shift.clone());
    shift["id"] = json!(id(22));
    shift["startsAt"] = endpoint("09:00:00");
    shift["endsAt"] = endpoint("11:00:00");
    entities.insert(id(22), shift);
    entities.insert(id(23), json!({
        "kind":"availability", "id":id(23), "personId":id(20),
        "availabilityKind":"unavailable",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z","endsAt":"2026-11-01T08:30:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"manual", "note":""
    }));
    let mut rules = serde_json::Map::new();
    for (index, kind) in [
        (30, "eligibility"),
        (31, "availability"),
        (32, "coverage"),
        (33, "noOverlap"),
    ] {
        let mut rule = json!({"id":id(index),"kind":kind,"active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
        if kind == "noOverlap" {
            rule["compatibleCategoryPairs"] = json!([]);
        }
        rules.insert(id(index), rule);
    }
    value["domain"]["rules"] = Value::Object(rules);
    Ok(value)
}

fn rest_fixture() -> Result<Value, Box<dyn Error>> {
    let mut value = fixture_value()?;
    let mut call = value["domain"]["entities"][id(4)].clone();
    call["id"] = json!(id(24));
    call["name"] = json!("Call");
    call["category"] = json!("call");
    value["domain"]["entities"][id(24)] = call;
    value["domain"]["entities"][id(8)]["assignmentTypeId"] = json!(id(24));
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint("19:59:59.999999999");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint("21:00:00");
    for person in [1, 20, 21] {
        value["domain"]["entities"][id(person)]["eligibleAssignmentTypeIds"] =
            json!([id(4), id(24)]);
    }
    value["domain"]["entities"][id(20)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    value["domain"]["rules"][id(34)] = json!({
        "id":id(34),"kind":"minimumRest","active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},
        "afterScope":{"people":{"kind":"all"},"categories":["call"]},
        "beforeScope":{"people":{"kind":"all"},"categories":["clinic"]},
        "minimumMinutes":600
    });
    Ok(value)
}

fn endpoint(time: &str) -> Value {
    json!({"instant":format!("2026-11-01T{time}Z"),"local":format!("2026-11-01T{time}"),"offsetSeconds":0})
}

fn endpoint_on(date: &str, time: &str) -> Value {
    json!({"instant":format!("{date}T{time}Z"),"local":format!("{date}T{time}"),"offsetSeconds":0})
}

fn consecutive_fixture() -> Result<Value, Box<dyn Error>> {
    let mut value = fixture_value()?;
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-05T00:00:00Z"});
    value["domain"]["rules"][id(33)]["active"] = json!(false);
    value["domain"]["entities"][id(8)]["startsAt"] = endpoint("08:00:00");
    value["domain"]["entities"][id(8)]["endsAt"] = endpoint("09:00:00");
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint("09:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint("10:00:00");
    let mut third = value["domain"]["entities"][id(22)].clone();
    third["id"] = json!(id(25));
    third["startsAt"] = endpoint("10:00:00");
    third["endsAt"] = endpoint("11:00:00");
    value["domain"]["entities"][id(25)] = third;
    for (lock, shift) in [(40, 8), (41, 22), (42, 25)] {
        value["domain"]["lockedAssignments"][id(lock)] = json!({
            "id":id(lock),"personId":id(1),"shiftId":id(shift),"state":{"kind":"hard"}
        });
    }
    value["domain"]["rules"][id(35)] = json!({
        "kind":"maximumConsecutive","id":id(35),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},"mode":{"kind":"assignments","breakMinutes":0},
        "maximum":2
    });
    Ok(value)
}

fn context() -> CompileContext {
    CompileContext {
        objective_context: None,
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair, Box<dyn Error>> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

// The universe deliberately includes pairs pruned by the compiler.
fn selections() -> Result<Vec<Vec<AssignmentPair>>, Box<dyn Error>> {
    let mut universe = Vec::new();
    for person in [1, 20, 21] {
        for shift in [8, 22] {
            universe.push(pair(person, shift)?);
        }
    }
    Ok((0..64)
        .map(|mask| {
            universe
                .iter()
                .enumerate()
                .filter_map(|(index, pair)| (mask & (1 << index) != 0).then_some(*pair))
                .collect()
        })
        .collect())
}

fn literal_value(
    literal: &Literal,
    values: &BTreeMap<BoolVariableId, bool>,
) -> Result<bool, Box<dyn Error>> {
    Ok(*values
        .get(&literal.variable)
        .ok_or("undeclared Boolean in contribution")?
        == literal.positive)
}

// This interpreter knows primitive mathematics, never Workforce predicates.
fn mathematics_accepts(
    compiled: &AssignmentRuleCompilation,
    selected: &[AssignmentPair],
) -> Result<bool, Box<dyn Error>> {
    let available: BTreeSet<_> = compiled.variables.iter().map(|item| item.pair).collect();
    if selected.iter().any(|pair| !available.contains(pair)) {
        return Ok(false);
    }
    let values: BTreeMap<_, _> = compiled
        .variables
        .iter()
        .map(|item| (item.variable.id.clone(), selected.contains(&item.pair)))
        .collect();
    for record in &compiled.constraints {
        let mut enforced = true;
        for literal in &record.enforcement {
            if !literal_value(literal, &values)? {
                enforced = false;
                break;
            }
        }
        if !enforced {
            continue;
        }
        let (literals, minimum, maximum) = match &record.body {
            Constraint::BoolOr { literals } => (literals, 1, u64::MAX),
            Constraint::BoolAnd { literals } => {
                (literals, u64::try_from(literals.len())?, u64::MAX)
            }
            Constraint::AtMostOne { literals } => (literals, 0, 1),
            Constraint::ExactlyOne { literals } => (literals, 1, 1),
            Constraint::CardinalityRange { literals, min, max } => (literals, *min, *max),
            _ => return Err("unexpected WF-003 mathematical primitive".into()),
        };
        let mut count = 0_u64;
        for literal in literals {
            count += u64::from(literal_value(literal, &values)?);
        }
        if count < minimum || count > maximum {
            return Ok(false);
        }
    }
    Ok(true)
}

fn original_accepts(
    document: &ScenarioDocument,
    selected: &[AssignmentPair],
) -> Result<bool, Box<dyn Error>> {
    let result = evaluate_assignment_rules(document, selected, None)?;
    assert!(
        result.obligations.remaining.is_empty(),
        "fixture must not hide unimplemented obligations"
    );
    assert_eq!(
        result
            .evaluations
            .iter()
            .map(|item| item.rule_id)
            .collect::<Vec<_>>(),
        result.obligations.handled
    );
    let report = authority::verify_selection(document, selected)?;
    assert_eq!(
        report.accepted,
        result.evaluations.iter().all(|item| item.satisfied)
    );
    Ok(report.accepted)
}

fn qualification_coverage_fixture() -> Result<Value, Box<dyn Error>> {
    let mut value = fixture_value()?;
    value["domain"]["rules"][id(30)]["active"] = json!(false);
    for shift in [8, 22] {
        value["domain"]["entities"][id(shift)]["coverage"]["qualificationMinimums"] = json!([
            {"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":1}
        ]);
    }
    Ok(value)
}

#[test]
fn coverage_and_overlap_mathematical_identities_match_frozen_vectors() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(fixture_value()?)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    // Fixed typed-key vectors: headcount(rule 32, instance/shift 8, 1..=1) and
    // no_overlap(rule 33, person 1, shifts 8/22), using the v1 domain separator.
    for (constraint, provenance) in [
        (
            "official.workforce.constraint.bf8b31fe7318d39dda1417741c4982815edb68097354db1c91f96a541f13ad46",
            "official.workforce.provenance.4c4935a84b783193c7da1feedfd50042017551a89a1be23fb34bb298c38b7e8d",
        ),
        (
            "official.workforce.constraint.eeb801ef7bd80fbb48420ec74ce634b5bd3de7cc9fe10652075b1d5613c7dbc1",
            "official.workforce.provenance.5adf6387fc1669c6d5e259227ec9188d275af4f12e0417fdfecf89ac067bdcdd",
        ),
    ] {
        let record = compiled
            .constraints
            .iter()
            .find(|record| record.id.as_str() == constraint)
            .ok_or("mathematical identity vector changed")?;
        assert_eq!(record.provenance.as_str(), provenance);
    }
    Ok(())
}

#[test]
fn exhaustive_original_domain_and_compiled_mathematics_agree() -> TestResult {
    let base = fixture_value()?;
    let mut touching = base.clone();
    touching["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    let mut compatible = base.clone();
    compatible["domain"]["rules"][id(33)]["compatibleCategoryPairs"] =
        json!([{"firstCategory":"clinic","secondCategory":"clinic"}]);
    let mut expiry_gap = base.clone();
    expiry_gap["domain"]["entities"][id(20)]["qualificationGrants"][1]["effectiveFrom"] =
        json!("2026-11-01T09:30:00.000000001Z");
    let mut wrong_type = base.clone();
    wrong_type["domain"]["entities"][id(21)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    wrong_type["domain"]["entities"][id(21)]["eligibleAssignmentTypeIds"] = json!([]);
    let mut inactive_eligibility = base.clone();
    inactive_eligibility["domain"]["rules"][id(30)]["active"] = json!(false);
    let mut approved_pto = base.clone();
    approved_pto["domain"]["rules"][id(31)]["active"] = json!(false);
    approved_pto["domain"]["entities"][id(23)]["availabilityKind"] = json!("approvedTimeOff");
    let mut outside_effective = base.clone();
    outside_effective["domain"]["entities"][id(23)]["effectiveRange"] =
        json!({"startDate":"2026-11-02","endDateExclusive":"2026-11-03"});
    for (name, value, expected_count) in [
        ("renewal and expiry equality", base, 1),
        ("touching", touching, 2),
        ("compatible overlap", compatible, 2),
        ("nanosecond qualification gap", expiry_gap, 0),
        ("type restriction", wrong_type, 1),
        ("inactive eligibility", inactive_eligibility, 4),
        ("unconditional approved PTO", approved_pto, 1),
        ("effective clipping", outside_effective, 2),
        (
            "qualification subset without eligibility",
            qualification_coverage_fixture()?,
            1,
        ),
    ] {
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_assignment_rules(&document, &context())?;
        assert!(compiled.obligations.remaining.is_empty());
        summarize(&problem(&document, &compiled)?, PlanningIrLimitsV1::DEFAULT)?;
        let mut accepted = 0;
        for selected in selections()? {
            let mathematical = mathematics_accepts(&compiled, &selected)?;
            let original = original_accepts(&document, &selected)?;
            assert_eq!(mathematical, original, "{name}: {selected:?}");
            accepted += usize::from(original);
        }
        assert_eq!(accepted, expected_count, "{name}");
    }
    Ok(())
}

#[test]
fn exhaustive_rest_mathematics_matches_original_direction_and_boundary() -> TestResult {
    let base = rest_fixture()?;
    let mut equal = base.clone();
    equal["domain"]["entities"][id(22)]["startsAt"] = endpoint("20:00:00");
    let mut above = base.clone();
    above["domain"]["entities"][id(22)]["startsAt"] = endpoint("20:00:00.000000001");
    let mut reverse = base.clone();
    reverse["domain"]["rules"][id(34)]["afterScope"]["categories"] = json!(["clinic"]);
    reverse["domain"]["rules"][id(34)]["beforeScope"]["categories"] = json!(["call"]);
    let mut inactive = base.clone();
    inactive["domain"]["rules"][id(34)]["active"] = json!(false);
    let mut empty = base.clone();
    empty["domain"]["rules"][id(34)]["scope"]["people"] =
        json!({"kind":"selected","personIds":[id(21)]});
    for (name, value, expected) in [
        ("one nanosecond short", base, &[9_usize][..]),
        ("exact ten hours", equal, &[3, 9][..]),
        ("one nanosecond above", above, &[3, 9][..]),
        ("reverse chronological roles", reverse, &[3, 9][..]),
        ("inactive rest", inactive, &[3, 9][..]),
        ("no eligible scoped person", empty, &[3, 9][..]),
    ] {
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_assignment_rules(&document, &context())?;
        assert!(compiled.obligations.remaining.is_empty());
        summarize(&problem(&document, &compiled)?, PlanningIrLimitsV1::DEFAULT)?;
        let mut accepted = Vec::new();
        for (mask, selected) in selections()?.into_iter().enumerate() {
            let original = original_accepts(&document, &selected)?;
            assert_eq!(
                mathematics_accepts(&compiled, &selected)?,
                original,
                "{name}: {selected:?}"
            );
            if original {
                accepted.push(mask);
            }
        }
        assert_eq!(accepted, expected, "{name}");
    }
    Ok(())
}

#[test]
fn original_rest_evaluator_detects_missing_edge_threshold_and_direction_mutants() -> TestResult {
    let original = rest_fixture()?;
    let document: ScenarioDocument = serde_json::from_value(original.clone())?;
    let mut missing = compile_assignment_rules(&document, &context())?;
    missing
        .constraints
        .retain(|record| !matches!(record.body, Constraint::AtMostOne { .. }));
    detects_mutation(&document, &missing)?;
    let mut threshold = original.clone();
    threshold["domain"]["rules"][id(34)]["minimumMinutes"] = json!(599);
    let threshold = serde_json::from_value(threshold)?;
    detects_mutation(
        &document,
        &compile_assignment_rules(&threshold, &context())?,
    )?;
    let mut direction = original;
    direction["domain"]["rules"][id(34)]["afterScope"]["categories"] = json!(["clinic"]);
    direction["domain"]["rules"][id(34)]["beforeScope"]["categories"] = json!(["call"]);
    let direction = serde_json::from_value(direction)?;
    detects_mutation(
        &document,
        &compile_assignment_rules(&direction, &context())?,
    )
}

#[test]
fn original_rest_evaluator_detects_nonadjacent_and_equal_start_omissions() -> TestResult {
    let mut nonadjacent = rest_fixture()?;
    nonadjacent["domain"]["rules"][id(32)]["active"] = json!(false);
    let mut admin = nonadjacent["domain"]["entities"][id(4)].clone();
    admin["id"] = json!(id(26));
    admin["category"] = json!("administrative");
    nonadjacent["domain"]["entities"][id(26)] = admin;
    nonadjacent["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] =
        json!([id(4), id(24), id(26)]);
    let mut middle = nonadjacent["domain"]["entities"][id(22)].clone();
    middle["id"] = json!(id(25));
    middle["assignmentTypeId"] = json!(id(26));
    middle["startsAt"] = endpoint("12:00:00");
    middle["endsAt"] = endpoint("13:00:00");
    nonadjacent["domain"]["entities"][id(25)] = middle;
    let mut equal = rest_fixture()?;
    equal["domain"]["rules"][id(32)]["active"] = json!(false);
    equal["domain"]["entities"][id(22)]["startsAt"] = endpoint("08:00:00");
    equal["domain"]["entities"][id(22)]["endsAt"] = endpoint("09:00:00");
    equal["domain"]["rules"][id(33)]["compatibleCategoryPairs"] =
        json!([{"firstCategory":"call","secondCategory":"clinic"}]);
    // Source has the larger UUID: chronological equality must not use ID orientation.
    equal["domain"]["rules"][id(34)]["afterScope"]["categories"] = json!(["clinic"]);
    equal["domain"]["rules"][id(34)]["beforeScope"]["categories"] = json!(["call"]);
    for (value, selected) in [
        (nonadjacent, vec![pair(1, 8)?, pair(1, 25)?, pair(1, 22)?]),
        (equal, vec![pair(1, 8)?, pair(1, 22)?]),
    ] {
        let document = serde_json::from_value(value)?;
        let mut mutant = compile_assignment_rules(&document, &context())?;
        assert!(!mathematics_accepts(&mutant, &selected)?);
        assert!(!original_accepts(&document, &selected)?);
        let variables: BTreeSet<_> = mutant
            .variables
            .iter()
            .filter(|item| selected.contains(&item.pair))
            .map(|item| item.variable.id.clone())
            .collect();
        mutant.constraints.retain(|record| match &record.body {
            Constraint::AtMostOne { literals } => !literals
                .iter()
                .all(|literal| variables.contains(&literal.variable)),
            _ => true,
        });
        assert!(mathematics_accepts(&mutant, &selected)?);
        assert!(!original_accepts(&document, &selected)?);
    }
    Ok(())
}

#[test]
fn original_evaluator_detects_removed_skill_mix_constraints_without_other_rule_failure()
-> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(12)] = json!({
        "kind":"qualification", "id":id(12), "name":"Specialist", "description":""
    });
    value["domain"]["entities"][id(20)]["qualificationGrants"]
        .as_array_mut()
        .ok_or("grants")?
        .push(json!({"qualificationId":id(12)}));
    value["domain"]["entities"][id(21)]["qualificationGrants"] = json!([
        {"qualificationId":id(11)}, {"qualificationId":id(12)}
    ]);
    value["domain"]["rules"][id(34)] = json!({
        "kind":"requiredSkillMix", "id":id(34), "active":true, "strength":"required",
        "scope":{"people":{"kind":"all"}},
        "qualificationMinimums":[{
            "qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},
            "minimum":1
        }]
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let selected = [pair(1, 8)?, pair(20, 22)?];

    let report = authority::verify_selection(&document, &selected)?;
    assert!(!report.accepted);
    for rule in [30, 31, 32, 33] {
        let rule_id: RuleId = id(rule).parse()?;
        let evaluation = report
            .required_rule_results
            .iter()
            .find(|evaluation| evaluation.rule_id == rule_id)
            .ok_or("missing non-skill-mix evaluation")?;
        assert!(
            evaluation.satisfied,
            "selected candidate unexpectedly violates rule {rule}"
        );
    }
    let skill_mix_rule_id: RuleId = id(34).parse()?;
    let skill_mix_evaluation = report
        .required_rule_results
        .iter()
        .find(|evaluation| evaluation.rule_id == skill_mix_rule_id)
        .ok_or("missing skill-mix evaluation")?;
    assert!(!skill_mix_evaluation.satisfied);
    assert!(!original_accepts(&document, &selected)?);

    let mut skill_mix_provenance = BTreeSet::new();
    for constraint in &compiled.constraints {
        let provenance = compiled
            .provenance
            .iter()
            .find(|record| record.id == constraint.provenance)
            .ok_or("missing constraint provenance")?;
        if provenance.source_kind != ProvenanceSourceKind::Derived
            || provenance.source_id != id(34)
            || provenance.message_key
                != "official.workforce.required_skill_mix_qualification_minimum"
        {
            continue;
        }
        assert_eq!(
            provenance.parameters.get("minimum"),
            Some(&ProvenanceParameter::Integer(1))
        );
        assert_eq!(
            provenance.parameters.get("source_row"),
            Some(&ProvenanceParameter::Integer(0))
        );
        let parent_id = provenance.parent.as_ref().ok_or("skill-mix parent")?;
        let parent = compiled
            .provenance
            .iter()
            .find(|record| &record.id == parent_id)
            .ok_or("skill-mix rule provenance")?;
        assert_eq!(parent.source_kind, ProvenanceSourceKind::RequiredRule);
        assert_eq!(parent.source_id, id(34));
        assert_eq!(parent.message_key, "official.workforce.required_skill_mix");
        skill_mix_provenance.insert(constraint.provenance.clone());
    }
    assert_eq!(skill_mix_provenance.len(), 2);

    assert!(!mathematics_accepts(&compiled, &selected)?);
    let mut mutant = compiled.clone();
    mutant
        .constraints
        .retain(|constraint| !skill_mix_provenance.contains(&constraint.provenance));
    assert_eq!(
        mutant.constraints.len(),
        compiled.constraints.len() - skill_mix_provenance.len()
    );
    assert!(mathematics_accepts(&mutant, &selected)?);
    assert!(!original_accepts(&document, &selected)?);
    Ok(())
}
#[test]
fn original_evaluator_quarantines_removed_fixed_and_hard_lock_constraints() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["rules"] = json!({
        (id(34)): {
            "kind":"fixedAssignment","id":id(34),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"personId":id(1),"shiftId":id(8)
        }
    });
    value["domain"]["lockedAssignments"] = json!({
        (id(35)): {
            "id":id(35),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}
        }
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let selected = [];
    assert!(!mathematics_accepts(&compiled, &selected)?);
    let report = authority::verify_selection(&document, &selected)?;
    assert!(!report.accepted);
    for owner in [34, 35] {
        let rule_id: RuleId = id(owner).parse()?;
        assert!(
            !report
                .required_rule_results
                .iter()
                .find(|item| item.rule_id == rule_id)
                .ok_or("missing original forced-pair obligation")?
                .satisfied
        );
    }
    let accepted = authority::verify_selection(&document, &[pair(1, 8)?])?;
    assert!(accepted.accepted);
    for owner in [34, 35] {
        let rule_id: RuleId = id(owner).parse()?;
        assert!(
            accepted
                .required_rule_results
                .iter()
                .find(|item| item.rule_id == rule_id)
                .ok_or("missing accepted forced-pair obligation")?
                .satisfied
        );
    }
    let mut mutant = compiled.clone();
    mutant.constraints.clear();
    assert!(mathematics_accepts(&mutant, &selected)?);
    assert!(!original_accepts(&document, &selected)?);
    Ok(())
}

fn detects_mutation(document: &ScenarioDocument, mutant: &AssignmentRuleCompilation) -> TestResult {
    for selected in selections()? {
        if mathematics_accepts(mutant, &selected)? && !original_accepts(document, &selected)? {
            return Ok(());
        }
    }
    Err("independent evaluator failed to detect weakened compiler mathematics".into())
}

fn mutual_fixture(mode: &str) -> Result<Value, Box<dyn Error>> {
    let mut value = fixture_value()?;
    value["domain"]["rules"][id(32)]["active"] = json!(false);
    value["domain"]["rules"][id(33)]["active"] = json!(false);
    value["domain"]["rules"][id(34)] = json!({
        "kind":"mutualAssignmentRestriction", "id":id(34), "active":true,
        "strength":"required", "scope":{"people":{"kind":"all"}},
        "personIds":[id(1),id(20)], "mode":mode
    });
    Ok(value)
}

fn transition_fixture() -> Result<Value, Box<dyn Error>> {
    let mut value = fixture_value()?;
    value["domain"]["rules"][id(32)]["active"] = json!(false);
    value["domain"]["rules"][id(33)]["active"] = json!(false);
    value["domain"]["entities"][id(40)] =
        json!({"kind":"location","id":id(40),"name":"South","transitions":[]});
    value["domain"]["entities"][id(5)]["transitions"] = json!([{"locationId":id(40),"minutes":30}]);
    value["domain"]["entities"][id(22)]["locationId"] = json!(id(40));
    value["domain"]["rules"][id(34)] = json!({
        "kind":"transitionTime","id":id(34),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},"locationIds":[id(5),id(40)]
    });
    Ok(value)
}

#[test]
fn transition_admission_rejects_empty_active_or_unknown_locations_but_keeps_inactive_raw()
-> TestResult {
    let mut empty = transition_fixture()?;
    empty["domain"]["rules"][id(34)]["locationIds"] = json!([]);
    let active: ScenarioDocument = serde_json::from_value(empty.clone())?;
    assert!(eutheto_workforce::validation::validate_document(&active).is_err());
    empty["domain"]["rules"][id(34)]["active"] = json!(false);
    let inactive: ScenarioDocument = serde_json::from_value(empty)?;
    eutheto_workforce::validation::validate_document(&inactive)?;
    let mut unknown = transition_fixture()?;
    unknown["domain"]["rules"][id(34)]["locationIds"] = json!([id(5), id(999)]);
    let missing: ScenarioDocument = serde_json::from_value(unknown)?;
    assert!(eutheto_workforce::validation::validate_document(&missing).is_err());
    Ok(())
}

#[test]
fn directed_transition_ir_and_original_verifier_agree_over_every_small_selection() -> TestResult {
    let short = transition_fixture()?;
    let mut exact = short.clone();
    exact["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:30:00");
    exact["domain"]["entities"][id(22)]["endsAt"] = endpoint("11:30:00");
    let mut touching_zero = short.clone();
    touching_zero["domain"]["entities"][id(5)]["transitions"][0]["minutes"] = json!(0);
    touching_zero["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    let mut missing = short.clone();
    missing["domain"]["entities"][id(5)]["transitions"] = json!([]);
    let mut reverse_only = missing.clone();
    reverse_only["domain"]["entities"][id(40)]["transitions"] =
        json!([{"locationId":id(5),"minutes":30}]);
    let mut tied_starts = reverse_only.clone();
    tied_starts["domain"]["entities"][id(22)]["startsAt"] = endpoint("08:00:00");
    let mut one_person_scoped = short.clone();
    one_person_scoped["domain"]["rules"][id(34)]["scope"]["people"] =
        json!({"kind":"selected","personIds":[id(20)]});
    let mut one_shift_scoped = short.clone();
    one_shift_scoped["domain"]["rules"][id(34)]["scope"]["locationIds"] = json!([id(5)]);
    let mut legacy_singleton = short.clone();
    legacy_singleton["domain"]["rules"][id(34)]["locationIds"] = json!([id(5)]);
    let mut fall_fold = short.clone();
    let original: Value = serde_json::to_value(workforce_fixture::fixture()?)?;
    fall_fold["settings"] = original["settings"].clone();
    fall_fold["domain"]["entities"][id(8)]["startsAt"] =
        original["domain"]["entities"][id(8)]["startsAt"].clone();
    fall_fold["domain"]["entities"][id(8)]["endsAt"] =
        original["domain"]["entities"][id(8)]["endsAt"].clone();
    fall_fold["domain"]["entities"][id(22)]["startsAt"] = json!({
        "instant":"2026-11-01T07:45:00Z","local":"2026-11-01T02:45:00","offsetSeconds":-18000
    });
    fall_fold["domain"]["entities"][id(22)]["endsAt"] = json!({
        "instant":"2026-11-01T08:45:00Z","local":"2026-11-01T03:45:00","offsetSeconds":-18000
    });
    let same_person = [pair(1, 8)?, pair(1, 22)?];
    let rule_id: RuleId = id(34).parse()?;
    for (name, value, accepted) in [
        ("short directed gap", short, false),
        ("exact directed gap", exact, true),
        ("touching zero-minute edge", touching_zero, true),
        ("missing edge", missing, true),
        ("reverse-only edge", reverse_only, true),
        ("equal starts inspect reverse direction", tied_starts, false),
        ("person scope intersection", one_person_scoped, true),
        ("shift scope intersection", one_shift_scoped, true),
        ("legacy singleton", legacy_singleton, true),
        ("fall fold uses instants", fall_fold, false),
    ] {
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_assignment_rules(&document, &context())?;
        assert!(compiled.obligations.handled.contains(&rule_id), "{name}");
        assert!(compiled.obligations.remaining.is_empty(), "{name}");
        summarize(&problem(&document, &compiled)?, PlanningIrLimitsV1::DEFAULT)?;
        assert_eq!(
            original_accepts(&document, &same_person)?,
            accepted,
            "{name}"
        );
        for selected in selections()? {
            assert_eq!(
                mathematics_accepts(&compiled, &selected)?,
                original_accepts(&document, &selected)?,
                "{name}: {selected:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn directed_transition_constraint_names_both_configured_locations_in_provenance() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(transition_fixture()?)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let provenance = compiled
        .provenance
        .iter()
        .find(|record| {
            record.source_kind == ProvenanceSourceKind::Derived
                && record.message_key == "official.workforce.transition_time"
        })
        .ok_or("missing directed travel constraint provenance")?;
    assert_eq!(provenance.source_kind, ProvenanceSourceKind::Derived);
    assert_eq!(provenance.source_id, id(34));
    for (key, kind, identity) in [
        ("source_location", "location", id(5)),
        ("target_location", "location", id(40)),
        ("source_shift", "shift", id(8)),
        ("target_shift", "shift", id(22)),
    ] {
        let Some(ProvenanceParameter::Entity(reference)) = provenance.parameters.get(key) else {
            return Err(format!("missing typed {key} provenance").into());
        };
        assert_eq!(
            reference.kind.as_str(),
            format!("official.workforce.{kind}")
        );
        assert_eq!(
            reference.id.as_str(),
            format!("official.workforce.{identity}")
        );
        assert!(
            provenance.entity_refs.contains(reference),
            "{key} must also be an affected entity"
        );
    }
    assert_eq!(
        provenance.parameters.get("required_minutes"),
        Some(&ProvenanceParameter::Integer(30))
    );
    let parent = compiled
        .provenance
        .iter()
        .find(|record| Some(&record.id) == provenance.parent.as_ref())
        .ok_or("missing travel rule provenance parent")?;
    assert_eq!(parent.source_kind, ProvenanceSourceKind::RequiredRule);
    assert_eq!(parent.source_id, id(34));
    Ok(())
}

#[test]
fn transition_command_inverse_and_portable_roundtrip_preserve_directed_matrix_meaning() -> TestResult
{
    use eutheto_domain_api::{
        DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPack, PortableImportContext,
    };
    use eutheto_types::{DomainCommandEnvelope, ScenarioDomain};
    use eutheto_workforce::{WorkforcePack, commands};

    let original: ScenarioDocument = serde_json::from_value(transition_fixture()?)?;
    let selected = [pair(1, 8)?, pair(1, 22)?];
    assert!(!original_accepts(&original, &selected)?);
    let mut relaxed = original
        .domain
        .rules
        .get(&id(34).parse()?)
        .ok_or("missing transition rule")?
        .clone();
    relaxed["locationIds"] = json!([id(5)]);
    let change = WorkforcePack.apply_batch(
        &original,
        &DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: original.domain_pack.id.clone(),
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: commands::UPDATE_RULE.to_owned(),
                payload: json!({"rule":relaxed}),
            }],
        },
        &CancellationToken::new(),
    )?;
    assert!(original_accepts(&change.document, &selected)?);
    let undone =
        WorkforcePack.apply_batch(&change.document, &change.inverse, &CancellationToken::new())?;
    assert_eq!(undone.document, original);
    let portable = WorkforcePack.export_portable(&original)?;
    let mut shell = original.clone();
    shell.domain = ScenarioDomain::default();
    shell.extensions.clear();
    let restored = WorkforcePack.import_portable(
        &serde_json::from_slice(&serde_json::to_vec(&portable)?)?,
        &PortableImportContext {
            scenario_shell: shell,
        },
    )?;
    assert_eq!(restored, original);
    assert!(!original_accepts(&restored, &selected)?);
    Ok(())
}

#[test]
fn transition_reports_original_hard_lock_conflicts_even_when_candidate_pruned() -> TestResult {
    let mut value = transition_fixture()?;
    value["domain"]["lockedAssignments"] = json!({
        (id(35)):{"id":id(35),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}},
        (id(36)):{"id":id(36),"personId":id(1),"shiftId":id(22),"state":{"kind":"hard"}}
    });
    value["domain"]["entities"][id(41)] = json!({
        "kind":"availability","id":id(41),"personId":id(1),
        "availabilityKind":"unavailable",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:15:00Z","endsAt":"2026-11-01T08:30:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"manual","note":""
    });
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let unavailable = pair(1, 8)?;
    assert!(
        !compiled
            .variables
            .iter()
            .any(|candidate| candidate.pair == unavailable)
    );
    let first_lock_id = id(35).parse()?;
    assert!(
        compiled.validation.issues.iter().any(|issue| {
            issue.code == "official.workforce.hard_lock_transition_conflict"
                && issue.field_path.as_deref()
                    == Some(format!("domain.lockedAssignments.{}", id(35)).as_str())
                && issue.resource == Some(eutheto_types::ResourceRef::Assignment(first_lock_id))
        }),
        "pruning must not hide the locked directed transition"
    );
    value["domain"]["rules"][id(34)]["locationIds"] = json!([id(5)]);
    let no_pair: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&no_pair, &context())?;
    assert!(
        compiled
            .validation
            .issues
            .iter()
            .all(|issue| issue.code != "official.workforce.hard_lock_transition_conflict")
    );
    Ok(())
}

#[test]
fn dense_directed_transition_conflicts_stop_at_constraint_budget() -> TestResult {
    let mut value = transition_fixture()?;
    let source = value["domain"]["entities"][id(8)].clone();
    let target = value["domain"]["entities"][id(22)].clone();
    for index in 1000..1016 {
        let mut shift = source.clone();
        shift["id"] = json!(id(index));
        value["domain"]["entities"][id(index)] = shift;
    }
    for index in 1100..1116 {
        let mut shift = target.clone();
        shift["id"] = json!(id(index));
        value["domain"]["entities"][id(index)] = shift;
    }
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let mut limited = context();
    limited.planning_limits.max_constraints = 32;
    let outcome = compile_assignment_rules(&document, &limited);
    assert!(
        matches!(
            outcome,
            Err(
                eutheto_workforce::assignment_rules::AssignmentRuleError::LimitExceeded(
                    eutheto_workforce::assignment_rules::AssignmentRuleLimit::Constraints
                )
            )
        ),
        "{outcome:?}"
    );
    Ok(())
}

#[test]
fn directed_transition_verification_quarantines_missing_constraint_mutant() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(transition_fixture()?)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let same_person = [pair(1, 8)?, pair(1, 22)?];
    let different_people = [pair(1, 8)?, pair(20, 22)?];
    assert!(!mathematics_accepts(&compiled, &same_person)?);
    assert!(!original_accepts(&document, &same_person)?);
    assert!(mathematics_accepts(&compiled, &different_people)?);
    assert!(original_accepts(&document, &different_people)?);
    let report = authority::verify_selection(&document, &same_person)?;
    let rule_id: RuleId = id(34).parse()?;
    let rule = report
        .required_rule_results
        .iter()
        .find(|entry| entry.rule_id == rule_id)
        .ok_or("missing transition evaluation")?;
    assert!(!rule.satisfied);
    for (kind, entity_id) in [
        ("person", id(1)),
        ("shift", id(8)),
        ("shift", id(22)),
        ("location", id(5)),
        ("location", id(40)),
    ] {
        assert!(
            rule.affected_entities.iter().any(|entity| {
                entity.kind.as_str() == format!("official.workforce.{kind}")
                    && entity.id.as_str() == format!("official.workforce.{entity_id}")
            }),
            "missing affected {kind}: {entity_id}"
        );
    }
    assert_eq!(
        rule.expected
            .get(&"official.workforce.fact.required_transition_minutes".parse()?),
        Some(&eutheto_domain_ir::VerificationValue::Integer(30))
    );
    assert_eq!(
        rule.observed
            .get(&"official.workforce.fact.actual_transition_seconds".parse()?),
        Some(&eutheto_domain_ir::VerificationValue::Integer(-3600))
    );
    let mut mutant = compiled.clone();
    mutant.constraints.clear();
    assert!(mathematics_accepts(&mutant, &same_person)?);
    assert!(!original_accepts(&document, &same_person)?);
    Ok(())
}

#[test]
fn mutual_modes_match_original_verifier_over_every_small_selection() -> TestResult {
    let same = mutual_fixture("sameShift")?;
    let overlap = mutual_fixture("overlappingShifts")?;
    let mut touching = overlap.clone();
    touching["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    let mut one_person_in_scope = overlap.clone();
    one_person_in_scope["domain"]["rules"][id(34)]["scope"]["people"] =
        json!({"kind":"selected","personIds":[id(1)]});
    let mut one_shift_in_scope = overlap.clone();
    one_shift_in_scope["domain"]["entities"][id(22)]["locationId"] = json!(id(40));
    one_shift_in_scope["domain"]["entities"][id(40)] =
        json!({"kind":"location","id":id(40),"name":"South","transitions":[]});
    one_shift_in_scope["domain"]["rules"][id(34)]["scope"]["locationIds"] = json!([id(40)]);
    for (name, value) in [
        ("same shift", same),
        ("overlapping", overlap),
        ("touching endpoints", touching),
        ("person scope intersection", one_person_in_scope),
        ("shift scope intersection", one_shift_in_scope),
    ] {
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_assignment_rules(&document, &context())?;
        assert!(compiled.obligations.handled.contains(&id(34).parse()?));
        assert!(compiled.obligations.remaining.is_empty());
        summarize(&problem(&document, &compiled)?, PlanningIrLimitsV1::DEFAULT)?;
        for selected in selections()? {
            assert_eq!(
                mathematics_accepts(&compiled, &selected)?,
                original_accepts(&document, &selected)?,
                "{name}: {selected:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn mutual_restriction_quarantines_cross_person_mutant_without_forbidding_same_person() -> TestResult
{
    let mutual_rule_id: RuleId = id(34).parse()?;
    let same = serde_json::from_value(mutual_fixture("sameShift")?)?;
    let overlap = serde_json::from_value(mutual_fixture("overlappingShifts")?)?;
    let two_on_one_shift = [pair(1, 22)?, pair(20, 22)?];
    let two_overlapping_shifts = [pair(1, 8)?, pair(20, 22)?];
    let one_person_two_shifts = [pair(1, 8)?, pair(1, 22)?];
    for (document, cross_shift_rejected) in [(&same, false), (&overlap, true)] {
        let compiled = compile_assignment_rules(document, &context())?;
        assert!(!mathematics_accepts(&compiled, &two_on_one_shift)?);
        assert!(!original_accepts(document, &two_on_one_shift)?);
        assert!(mathematics_accepts(&compiled, &one_person_two_shifts)?);
        assert!(original_accepts(document, &one_person_two_shifts)?);
        assert_eq!(
            mathematics_accepts(&compiled, &two_overlapping_shifts)?,
            !cross_shift_rejected
        );
        assert_eq!(
            original_accepts(document, &two_overlapping_shifts)?,
            !cross_shift_rejected
        );
        let report = authority::verify_selection(document, &two_on_one_shift)?;
        let rule = report
            .required_rule_results
            .iter()
            .find(|entry| entry.rule_id == mutual_rule_id)
            .ok_or("missing required mutual restriction evaluation")?;
        assert!(!rule.satisfied);
        for person in [1, 20] {
            assert!(rule.affected_entities.iter().any(|entity| {
                entity.kind.as_str() == "official.workforce.person"
                    && entity.id.as_str() == format!("official.workforce.{}", id(person))
            }));
        }
        let mut mutant = compiled.clone();
        mutant.constraints.clear();
        assert!(mathematics_accepts(&mutant, &two_on_one_shift)?);
        assert!(!original_accepts(document, &two_on_one_shift)?);
    }
    Ok(())
}

#[test]
fn legacy_singleton_mutual_restriction_stays_loadable_and_never_excludes_a_pair() -> TestResult {
    let mut value = mutual_fixture("overlappingShifts")?;
    for selected in [json!([]), json!([id(1)])] {
        value["domain"]["rules"][id(34)]["personIds"] = selected;
        let document: ScenarioDocument = serde_json::from_value(value.clone())?;
        compile_workforce(&document, &context())?;
        let compiled = compile_assignment_rules(&document, &context())?;
        assert!(compiled.obligations.handled.contains(&id(34).parse()?));
        assert!(original_accepts(&document, &[pair(1, 22)?, pair(20, 22)?])?);
    }
    Ok(())
}

#[test]
fn mutual_command_inverse_and_portable_roundtrip_preserve_rule_meaning() -> TestResult {
    use eutheto_domain_api::{
        DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPack, PortableImportContext,
    };
    use eutheto_types::{DomainCommandEnvelope, ScenarioDomain};
    use eutheto_workforce::{WorkforcePack, commands};

    let original: ScenarioDocument = serde_json::from_value(mutual_fixture("overlappingShifts")?)?;
    let selected = [pair(1, 8)?, pair(20, 22)?];
    assert!(!original_accepts(&original, &selected)?);
    let mut relaxed = original
        .domain
        .rules
        .get(&id(34).parse()?)
        .ok_or("missing mutual restriction")?
        .clone();
    relaxed["mode"] = json!("sameShift");
    let change = WorkforcePack.apply_batch(
        &original,
        &DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: original.domain_pack.id.clone(),
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: commands::UPDATE_RULE.to_owned(),
                payload: json!({"rule":relaxed}),
            }],
        },
        &CancellationToken::new(),
    )?;
    assert!(original_accepts(&change.document, &selected)?);
    let undone =
        WorkforcePack.apply_batch(&change.document, &change.inverse, &CancellationToken::new())?;
    assert_eq!(undone.document, original);
    let portable = WorkforcePack.export_portable(&original)?;
    let mut shell = original.clone();
    shell.domain = ScenarioDomain::default();
    shell.extensions.clear();
    let restored = WorkforcePack.import_portable(
        &serde_json::from_slice(&serde_json::to_vec(&portable)?)?,
        &PortableImportContext {
            scenario_shell: shell,
        },
    )?;
    assert_eq!(restored, original);
    assert!(!original_accepts(&restored, &selected)?);
    let compiled = compile_assignment_rules(&restored, &context())?;
    for selection in selections()? {
        assert_eq!(
            mathematics_accepts(&compiled, &selection)?,
            original_accepts(&restored, &selection)?
        );
    }
    Ok(())
}

#[test]
fn mutual_hard_lock_conflicts_are_reported_from_original_pairs_even_when_pruned() -> TestResult {
    let first_lock_id = id(35).parse()?;
    for (mode, first_shift, second_shift) in [
        ("sameShift", 22, 22),
        ("overlappingShifts", 8, 22),
        ("overlappingShifts", 8, 8),
    ] {
        let mut value = mutual_fixture(mode)?;
        value["domain"]["lockedAssignments"] = json!({
            (id(35)): {
                "id":id(35),"personId":id(1),"shiftId":id(first_shift),"state":{"kind":"hard"}
            },
            (id(36)): {
                "id":id(36),"personId":id(20),"shiftId":id(second_shift),"state":{"kind":"hard"}
            }
        });
        let document: ScenarioDocument = serde_json::from_value(value.clone())?;
        let compiled = compile_assignment_rules(&document, &context())?;
        assert!(
            compiled.validation.issues.iter().any(|issue| {
                issue.code == "official.workforce.hard_lock_mutual_conflict"
                    && issue.field_path.as_deref()
                        == Some(format!("domain.lockedAssignments.{}", id(35)).as_str())
                    && issue.resource == Some(eutheto_types::ResourceRef::Assignment(first_lock_id))
            }),
            "{mode} must report source-resolved locked conflict"
        );
        value["domain"]["rules"][id(34)]["scope"]["people"] =
            json!({"kind":"selected","personIds":[id(1)]});
        let without_pair: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_assignment_rules(&without_pair, &context())?;
        assert!(
            compiled
                .validation
                .issues
                .iter()
                .all(|issue| issue.code != "official.workforce.hard_lock_mutual_conflict")
        );
    }
    Ok(())
}

#[test]
fn mutual_dense_overlap_conflicts_fail_at_the_existing_constraint_budget() -> TestResult {
    let mut value = mutual_fixture("overlappingShifts")?;
    let mut people = vec![id(1), id(20)];
    let prototype_person = value["domain"]["entities"][id(1)].clone();
    for index in 101..109 {
        let mut person = prototype_person.clone();
        person["id"] = json!(id(index));
        person["externalId"] = json!(format!("synthetic-{index}"));
        value["domain"]["entities"][id(index)] = person;
        people.push(id(index));
    }
    let prototype_shift = value["domain"]["entities"][id(22)].clone();
    for index in 200..208 {
        let mut shift = prototype_shift.clone();
        shift["id"] = json!(id(index));
        value["domain"]["entities"][id(index)] = shift;
    }
    value["domain"]["rules"][id(34)]["personIds"] = json!(people);
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let mut limited = context();
    limited.planning_limits.max_constraints = 32;
    let outcome = compile_assignment_rules(&document, &limited);
    assert!(
        matches!(
            outcome,
            Err(
                eutheto_workforce::assignment_rules::AssignmentRuleError::LimitExceeded(
                    eutheto_workforce::assignment_rules::AssignmentRuleLimit::Constraints
                )
            )
        ),
        "{outcome:?}"
    );
    Ok(())
}

#[test]
fn original_evaluator_detects_pruned_eligibility_availability_and_structural_mutants() -> TestResult
{
    let document: ScenarioDocument = serde_json::from_value(fixture_value()?)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    // The first restored pair violates eligibility; the second intersects unavailable time.
    for rejected in [pair(21, 8)?, pair(20, 8)?] {
        let mut mutant = compiled.clone();
        let mut restored = mutant
            .variables
            .first()
            .ok_or("fixture has no assignment variables")?
            .clone();
        restored.pair = rejected;
        restored.variable.id = BoolVariableId::new("test.workforce.restored")?;
        mutant.variables.push(restored);
        detects_mutation(&document, &mutant)?;
    }
    let mut overlap = compiled.clone();
    overlap
        .constraints
        .retain(|record| !matches!(&record.body, Constraint::AtMostOne { .. }));
    detects_mutation(&document, &overlap)?;
    let mut lower = compiled;
    for record in &mut lower.constraints {
        match &mut record.body {
            Constraint::ExactlyOne { literals } => {
                record.body = Constraint::CardinalityRange {
                    literals: literals.clone(),
                    min: 0,
                    max: 1,
                };
            }
            Constraint::CardinalityRange { min, .. } => *min = 0,
            _ => {}
        }
    }
    detects_mutation(&document, &lower)?;
    let mut touching = fixture_value()?;
    touching["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    let touching: ScenarioDocument = serde_json::from_value(touching)?;
    let mut upper = compile_assignment_rules(&touching, &context())?;
    for record in &mut upper.constraints {
        match &mut record.body {
            Constraint::ExactlyOne { literals } => {
                record.body = Constraint::CardinalityRange {
                    literals: literals.clone(),
                    min: 1,
                    max: u64::try_from(literals.len())?,
                };
            }
            Constraint::CardinalityRange { literals, max, .. } => {
                *max = u64::try_from(literals.len())?;
            }
            _ => {}
        }
    }
    detects_mutation(&touching, &upper)
}

#[test]
fn original_evaluator_detects_headcount_substituted_for_qualification_count() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(qualification_coverage_fixture()?)?;
    let mut mutant = compile_assignment_rules(&document, &context())?;
    let pairs: BTreeMap<_, _> = mutant
        .variables
        .iter()
        .map(|item| (item.variable.id.clone(), item.pair))
        .collect();
    for record in &mut mutant.constraints {
        let (Constraint::CardinalityRange { literals, .. } | Constraint::ExactlyOne { literals }) =
            &mut record.body
        else {
            continue;
        };
        let Some(first) = literals.first() else {
            continue;
        };
        let shift = pairs
            .get(&first.variable)
            .ok_or("unknown constraint variable")?
            .shift_id;
        if literals
            .iter()
            .any(|literal| pairs[&literal.variable].shift_id != shift)
        {
            continue;
        }
        let full_population: Vec<_> = pairs
            .iter()
            .filter(|(_, pair)| pair.shift_id == shift)
            .map(|(variable, _)| Literal {
                variable: variable.clone(),
                positive: true,
            })
            .collect();
        if literals.len() < full_population.len() {
            *literals = full_population;
        }
    }
    detects_mutation(&document, &mutant)
}

fn problem(
    document: &ScenarioDocument,
    compiled: &AssignmentRuleCompilation,
) -> Result<PlanningProblem, Box<dyn Error>> {
    let mut problem = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: compiled
            .variables
            .iter()
            .map(|item| Variable::Boolean(item.variable.clone()))
            .chain(compiled.auxiliary_variables.iter().cloned())
            .collect(),
        constraints: compiled.constraints.clone(),
        objectives: ObjectivePlan::default(),
        assumptions: Vec::new(),
        projections: Vec::new(),
        provenance: compiled.provenance.clone(),
        metadata: PlanningMetadata {
            pack_id: document.domain_pack.id.clone(),
            scenario_id: document.scenario_id,
            scenario_revision: 1,
            projection_version: PROJECTION_SCHEMA_VERSION,
            compiler_id: CompilerId::new("test.workforce.assignment-rules")?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: BTreeMap::new(),
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::new(),
        split_authorization: None,
    };
    problem.declared_capabilities = feature_usage(&problem).required_capabilities();
    Ok(problem)
}

struct Progress;
impl ProgressSink for Progress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), eutheto_solver_api::OutputError> {
        Ok(())
    }
}

async fn solve_with_real_worker(
    problem: PlanningProblem,
) -> Result<BackendSolveResult, Box<dyn Error>> {
    let root = PathBuf::from(std::env::var_os("EUTHETO_TEST_ORTOOLS_ARTIFACT")
        .ok_or("EUTHETO_TEST_ORTOOLS_ARTIFACT must name an installed real OR-Tools artifact; no helper fallback")?);
    let bytes = std::fs::read(root.join("solver-manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    if manifest["approval"].is_null()
        || manifest["backend_source"]["sha256"]
            .as_str()
            .is_none_or(|value| value.len() != 64)
    {
        return Err("Workforce evidence requires an approved real worker artifact".into());
    }
    let artifact = VerifiedWorkerArtifact::verify(root, Sha256::digest(&bytes).into()).await?;
    let registry = registry_with_ortools(artifact)?;
    let backend_id = ORTOOLS_BACKEND_ID.parse()?;
    let backend = registry
        .get(&backend_id)
        .ok_or("missing OR-Tools backend")?;
    let problem = Arc::new(problem);
    let summary = summarize(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let options = SolveOptions {
        backend: BackendSelection::Specific(backend_id.clone()),
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(5_000)?,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 1,
        solution_limit: None,
        stop_after_first_feasible: false,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::None,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 100,
            max_rules: 100,
            max_variables: 100,
            max_constraints: 100,
        },
    };
    assert!(
        backend.compatibility(&summary, &options).compatible(),
        "real-worker test model unsupported: {:?}",
        backend.compatibility(&summary, &options)
    );
    let budget = ParentSolveBudget::new(
        options.time_limit_milliseconds,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    let request = SolveRequest::new(
        backend_id,
        ORTOOLS_VERSION,
        ORTOOLS_ADAPTER_VERSION,
        Arc::clone(&problem),
        summary,
        options,
        budget.phase_view(),
        None,
        DurationMillis::ZERO,
        backend.completion_reserve(),
    )?;
    let mut progress = Progress;
    let mut output = BoundedBackendOutput::new(
        &problem,
        &mut progress,
        request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    let outcome = backend.solve(&request, &mut output).await?;
    let result = output.into_result(outcome);
    validate_outcome(
        &request,
        &result.outcome,
        &result.candidates,
        SolverApiLimits::DEFAULT,
    )?;
    Ok(result)
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_solves_and_rejects_assignment_rule_models() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(fixture_value()?)?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(problem(&document, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    assert_eq!(result.candidates.len(), 1);
    let candidate = &result.candidates[0];
    let mut selected = Vec::new();
    for item in &compiled.variables {
        if *candidate
            .values
            .booleans
            .get(&item.variable.id)
            .ok_or("candidate omitted assignment Boolean")?
        {
            selected.push(item.pair);
        }
    }
    assert_eq!(
        selected.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([pair(1, 8)?, pair(20, 22)?])
    );
    assert!(original_accepts(&document, &selected)?);

    let mut impossible = fixture_value()?;
    impossible["domain"]["entities"][id(20)]["eligibleAssignmentTypeIds"] = json!([]);
    let impossible: ScenarioDocument = serde_json::from_value(impossible)?;
    let compiled = compile_assignment_rules(&impossible, &context())?;
    let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_maximum_assignment_count_per_person() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint("11:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint("13:00:00");
    value["domain"]["rules"][id(35)] = json!({
        "id":id(35),"kind":"maximumAssignmentCount","active":true,
        "strength":"required","scope":{"people":{"kind":"all"}},
        "calendarId":id(2),"maximum":2
    });
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(problem(&document, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let candidate = result.candidates.first().ok_or("missing candidate")?;
    let selected = compiled
        .variables
        .iter()
        .filter_map(|item| {
            candidate
                .values
                .booleans
                .get(&item.variable.id)
                .and_then(|value| (*value).then_some(item.pair))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([pair(1, 8)?, pair(1, 22)?])
    );
    assert!(original_accepts(&document, &selected)?);

    value["domain"]["rules"][id(35)]["maximum"] = json!(1);
    let impossible: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&impossible, &context())?;
    let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_authored_count_and_exact_minute_quotas() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["rules"][id(35)] = json!({
        "id":id(35),"kind":"workloadQuota","active":true,
        "strength":"required","scope":{"people":{"kind":"selected","personIds":[id(1)]}},
        "calendarId":id(2),"membership":"reportingDate",
        "measure":{"kind":"assignmentCount"},"minimum":1,"maximum":1
    });
    for (measure, feasible_minimum, impossible_minimum) in [
        (json!({"kind":"assignmentCount"}), 1, 2),
        (json!({"kind":"bucketMinutes","bucketId":id(3)}), 120, 121),
    ] {
        value["domain"]["rules"][id(35)]["measure"] = measure;
        value["domain"]["rules"][id(35)]["minimum"] = json!(feasible_minimum);
        value["domain"]["rules"][id(35)]["maximum"] = json!(feasible_minimum);
        let feasible: ScenarioDocument = serde_json::from_value(value.clone())?;
        let compiled = compile_assignment_rules(&feasible, &context())?;
        let result = solve_with_real_worker(problem(&feasible, &compiled)?).await?;
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed
        );
        let selected = compiled
            .variables
            .iter()
            .filter_map(|item| {
                result.candidates[0]
                    .values
                    .booleans
                    .get(&item.variable.id)
                    .and_then(|value| (*value).then_some(item.pair))
            })
            .collect::<Vec<_>>();
        assert!(original_accepts(&feasible, &selected)?);

        value["domain"]["rules"][id(35)]["minimum"] = json!(impossible_minimum);
        value["domain"]["rules"][id(35)]["maximum"] = json!(impossible_minimum);
        let impossible: ScenarioDocument = serde_json::from_value(value.clone())?;
        let compiled = compile_assignment_rules(&impossible, &context())?;
        let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::InfeasibilityClaimed
        );
        assert!(result.candidates.is_empty());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_distinct_weekend_cap_without_pruning_source_weekends() -> TestResult {
    let mut value = fixture_value()?;
    value["settings"]["horizon"]["end"] = json!("2026-11-10T00:00:00Z");
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint_on("2026-11-08", "09:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint_on("2026-11-08", "11:00:00");
    value["domain"]["rules"][id(35)] = json!({
        "id":id(35),"kind":"maximumWorkedWeekends","active":true,
        "strength":"required","scope":{"people":{"kind":"all"}},"maximum":2
    });
    let feasible: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&feasible, &context())?;
    let result = solve_with_real_worker(problem(&feasible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let selected = compiled
        .variables
        .iter()
        .filter_map(|item| {
            result.candidates[0]
                .values
                .booleans
                .get(&item.variable.id)
                .and_then(|selected| (*selected).then_some(item.pair))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([pair(1, 8)?, pair(1, 22)?])
    );
    assert!(original_accepts(&feasible, &[pair(1, 8)?, pair(1, 22)?])?);

    value["domain"]["rules"][id(35)]["maximum"] = json!(1);
    let impossible: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&impossible, &context())?;
    let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_optimizes_consecutive_days_and_weekends_and_respects_hard_lock() -> TestResult
{
    for weekend in [false, true] {
        for forced in [false, true] {
            let mut value = fixture_value()?;
            value["settings"]["horizon"] =
                json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-03T00:00:00Z"});
            value["domain"]["entities"][id(20)]["qualificationGrants"] = json!([{"qualificationId":id(11),"effectiveFrom":"2026-11-01T00:00:00Z","expiresAt":"2026-11-03T00:00:00Z"}]);
            value["domain"]["entities"][id(8)]["endsAt"] = endpoint("09:00:00");
            value["domain"]["entities"][id(22)]["startsAt"] = endpoint_on("2026-11-02", "08:00:00");
            value["domain"]["entities"][id(22)]["endsAt"] = endpoint_on("2026-11-02", "09:00:00");
            if weekend {
                value["settings"]["horizon"]["end"] = json!("2026-11-16T00:00:00Z");
                value["domain"]["entities"][id(2)]["period"] =
                    json!({"kind":"week","anchorDate":"2026-11-02","startTime":"00:00:00"});
                value["domain"]["entities"][id(8)]["startsAt"] =
                    endpoint_on("2026-11-07", "08:00:00");
                value["domain"]["entities"][id(8)]["endsAt"] =
                    endpoint_on("2026-11-07", "09:00:00");
                value["domain"]["entities"][id(22)]["startsAt"] =
                    endpoint_on("2026-11-14", "08:00:00");
                value["domain"]["entities"][id(22)]["endsAt"] =
                    endpoint_on("2026-11-14", "09:00:00");
                value["domain"]["entities"][id(20)]["qualificationGrants"][0]["expiresAt"] =
                    json!("2026-11-16T00:00:00Z");
                value["domain"]["entities"][id(23)]["timeWindow"] = json!({
                    "kind":"instant","startsAt":"2026-11-07T08:00:00Z",
                    "endsAt":"2026-11-07T08:30:00Z"
                });
                value["domain"]["entities"][id(23)]["effectiveRange"] =
                    json!({"startDate":"2026-11-07","endDateExclusive":"2026-11-08"});
            }
            let mode = if weekend {
                json!({"kind":"weekends","calendarId":id(2),"weekdays":["saturday"]})
            } else {
                json!({"kind":"reportingDays"})
            };
            value["domain"]["preferences"][id(40)] = json!({
                "id":id(40),"kind":"consecutiveWork","active":true,
                "scope":{"people":{"kind":"selected","personIds":[id(1)]}},
                "priority":"normal","weight":1,
                "mode":mode,"assignmentTypeIds":[id(4)],"threshold":1
            });
            if forced {
                value["domain"]["lockedAssignments"][id(41)] = json!({
                    "id":id(41),"personId":id(1),"shiftId":id(22),"state":{"kind":"hard"}
                });
            }
            let document: ScenarioDocument = serde_json::from_value(value)?;
            let compiled = compile_workforce(&document, &context())?;
            let assignment = compile_assignment_rules(&document, &context())?;
            let result = solve_with_real_worker(compiled.problem.clone()).await?;
            assert_eq!(
                result.outcome.termination,
                BackendTerminationReason::OptimalityClaimed
            );
            let candidate = result
                .candidates
                .first()
                .ok_or("no consecutive-work candidate")?;
            let selected = assignment
                .variables
                .iter()
                .filter_map(|item| {
                    candidate
                        .values
                        .booleans
                        .get(&item.variable.id)
                        .and_then(|selected| (*selected).then_some(item.pair))
                })
                .collect::<BTreeSet<_>>();
            let expected = if forced {
                BTreeSet::from([pair(1, 8)?, pair(1, 22)?])
            } else {
                BTreeSet::from([pair(1, 8)?, pair(20, 22)?])
            };
            assert_eq!(selected, expected);
            let clock = eutheto_verify::SystemVerificationClock::default();
            let reviewer = eutheto_verify::AcceptanceReviewer::new(
                &eutheto_workforce::WorkforcePack,
                &document,
                1,
                &compiled.problem,
                None,
                &clock,
            )
            .map_err(|alarm| alarm.diagnostic_code)?;
            let eutheto_verify::AcceptanceDecision::Accepted {
                result: accepted,
                objective_reconciliation: eutheto_verify::BackendObjectiveReconciliation::Matched,
                ..
            } = reviewer.review(candidate, id(500).parse()?, &context().control)
            else {
                return Err("consecutive-work backend candidate and source score disagree".into());
            };
            assert_eq!(
                accepted.verification.score.levels[0].value,
                if forced { 10 } else { 0 }
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_optimizes_weighted_peer_share_and_reconciles_source_score() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(20)]["workloadWeight"] = json!({"numerator":2,"denominator":1});
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint("11:00:00");
    value["domain"]["entities"][id(9)]["workloadPolicies"][id(10)]["targetMode"] =
        json!({"kind":"weightedEqualShare"});
    value["domain"]["preferences"][id(40)] = json!({
        "id":id(40),"kind":"workloadBalance","active":true,
        "scope":{"people":{"kind":"all"}},"priority":"low","weight":1,
        "workloadPolicyId":id(10)
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let assignment = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(compiled.problem.clone()).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let candidate = result
        .candidates
        .first()
        .ok_or("no weighted-share candidate")?;
    let selected = assignment
        .variables
        .iter()
        .filter_map(|item| {
            candidate
                .values
                .booleans
                .get(&item.variable.id)
                .and_then(|selected| (*selected).then_some(item.pair))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(selected, BTreeSet::from([pair(1, 8)?, pair(20, 22)?]));
    let clock = eutheto_verify::SystemVerificationClock::default();
    let reviewer = eutheto_verify::AcceptanceReviewer::new(
        &eutheto_workforce::WorkforcePack,
        &document,
        1,
        &compiled.problem,
        None,
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Accepted {
        result: accepted,
        objective_reconciliation,
        ..
    } = reviewer.review(candidate, id(500).parse()?, &context().control)
    else {
        return Err("weighted-share candidate failed original-source verification".into());
    };
    assert_eq!(
        objective_reconciliation,
        eutheto_verify::BackendObjectiveReconciliation::Matched
    );
    assert_eq!(accepted.verification.score.feasibility, 0);
    assert_eq!(accepted.verification.score.levels[0].value, 600);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_preserves_one_nanosecond_workload_deviation_and_reconciles_score() -> TestResult
{
    let mut value = fixture_value()?;
    for entity in [20, 21, 22, 23] {
        value["domain"]["entities"]
            .as_object_mut()
            .ok_or("entities")?
            .remove(&id(entity));
    }
    value["domain"]["entities"][id(8)]["endsAt"] = endpoint("10:00:00.000000001");
    value["domain"]["preferences"][id(40)] = json!({
        "id":id(40),"kind":"workloadBalance","active":true,
        "scope":{"people":{"kind":"all"}},"priority":"low","weight":1,
        "workloadPolicyId":id(10)
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let result = solve_with_real_worker(compiled.problem.clone()).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let candidate = result
        .candidates
        .first()
        .ok_or("no fractional workload candidate")?;
    let clock = eutheto_verify::SystemVerificationClock::default();
    let reviewer = eutheto_verify::AcceptanceReviewer::new(
        &eutheto_workforce::WorkforcePack,
        &document,
        1,
        &compiled.problem,
        None,
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Accepted {
        result: accepted,
        objective_reconciliation,
        ..
    } = reviewer.review(candidate, id(500).parse()?, &context().control)
    else {
        return Err("fractional workload solver candidate failed source verification".into());
    };
    assert_eq!(
        objective_reconciliation,
        eutheto_verify::BackendObjectiveReconciliation::Matched
    );
    assert_eq!(accepted.verification.score.feasibility, 0);
    assert_eq!(accepted.verification.score.levels[0].value, 1);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
#[allow(clippy::too_many_lines)]
async fn real_worker_optimizes_four_preference_variants_and_independently_scores_them() -> TestResult
{
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(21)]["eligibleAssignmentTypeIds"] = json!([id(4)]);
    value["domain"]["entities"][id(21)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    value["domain"]["preferences"][id(40)] = json!({
        "kind":"time","id":id(40),"active":true,
        "scope":{"people":{"kind":"selected","personIds":[id(21)]}},
        "priority":"normal","weight":10,"direction":"prefer",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z",
                      "endsAt":"2026-11-01T09:00:00Z"}
    });
    value["domain"]["preferences"][id(41)] = json!({
        "kind":"assignmentType","id":id(41),"active":true,
        "scope":{"people":{"kind":"selected","personIds":[id(1)]}},
        "priority":"high","weight":10,"direction":"avoid",
        "assignmentTypeIds":[id(4)]
    });
    value["domain"]["preferences"][id(42)] = json!({
        "kind":"location","id":id(42),"active":true,
        "scope":{"people":{"kind":"selected","personIds":[id(1)]}},
        "priority":"normal","weight":3,"direction":"avoid",
        "locationIds":[id(5)]
    });
    value["domain"]["entities"][id(43)] = json!({
        "kind":"availability","id":id(43),"personId":id(21),
        "availabilityKind":"requestedTimeOff",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z",
                      "endsAt":"2026-11-01T10:00:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"synthetic","note":""
    });
    value["domain"]["preferences"][id(44)] = json!({
        "kind":"requestedTimeOff","id":id(44),"active":true,
        "scope":{"people":{"kind":"selected","personIds":[id(21)]}},
        "priority":"normal","weight":7,"availabilityIds":[id(43)]
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let result = solve_with_real_worker(compiled.problem.clone()).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let candidate = result
        .candidates
        .first()
        .ok_or("missing preferred candidate")?;
    let solution = eutheto_workforce::assignment_rules::project_workforce_candidate(
        &compiled.problem,
        &candidate.values,
        id(500).parse()?,
        PlanningIrLimitsV1::DEFAULT,
        &context().control,
    )?;
    let selected = solution
        .assignments
        .iter()
        .map(eutheto_workforce::assignment_rules::decode_workforce_assignment)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(pair, active)| active.then_some(pair))
        .collect::<BTreeSet<_>>();
    assert_eq!(selected, BTreeSet::from([pair(21, 8)?, pair(20, 22)?]));
    let clock = eutheto_verify::SystemVerificationClock::default();
    let reviewer = eutheto_verify::AcceptanceReviewer::new(
        &eutheto_workforce::WorkforcePack,
        &document,
        1,
        &compiled.problem,
        None,
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Accepted {
        result: accepted,
        objective_reconciliation,
        ..
    } = reviewer.review(candidate, solution.solution_id, &context().control)
    else {
        return Err("preferred real-worker solution failed independent verification".into());
    };
    assert_eq!(
        objective_reconciliation,
        eutheto_verify::BackendObjectiveReconciliation::Matched
    );
    assert_eq!(accepted.verification.score.feasibility, 0);
    // Shift 8 overlaps requested leave, so its selected pair incurs normal scale 10 × weight 7.
    assert_eq!(accepted.verification.score.levels[0].value, 70);
    assert_eq!(accepted.verification.score.levels[1].value, 9);
    for include_evidence_references in [false, true] {
        let shared = eutheto_workforce::assignment_rules::build_workforce_share_result(
            &document,
            &accepted,
            None,
            eutheto_domain_api::ShareResultOptions {
                include_evidence_references,
            },
        )?;
        assert_eq!(shared.schema_version, 2);
        assert_eq!(shared.payload["schemaVersion"], 2);
        assert!(shared.payload.get("score").is_none());
        assert!(!serde_json::to_string(&shared)?.contains("requested_time_off"));
        assert_eq!(
            shared.payload.get("evidenceReferences").is_some(),
            include_evidence_references
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_reconciles_stability_pairing_and_preferred_coverage() -> TestResult {
    let mut stability = fixture_value()?;
    stability["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(23));
    stability["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),
        "sourceRevision":1,
        "assignments":[
            {"personId":id(1),"shiftId":id(8)},
            {"personId":id(20),"shiftId":id(22)}
        ]
    });
    stability["domain"]["preferences"][id(42)] = json!({
        "kind":"baseStability","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "baseScheduleId":id(40)
    });
    stability["domain"]["lockedAssignments"][id(50)] = json!({
        "id":id(50),"personId":id(1),"shiftId":id(22),"state":{"kind":"hard"}
    });

    let mut pruned_base = fixture_value()?;
    pruned_base["domain"]["rules"]
        .as_object_mut()
        .ok_or("rules")?
        .remove(&id(32));
    pruned_base["domain"]["entities"][id(24)] = json!({
        "kind":"availability","id":id(24),"personId":id(1),
        "availabilityKind":"unavailable",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z","endsAt":"2026-11-01T10:00:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"manual","note":""
    });
    pruned_base["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),
        "sourceRevision":1,"assignments":[{"personId":id(1),"shiftId":id(8)}]
    });
    pruned_base["domain"]["preferences"][id(42)] = json!({
        "kind":"baseStability","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "baseScheduleId":id(40)
    });

    let mut pairing = fixture_value()?;
    pairing["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(23));
    pairing["domain"]["entities"][id(21)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    pairing["domain"]["preferences"][id(42)] = json!({
        "kind":"togetherSeparate","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "direction":"separate","personIds":[id(1),id(20)],
        "mode":"overlappingShifts"
    });
    for (lock, person, shift) in [(50, 1, 8), (51, 20, 22)] {
        pairing["domain"]["lockedAssignments"][id(lock)] = json!({
            "id":id(lock),"personId":id(person),"shiftId":id(shift),
            "state":{"kind":"hard"}
        });
    }

    let mut coverage = fixture_value()?;
    coverage["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(22));
    coverage["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(23));
    coverage["domain"]["entities"][id(8)]["coverage"] = json!({
        "kind":"atLeast","minimum":1,"preferredCount":2,
        "maximumCount":2,"qualificationMinimums":[]
    });
    coverage["domain"]["entities"][id(40)] = json!({
        "kind":"coverageRequirement","id":id(40),"active":true,
        "scope":{"kind":"all"},
        "coverage":{"kind":"atLeast","minimum":1,"preferredCount":2,
            "maximumCount":2,"qualificationMinimums":[]}
    });
    coverage["domain"]["preferences"][id(42)] = json!({
        "kind":"preferredCoverage","id":id(42),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "coverageRequirementIds":[id(40)]
    });

    let mut adjacency = fixture_value()?;
    adjacency["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(23));
    adjacency["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    adjacency["domain"]["entities"][id(40)] = json!({
        "kind":"baseSchedule","id":id(40),"sourceSolutionId":id(41),
        "sourceRevision":1,"assignments":[{"personId":id(1),"shiftId":id(8)}]
    });
    adjacency["domain"]["preferences"][id(42)] = json!({
        "kind":"adjacency","id":id(42),"active":true,
        "scope":{"people":{"kind":"selected","personIds":[id(1)]}},
        "priority":"normal","weight":1,"direction":"prefer",
        "baseScheduleId":id(40),"maximumGapMinutes":0,"mode":"after"
    });
    adjacency["domain"]["lockedAssignments"][id(50)] = json!({
        "id":id(50),"personId":id(20),"shiftId":id(8),"state":{"kind":"hard"}
    });
    let mut adjacency_avoid = adjacency.clone();
    adjacency_avoid["domain"]["preferences"][id(42)]["direction"] = json!("avoid");
    adjacency_avoid["domain"]["lockedAssignments"][id(51)] = json!({
        "id":id(51),"personId":id(1),"shiftId":id(22),"state":{"kind":"hard"}
    });

    for (name, source, expected_score, expected_pairs, category) in [
        (
            "base stability",
            stability,
            40,
            BTreeSet::from([pair(1, 22)?, pair(20, 8)?]),
            "official.workforce.score.preference.base_stability",
        ),
        (
            "pruned base stability",
            pruned_base,
            10,
            BTreeSet::new(),
            "official.workforce.score.preference.base_stability",
        ),
        (
            "together/separate",
            pairing,
            10,
            BTreeSet::from([pair(1, 8)?, pair(20, 22)?]),
            "official.workforce.score.preference.together_separate",
        ),
        (
            "adjacency prefers the source-shift successor",
            adjacency,
            0,
            BTreeSet::from([pair(20, 8)?, pair(1, 22)?]),
            "official.workforce.score.preference.adjacency",
        ),
        (
            "adjacency avoid charges the forced successor",
            adjacency_avoid,
            10,
            BTreeSet::from([pair(20, 8)?, pair(1, 22)?]),
            "official.workforce.score.preference.adjacency",
        ),
        (
            "preferred coverage",
            coverage,
            0,
            BTreeSet::from([pair(1, 8)?, pair(20, 8)?]),
            "official.workforce.score.preference.preferred_coverage",
        ),
    ] {
        let document: ScenarioDocument = serde_json::from_value(source)?;
        let compiled = compile_workforce(&document, &context())?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed,
            "{name}"
        );
        let candidate = result.candidates.first().ok_or("no backend candidate")?;
        let solution = eutheto_workforce::assignment_rules::project_workforce_candidate(
            &compiled.problem,
            &candidate.values,
            id(500).parse()?,
            PlanningIrLimitsV1::DEFAULT,
            &context().control,
        )?;
        let selected = solution
            .assignments
            .iter()
            .map(eutheto_workforce::assignment_rules::decode_workforce_assignment)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(pair, active)| active.then_some(pair))
            .collect::<BTreeSet<_>>();
        assert_eq!(selected, expected_pairs, "{name}");
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let eutheto_verify::AcceptanceDecision::Accepted {
            result: accepted,
            objective_reconciliation,
            ..
        } = reviewer.review(candidate, solution.solution_id, &context().control)
        else {
            return Err(format!("{name} candidate failed independent verification").into());
        };
        assert_eq!(
            objective_reconciliation,
            eutheto_verify::BackendObjectiveReconciliation::Matched,
            "{name}"
        );
        assert_eq!(accepted.verification.score.feasibility, 0, "{name}");
        assert_eq!(
            accepted.verification.score.levels[0].value, expected_score,
            "{name}"
        );
        if expected_score > 0 {
            assert_eq!(
                accepted.verification.score.levels[0]
                    .category_breakdown
                    .get(&eutheto_domain_ir::ScoreCategoryId::new(category)?),
                Some(&expected_score),
                "{name}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_changes_assignment_for_authored_target_and_reconciles_shortfall() -> TestResult
{
    for (target_count, first_person, expected_penalty, no_target_candidates) in [
        (0, 1, 0, false),
        (1, 21, 0, false),
        (2, 21, 500, false),
        (2, 1, 1_000, true),
    ] {
        let mut value = fixture_value()?;
        let mut call = value["domain"]["entities"][id(4)].clone();
        call["id"] = json!(id(24));
        call["name"] = json!("Call");
        call["category"] = json!("call");
        value["domain"]["entities"][id(24)] = call;
        value["domain"]["entities"][id(22)]["assignmentTypeId"] = json!(id(24));
        value["domain"]["entities"][id(20)]["eligibleAssignmentTypeIds"] = json!([id(4), id(24)]);
        value["domain"]["entities"][id(21)]["eligibleAssignmentTypeIds"] = json!([id(4)]);
        value["domain"]["entities"][id(21)]["qualificationGrants"] =
            json!([{"qualificationId":id(11)}]);
        if no_target_candidates {
            value["domain"]["entities"][id(21)]["eligibleAssignmentTypeIds"] = json!([]);
        }
        value["domain"]["preferences"][id(40)] = json!({
            "kind":"assignmentTarget","id":id(40),"active":true,
            "scope":{"people":{"kind":"selected","personIds":[id(21)]}},
            "priority":"high","weight":5,"calendarId":id(2),"targetCount":target_count
        });
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_workforce(&document, &context())?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed
        );
        let candidate = result
            .candidates
            .first()
            .ok_or("missing target candidate")?;
        let solution = eutheto_workforce::assignment_rules::project_workforce_candidate(
            &compiled.problem,
            &candidate.values,
            id(500).parse()?,
            PlanningIrLimitsV1::DEFAULT,
            &context().control,
        )?;
        let selected = solution
            .assignments
            .iter()
            .map(eutheto_workforce::assignment_rules::decode_workforce_assignment)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(pair, active)| active.then_some(pair))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            selected,
            BTreeSet::from([pair(first_person, 8)?, pair(20, 22)?])
        );
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let eutheto_verify::AcceptanceDecision::Accepted {
            result: accepted,
            objective_reconciliation,
            ..
        } = reviewer.review(candidate, solution.solution_id, &context().control)
        else {
            return Err("target real-worker candidate failed independent verification".into());
        };
        assert_eq!(
            objective_reconciliation,
            eutheto_verify::BackendObjectiveReconciliation::Matched
        );
        assert_eq!(accepted.verification.score.feasibility, 0);
        assert_eq!(
            accepted.verification.score.levels[0].value,
            expected_penalty
        );
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_optimizes_distinct_weekend_target_and_independently_reconciles_score()
-> TestResult {
    for (target_count, person_index, expected_worked_weekends, expected_penalty) in
        [(0, 1, 0, 0), (1, 1, 1, 0), (2, 1, 2, 0), (2, 21, 0, 200)]
    {
        let mut value = fixture_value()?;
        value["settings"]["horizon"]["end"] = json!("2026-11-10T00:00:00Z");
        value["domain"]["entities"][id(22)]["startsAt"] = endpoint_on("2026-11-08", "09:00:00");
        value["domain"]["entities"][id(22)]["endsAt"] = endpoint_on("2026-11-08", "11:00:00");
        value["domain"]["entities"][id(20)]["qualificationGrants"] =
            json!([{"qualificationId":id(11)}]);
        value["domain"]["entities"]
            .as_object_mut()
            .ok_or("entities")?
            .remove(&id(23));
        value["domain"]["preferences"][id(40)] = json!({
            "kind":"weekendTarget","id":id(40),"active":true,
            "scope":{"people":{"kind":"selected","personIds":[id(person_index)]}},
            "priority":"normal","weight":10,"targetCount":target_count
        });
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_workforce(&document, &context())?;
        let assignment = compile_assignment_rules(&document, &context())?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed
        );
        let candidate = result
            .candidates
            .first()
            .ok_or("missing weekend-target candidate")?;
        let person_id: eutheto_types::PersonId = id(person_index).parse()?;
        let selected = assignment
            .variables
            .iter()
            .filter_map(|item| {
                candidate
                    .values
                    .booleans
                    .get(&item.variable.id)
                    .and_then(|selected| (*selected).then_some(item.pair))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(selected.len(), 2);
        assert_eq!(
            selected
                .iter()
                .filter(|pair| pair.person_id == person_id)
                .count(),
            expected_worked_weekends
        );
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let eutheto_verify::AcceptanceDecision::Accepted {
            result: accepted,
            objective_reconciliation,
            ..
        } = reviewer.review(candidate, id(500).parse()?, &context().control)
        else {
            return Err("weekend-target worker candidate failed independent verification".into());
        };
        assert_eq!(
            objective_reconciliation,
            eutheto_verify::BackendObjectiveReconciliation::Matched
        );
        assert_eq!(accepted.verification.score.feasibility, 0);
        assert_eq!(
            accepted.verification.score.levels[0].value,
            expected_penalty
        );
        if expected_penalty > 0 {
            assert_eq!(
                accepted.verification.score.levels[0]
                    .category_breakdown
                    .get(&eutheto_domain_ir::ScoreCategoryId::new(
                        "official.workforce.score.preference.weekend_target"
                    )?),
                Some(&expected_penalty)
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_approved_leave_and_available_only_filters() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(21)]["eligibleAssignmentTypeIds"] = json!([id(4)]);
    value["domain"]["entities"][id(21)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    value["domain"]["entities"][id(24)] = json!({
        "kind":"availability",
        "id":id(24),
        "personId":id(1),
        "availabilityKind":"approvedTimeOff",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z","endsAt":"2026-11-01T10:00:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"synthetic","note":""
    });
    value["domain"]["entities"][id(25)] = json!({
        "kind":"availability",
        "id":id(25),
        "personId":id(21),
        "availabilityKind":"availableOnly",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z","endsAt":"2026-11-01T10:00:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
        "source":"synthetic","note":""
    });
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(problem(&document, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let candidate = result.candidates.first().ok_or("missing candidate")?;
    let selected: BTreeSet<_> = compiled
        .variables
        .iter()
        .filter_map(|item| {
            candidate
                .values
                .booleans
                .get(&item.variable.id)
                .and_then(|value| (*value).then_some(item.pair))
        })
        .collect();
    assert_eq!(selected.len(), 2);
    assert!(selected.contains(&pair(21, 8)?));
    assert!(!selected.contains(&pair(1, 8)?));
    assert!(!selected.contains(&pair(20, 8)?));
    assert!(!selected.contains(&pair(21, 22)?));
    let selected_vec: Vec<_> = selected.iter().copied().collect();
    assert!(original_accepts(&document, &selected_vec)?);

    let mut impossible = value;
    impossible["domain"]["entities"][id(25)]["timeWindow"]["startsAt"] =
        json!("2026-11-01T09:00:00Z");
    impossible["domain"]["entities"][id(25)]["timeWindow"]["endsAt"] =
        json!("2026-11-01T10:00:00Z");
    let impossible: ScenarioDocument = serde_json::from_value(impossible)?;
    let compiled = compile_assignment_rules(&impossible, &context())?;
    let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_solves_and_rejects_minimum_rest_models() -> TestResult {
    let value = rest_fixture()?;
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(problem(&document, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    assert_eq!(result.candidates.len(), 1);
    let mut selected = Vec::new();
    for item in &compiled.variables {
        if *result.candidates[0]
            .values
            .booleans
            .get(&item.variable.id)
            .ok_or("candidate omitted assignment Boolean")?
        {
            selected.push(item.pair);
        }
    }
    assert_eq!(
        selected.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([pair(1, 8)?, pair(20, 22)?])
    );
    assert!(original_accepts(&document, &selected)?);
    let mut impossible = value;
    impossible["domain"]["entities"][id(20)]["eligibleAssignmentTypeIds"] = json!([]);
    let impossible = serde_json::from_value(impossible)?;
    let compiled = compile_assignment_rules(&impossible, &context())?;
    let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_mutual_modes_and_half_open_endpoint() -> TestResult {
    let mutual_rule_id: RuleId = id(34).parse()?;
    let same = mutual_fixture("sameShift")?;
    let overlap = mutual_fixture("overlappingShifts")?;
    let mut touching = overlap.clone();
    touching["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    // Both coverage shifts require one person. Only person 1 can take the first
    // shift; person 20 can take the second, so the overlap restriction is decisive.
    for (name, mut value, feasible) in [
        ("same shift", same, true),
        ("overlapping", overlap, false),
        ("touching endpoints", touching, true),
    ] {
        value["domain"]["rules"][id(32)]["active"] = json!(true);
        value["domain"]["rules"][id(33)]["active"] = json!(true);
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_workforce(&document, &context())?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        if !feasible {
            assert_eq!(
                result.outcome.termination,
                BackendTerminationReason::InfeasibilityClaimed,
                "{name}"
            );
            assert!(result.candidates.is_empty(), "{name}");
            continue;
        }
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed,
            "{name}"
        );
        let candidate = result
            .candidates
            .first()
            .ok_or("missing real worker candidate")?;
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let eutheto_verify::AcceptanceDecision::Accepted { result, .. } =
            reviewer.review(candidate, id(501).parse()?, &context().control)
        else {
            return Err(format!("{name}: real worker candidate was not accepted").into());
        };
        assert_eq!(result.verification.score.feasibility, 0, "{name}");
        assert!(
            result
                .verification
                .required_rule_results
                .iter()
                .find(|item| item.rule_id == mutual_rule_id)
                .ok_or("missing mutual verification result")?
                .satisfied
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_directed_transition_boundaries() -> TestResult {
    let short = transition_fixture()?;
    let mut exact = short.clone();
    exact["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:30:00");
    exact["domain"]["entities"][id(22)]["endsAt"] = endpoint("11:30:00");
    let mut touching_zero = short.clone();
    touching_zero["domain"]["entities"][id(5)]["transitions"][0]["minutes"] = json!(0);
    touching_zero["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    let mut absent_edge = short.clone();
    absent_edge["domain"]["entities"][id(5)]["transitions"] = json!([]);
    let rule_id: RuleId = id(34).parse()?;
    for (name, mut value, feasible) in [
        ("under thirty minutes", short, false),
        ("exactly thirty minutes", exact, true),
        ("touching with zero minutes", touching_zero, true),
        ("no configured directed edge", absent_edge, true),
    ] {
        value["domain"]["rules"][id(32)]["active"] = json!(true);
        // Person 20 cannot cover either shift; Person 1 must cover both, making
        // only the directed travel constraint decisive.
        value["domain"]["entities"][id(41)] = json!({
            "kind":"availability","id":id(41),"personId":id(20),
            "availabilityKind":"unavailable",
            "timeWindow":{"kind":"instant","startsAt":"2026-11-01T09:00:00Z","endsAt":"2026-11-01T12:00:00Z"},
            "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
            "source":"manual","note":""
        });
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let compiled = compile_workforce(&document, &context())?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        if !feasible {
            assert_eq!(
                result.outcome.termination,
                BackendTerminationReason::InfeasibilityClaimed,
                "{name}"
            );
            assert!(result.candidates.is_empty(), "{name}");
            continue;
        }
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed,
            "{name}"
        );
        let candidate = result
            .candidates
            .first()
            .ok_or("missing real worker transition candidate")?;
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let eutheto_verify::AcceptanceDecision::Accepted { result, .. } =
            reviewer.review(candidate, id(502).parse()?, &context().control)
        else {
            return Err(format!("{name}: travel-constrained candidate was not accepted").into());
        };
        assert_eq!(result.verification.score.feasibility, 0, "{name}");
        assert!(
            result
                .verification
                .required_rule_results
                .iter()
                .find(|item| item.rule_id == rule_id)
                .ok_or("missing directed transition verification result")?
                .satisfied
        );
    }
    Ok(())
}

#[test]
fn maximum_hours_contribution_estimate_counts_every_sum_and_union_helper() -> TestResult {
    for contribution in ["sum", "union"] {
        let mut value = fixture_value()?;
        value["domain"]["entities"][id(3)]["overlappingContribution"] = json!(contribution);
        value["domain"]["rules"][id(35)] = json!({
            "kind":"maximumHours","id":id(35),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"bucketId":id(3),
            "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
            "maximumMinutes":240,
        });
        let document: ScenarioDocument = serde_json::from_value(value)?;
        let analysis = analyze_assignments(&document, None, PlanningIrLimitsV1::DEFAULT)?;
        let compilation = compile_assignment_rules(&document, &context())?;
        assert_eq!(analysis.estimate, compilation.estimate, "{contribution}");
        assert_eq!(
            compilation.estimate.variables,
            u64::try_from(compilation.variables.len() + compilation.auxiliary_variables.len())?,
            "{contribution}"
        );
        assert_eq!(
            compilation.estimate.constraints,
            u64::try_from(compilation.constraints.len())?,
            "{contribution}"
        );
        assert_eq!(
            compilation.estimate.provenance_records,
            u64::try_from(compilation.provenance.len())?,
            "{contribution}"
        );
    }
    Ok(())
}

#[test]
fn maximum_hours_preflight_rejects_tight_provenance_and_per_record_limits() -> TestResult {
    use eutheto_workforce::assignment_rules::{AssignmentRuleError, AssignmentRuleLimit};

    let mut value = fixture_value()?;
    value["domain"]["rules"] = json!({});
    value["domain"]["rules"][id(35)] = json!({
        "kind":"maximumHours","id":id(35),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},"bucketId":id(3),
        "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
        "maximumMinutes":120,
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let contribution = compile_assignment_rules(&document, &context())?;
    let mut restricted = context();
    restricted.planning_limits.max_provenance_records =
        u64::try_from(contribution.provenance.len())? - 1;
    for result in [
        compile_assignment_rules(&document, &restricted).map(|_| ()),
        compile_workforce(&document, &restricted).map(|_| ()),
    ] {
        assert!(matches!(
            result,
            Err(AssignmentRuleError::LimitExceeded(
                AssignmentRuleLimit::Records | AssignmentRuleLimit::ProvenanceRecords
            ))
        ));
    }
    for tighten in [
        (2_u64, PlanningIrLimitsV1::DEFAULT.max_parameters_per_record),
        (PlanningIrLimitsV1::DEFAULT.max_entity_refs_per_record, 6),
    ] {
        restricted = context();
        restricted.planning_limits.max_entity_refs_per_record = tighten.0;
        restricted.planning_limits.max_parameters_per_record = tighten.1;
        for result in [
            compile_assignment_rules(&document, &restricted).map(|_| ()),
            compile_workforce(&document, &restricted).map(|_| ()),
        ] {
            assert_eq!(
                result,
                Err(AssignmentRuleError::LimitExceeded(
                    AssignmentRuleLimit::PerRecord
                ))
            );
        }
    }
    Ok(())
}

#[test]
fn maximum_hours_calendar_evaluates_only_periods_touched_by_sparse_selected_shifts() -> TestResult {
    let mut value = fixture_value()?;
    value["settings"]["horizon"] =
        json!({"start":"2000-01-01T00:00:00Z","end":"2090-01-02T00:00:00Z"});
    let endpoint =
        |local: &str| json!({"instant":format!("{local}Z"),"local":local,"offsetSeconds":0});
    value["domain"]["entities"][id(8)]["startsAt"] = endpoint("2000-01-01T00:00:00");
    value["domain"]["entities"][id(8)]["endsAt"] = endpoint("2000-01-01T01:00:00");
    let mut later = value["domain"]["entities"][id(8)].clone();
    later["id"] = json!(id(15));
    later["startsAt"] = endpoint("2090-01-01T00:00:00");
    later["endsAt"] = endpoint("2090-01-01T01:00:00");
    value["domain"]["entities"][id(15)] = later;
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(22));
    value["domain"]["rules"][id(35)] = json!({
        "kind":"maximumHours","id":id(35),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},"bucketId":id(3),
        "window":{"kind":"calendar","calendarId":id(2),"membership":"startInstant"},
        "maximumMinutes":60,
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    compile_workforce(&document, &context())?;
    let selected = [pair(1, 8)?, pair(1, 15)?];
    assert!(original_accepts(&document, &selected)?);
    let evaluated = evaluate_assignment_rules(&document, &selected, None)?;
    let rule_id: RuleId = id(35).parse()?;
    assert!(
        evaluated
            .evaluations
            .iter()
            .find(|evaluation| evaluation.rule_id == rule_id)
            .ok_or("maximum hours summary missing")?
            .satisfied
    );
    Ok(())
}

#[test]
fn maximum_hours_calendar_start_ignores_ambiguous_period_after_shift_start() -> TestResult {
    let mut value = fixture_value()?;
    value["settings"]["timeZone"] = json!("America/New_York");
    value["settings"]["horizon"] =
        json!({"start":"2026-10-30T04:00:00Z","end":"2026-11-03T05:00:00Z"});
    value["domain"]["entities"][id(2)]["period"] = json!({"kind":"day","startTime":"01:30:00"});
    value["domain"]["entities"][id(8)]["startsAt"] = json!({
        "instant":"2026-10-30T06:00:00Z","local":"2026-10-30T02:00:00",
        "offsetSeconds":-14400
    });
    value["domain"]["entities"][id(8)]["endsAt"] = json!({
        "instant":"2026-11-02T08:00:00Z","local":"2026-11-02T03:00:00",
        "offsetSeconds":-18000
    });
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(22));
    value["domain"]["rules"][id(35)] = json!({
        "kind":"maximumHours","id":id(35),"active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},"bucketId":id(3),
        "window":{"kind":"calendar","calendarId":id(2),"membership":"startInstant"},
        "maximumMinutes":5000,
    });
    let document: ScenarioDocument = serde_json::from_value(value)?;
    compile_assignment_rules(&document, &context())?;
    compile_workforce(&document, &context())?;
    assert!(original_accepts(&document, &[pair(1, 8)?])?);
    Ok(())
}

// Keep the calendar, rolling and nanosecond worker cases in one acceptance matrix.
#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
#[allow(clippy::too_many_lines)]
async fn real_worker_enforces_calendar_and_rolling_maximum_hours() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint("10:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint("12:00:00");
    let rule_id: RuleId = id(35).parse()?;
    for (name, window, maximum, feasible, fractional) in [
        (
            "calendar under cap",
            json!({"kind":"calendar","calendarId":id(2),"membership":"reportingDate"}),
            239,
            false,
            false,
        ),
        (
            "calendar exact cap",
            json!({"kind":"calendar","calendarId":id(2),"membership":"reportingDate"}),
            240,
            true,
            false,
        ),
        (
            "calendar one nanosecond over exact cap",
            json!({"kind":"calendar","calendarId":id(2),"membership":"reportingDate"}),
            240,
            false,
            true,
        ),
        (
            "rolling start under cap",
            json!({"kind":"rolling","durationMinutes":240,"membership":"startInstant"}),
            239,
            false,
            false,
        ),
        (
            "rolling start exact cap",
            json!({"kind":"rolling","durationMinutes":240,"membership":"startInstant"}),
            240,
            true,
            false,
        ),
        (
            "rolling intersection under cap",
            json!({"kind":"rolling","durationMinutes":120,"membership":"intersection"}),
            119,
            false,
            false,
        ),
        (
            "rolling intersection exact cap",
            json!({"kind":"rolling","durationMinutes":120,"membership":"intersection"}),
            120,
            true,
            false,
        ),
    ] {
        let mut candidate = value.clone();
        if fractional {
            candidate["domain"]["entities"][id(22)]["endsAt"] = endpoint("12:00:00.000000001");
        }
        candidate["domain"]["rules"][id(35)] = json!({
            "kind":"maximumHours","id":id(35),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"bucketId":id(3),
            "window":window,"maximumMinutes":maximum,
        });
        let document: ScenarioDocument = serde_json::from_value(candidate)?;
        compile_assignment_rules(&document, &context())
            .map_err(|error| format!("{name}: contribution: {error:?}"))?;
        let compiled = compile_workforce(&document, &context())
            .map_err(|error| format!("{name}: complete model: {error:?}"))?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        if !feasible {
            assert_eq!(
                result.outcome.termination,
                BackendTerminationReason::InfeasibilityClaimed,
                "{name}"
            );
            assert!(result.candidates.is_empty(), "{name}");
            continue;
        }
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed,
            "{name}"
        );
        let candidate = result
            .candidates
            .first()
            .ok_or("missing real worker maximum-hours candidate")?;
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let decision = reviewer.review(candidate, id(503).parse()?, &context().control);
        let eutheto_verify::AcceptanceDecision::Accepted { result, .. } = decision else {
            return Err(
                format!("{name}: maximum-hours candidate was not accepted: {decision:?}").into(),
            );
        };
        assert_eq!(result.verification.score.feasibility, 0, "{name}");
        assert!(
            result
                .verification
                .required_rule_results
                .iter()
                .find(|item| item.rule_id == rule_id)
                .ok_or("missing maximum-hours verification result")?
                .satisfied,
            "{name}"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_signed_scheduled_rolling_credit_exit() -> TestResult {
    let mut value = fixture_value()?;
    value["settings"]["timeZone"] = json!("America/New_York");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T04:00:00Z","end":"2026-11-02T05:00:00Z"});
    value["domain"]["entities"][id(3)]["measurement"] = json!("scheduledMinutes");
    value["domain"]["entities"][id(3)]["overlappingContribution"] = json!("sum");
    value["domain"]["entities"][id(8)]["startsAt"] = json!({
        "instant":"2026-11-01T05:45:00Z","local":"2026-11-01T01:45:00",
        "offsetSeconds":-14400
    });
    value["domain"]["entities"][id(8)]["endsAt"] = json!({
        "instant":"2026-11-01T06:15:00Z","local":"2026-11-01T01:15:00",
        "offsetSeconds":-18000
    });
    value["domain"]["entities"][id(22)]["startsAt"] = json!({
        "instant":"2026-11-01T06:20:00Z","local":"2026-11-01T01:20:00",
        "offsetSeconds":-18000
    });
    value["domain"]["entities"][id(22)]["endsAt"] = json!({
        "instant":"2026-11-01T07:20:00Z","local":"2026-11-01T02:20:00",
        "offsetSeconds":-18000
    });
    let rule_id: RuleId = id(35).parse()?;
    for (maximum, feasible) in [(40, false), (60, true)] {
        value["domain"]["rules"][id(35)] = json!({
            "kind":"maximumHours","id":id(35),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}},"bucketId":id(3),
            "window":{"kind":"rolling","durationMinutes":60,"membership":"startInstant"},
            "maximumMinutes":maximum,
        });
        let document: ScenarioDocument = serde_json::from_value(value.clone())?;
        let compiled = compile_workforce(&document, &context())?;
        let result = solve_with_real_worker(compiled.problem.clone()).await?;
        if !feasible {
            assert_eq!(
                result.outcome.termination,
                BackendTerminationReason::InfeasibilityClaimed,
                "signed negative credit must not mask a later positive-only window"
            );
            assert!(result.candidates.is_empty());
            continue;
        }
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::OptimalityClaimed
        );
        let candidate = result
            .candidates
            .first()
            .ok_or("missing signed hours candidate")?;
        let clock = eutheto_verify::SystemVerificationClock::default();
        let reviewer = eutheto_verify::AcceptanceReviewer::new(
            &eutheto_workforce::WorkforcePack,
            &document,
            1,
            &compiled.problem,
            None,
            &clock,
        )
        .map_err(|alarm| alarm.diagnostic_code)?;
        let decision = reviewer.review(candidate, id(504).parse()?, &context().control);
        let eutheto_verify::AcceptanceDecision::Accepted { result, .. } = decision else {
            return Err(format!("signed rolling candidate was not accepted: {decision:?}").into());
        };
        assert_eq!(result.verification.score.feasibility, 0);
        assert!(
            result
                .verification
                .required_rule_results
                .iter()
                .find(|item| item.rule_id == rule_id)
                .ok_or("missing signed hours verification result")?
                .satisfied
        );
    }
    Ok(())
}

async fn check_consecutive_worker(
    name: &str,
    value: Value,
    expected_feasible: bool,
    selected: &[AssignmentPair],
) -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(value)?;
    assert_eq!(
        original_accepts(&document, selected)?,
        expected_feasible,
        "{name}: original source meaning"
    );
    compile_assignment_rules(&document, &context())
        .map_err(|error| format!("{name}: contribution: {error:?}"))?;
    let compiled = compile_workforce(&document, &context())
        .map_err(|error| format!("{name}: complete model: {error:?}"))?;
    let result = solve_with_real_worker(compiled.problem.clone()).await?;
    if !expected_feasible {
        assert_eq!(
            result.outcome.termination,
            BackendTerminationReason::InfeasibilityClaimed,
            "{name}"
        );
        assert!(result.candidates.is_empty(), "{name}");
        return Ok(());
    }
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed,
        "{name}"
    );
    let candidate = result
        .candidates
        .first()
        .ok_or("missing real-worker consecutive candidate")?;
    let clock = eutheto_verify::SystemVerificationClock::default();
    let reviewer = eutheto_verify::AcceptanceReviewer::new(
        &eutheto_workforce::WorkforcePack,
        &document,
        1,
        &compiled.problem,
        None,
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let decision = reviewer.review(candidate, id(505).parse()?, &context().control);
    let eutheto_verify::AcceptanceDecision::Accepted { result, .. } = decision else {
        return Err(format!("{name}: consecutive candidate was not accepted: {decision:?}").into());
    };
    let rule_id: RuleId = id(35).parse()?;
    assert_eq!(result.verification.score.feasibility, 0, "{name}");
    assert!(
        result
            .verification
            .required_rule_results
            .iter()
            .find(|item| item.rule_id == rule_id)
            .ok_or("missing consecutive verification result")?
            .satisfied,
        "{name}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
#[allow(clippy::too_many_lines)]
async fn real_worker_enforces_consecutive_civil_days_and_selected_assignment_runs() -> TestResult {
    let base = consecutive_fixture()?;
    let all_selected = [pair(1, 8)?, pair(1, 22)?, pair(1, 25)?];
    for (name, third_date, maximum, feasible) in [
        ("duplicate same-day assignments", "2026-11-01", 1, true),
        ("zero cap forbids any match", "2026-11-01", 0, false),
        ("adjacent civil dates", "2026-11-02", 1, false),
        ("two worked dates within cap", "2026-11-02", 2, true),
        ("sparse dates do not join", "2026-11-03", 1, true),
    ] {
        let mut value = base.clone();
        value["domain"]["rules"][id(35)]["mode"] = json!({"kind":"workedDays"});
        value["domain"]["rules"][id(35)]["maximum"] = json!(maximum);
        value["domain"]["entities"][id(25)]["startsAt"] = endpoint_on(third_date, "10:00:00");
        value["domain"]["entities"][id(25)]["endsAt"] = endpoint_on(third_date, "11:00:00");
        check_consecutive_worker(name, value, feasible, &all_selected).await?;
    }
    for (name, second_start, third_start, first_end, third_end, break_minutes, maximum, feasible) in [
        (
            "three touching selected assignments",
            "09:00:00",
            "10:00:00",
            "09:00:00",
            "11:00:00",
            0,
            2,
            false,
        ),
        (
            "three selected assignments at cap",
            "09:00:00",
            "10:00:00",
            "09:00:00",
            "11:00:00",
            0,
            3,
            true,
        ),
        (
            "exact elapsed break",
            "09:30:00",
            "10:30:00",
            "09:00:00",
            "11:00:00",
            30,
            2,
            false,
        ),
        (
            "one nanosecond beyond break",
            "09:30:00",
            "10:30:00.000000001",
            "09:00:00",
            "11:00:00",
            30,
            2,
            true,
        ),
        (
            "overlap with reversed end order resets run",
            "09:00:00",
            "12:00:00",
            "20:00:00",
            "13:00:00",
            0,
            2,
            true,
        ),
    ] {
        let mut value = base.clone();
        value["domain"]["rules"][id(35)]["maximum"] = json!(maximum);
        value["domain"]["rules"][id(35)]["mode"]["breakMinutes"] = json!(break_minutes);
        value["domain"]["entities"][id(8)]["endsAt"] = endpoint(first_end);
        value["domain"]["entities"][id(22)]["startsAt"] = endpoint(second_start);
        value["domain"]["entities"][id(25)]["startsAt"] = endpoint(third_start);
        value["domain"]["entities"][id(25)]["endsAt"] = endpoint(third_end);
        check_consecutive_worker(name, value, feasible, &all_selected).await?;
    }
    let mut filtered = base;
    let mut call = filtered["domain"]["entities"][id(4)].clone();
    call["id"] = json!(id(26));
    call["category"] = json!("call");
    filtered["domain"]["entities"][id(26)] = call;
    filtered["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] = json!([id(4), id(26)]);
    for shift in [8, 25] {
        filtered["domain"]["entities"][id(shift)]["assignmentTypeId"] = json!(id(26));
    }
    filtered["domain"]["entities"][id(25)]["startsAt"] = endpoint("09:00:00");
    filtered["domain"]["entities"][id(25)]["endsAt"] = endpoint("10:00:00");
    filtered["domain"]["rules"][id(35)]["scope"]["categories"] = json!(["call"]);
    filtered["domain"]["rules"][id(35)]["maximum"] = json!(1);
    check_consecutive_worker(
        "intervening selected nonmatch never resets run",
        filtered,
        false,
        &all_selected,
    )
    .await?;
    let mut skipped_middle = consecutive_fixture()?;
    skipped_middle["domain"]["rules"][id(35)]["mode"] = json!({"kind":"workedDays"});
    skipped_middle["domain"]["rules"][id(35)]["maximum"] = json!(1);
    skipped_middle["domain"]["entities"][id(20)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    skipped_middle["domain"]["entities"][id(22)]["startsAt"] =
        endpoint_on("2026-11-02", "09:00:00");
    skipped_middle["domain"]["entities"][id(22)]["endsAt"] = endpoint_on("2026-11-02", "10:00:00");
    skipped_middle["domain"]["entities"][id(25)]["startsAt"] =
        endpoint_on("2026-11-03", "10:00:00");
    skipped_middle["domain"]["entities"][id(25)]["endsAt"] = endpoint_on("2026-11-03", "11:00:00");
    skipped_middle["domain"]["lockedAssignments"][id(41)]["personId"] = json!(id(20));
    check_consecutive_worker(
        "unselected matching middle day does not prohibit separated worked days",
        skipped_middle,
        true,
        &[pair(1, 8)?, pair(20, 22)?, pair(1, 25)?],
    )
    .await
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_required_skill_mix_even_when_other_rules_have_candidates()
-> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(12)] = json!({
        "kind":"qualification", "id":id(12), "name":"Specialist", "description":""
    });
    for person in [1, 20] {
        value["domain"]["entities"][id(person)]["qualificationGrants"]
            .as_array_mut()
            .ok_or("grants")?
            .push(json!({"qualificationId":id(12)}));
    }
    value["domain"]["rules"][id(34)] = json!({
        "kind":"requiredSkillMix", "id":id(34), "active":true, "strength":"required",
        "scope":{"people":{"kind":"all"}},
        "qualificationMinimums":[{
            "qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},
            "minimum":1
        }]
    });
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(problem(&document, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    assert_eq!(result.candidates.len(), 1);
    let selected = compiled
        .variables
        .iter()
        .filter_map(|item| {
            result.candidates[0]
                .values
                .booleans
                .get(&item.variable.id)
                .copied()
                .filter(|included| *included)
                .map(|_| item.pair)
        })
        .collect::<Vec<_>>();
    assert!(original_accepts(&document, &selected)?);
    value["domain"]["entities"][id(1)]["qualificationGrants"] = json!([{"qualificationId":id(11)}]);
    let impossible: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&impossible, &context())?;
    let result = solve_with_real_worker(problem(&impossible, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
async fn real_worker_enforces_fixed_assignment_and_hard_lock() -> TestResult {
    let mut value = fixture_value()?;
    value["domain"]["entities"][id(21)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    value["domain"]["rules"][id(34)] = json!({
        "kind":"fixedAssignment", "id":id(34), "active":true, "strength":"required",
        "scope":{"people":{"kind":"all"}},"personId":id(21),"shiftId":id(8)
    });
    value["domain"]["lockedAssignments"][id(35)] = json!({
        "id":id(35),"personId":id(20),"shiftId":id(22),"state":{"kind":"hard"}
    });
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&document, &context())?;
    let result = solve_with_real_worker(problem(&document, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    let candidate = result
        .candidates
        .first()
        .ok_or("missing feasible worker candidate")?;
    let selected = compiled
        .variables
        .iter()
        .filter_map(|item| {
            candidate
                .values
                .booleans
                .get(&item.variable.id)
                .copied()
                .filter(|included| *included)
                .map(|_| item.pair)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([pair(21, 8)?, pair(20, 22)?])
    );
    assert!(original_accepts(&document, &[pair(21, 8)?, pair(20, 22)?])?);

    value["domain"]["entities"][id(21)]["qualificationGrants"] = json!([]);
    let pruned_fixed: ScenarioDocument = serde_json::from_value(value.clone())?;
    let compiled = compile_assignment_rules(&pruned_fixed, &context())?;
    let result = solve_with_real_worker(problem(&pruned_fixed, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());

    value["domain"]["entities"][id(21)]["qualificationGrants"] =
        json!([{"qualificationId":id(11)}]);
    value["domain"]["lockedAssignments"][id(35)]["shiftId"] = json!(id(8));
    let pruned_lock: ScenarioDocument = serde_json::from_value(value)?;
    let compiled = compile_assignment_rules(&pruned_lock, &context())?;
    let result = solve_with_real_worker(problem(&pruned_lock, &compiled)?).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

#[test]
fn skill_mix_command_inverse_and_portable_roundtrip_preserve_verifier_meaning() -> TestResult {
    use eutheto_domain_api::{
        DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPack, PortableImportContext,
    };
    use eutheto_types::{DomainCommandEnvelope, ScenarioDomain};
    use eutheto_workforce::{WorkforcePack, commands};

    let mut value = fixture_value()?;
    value["domain"]["entities"][id(12)] = json!({
        "kind":"qualification", "id":id(12), "name":"Specialist", "description":""
    });
    value["domain"]["entities"][id(1)]["qualificationGrants"]
        .as_array_mut()
        .ok_or("grants")?
        .push(json!({"qualificationId":id(12)}));
    value["domain"]["rules"][id(34)] = json!({
        "kind":"requiredSkillMix", "id":id(34), "active":true, "strength":"required",
        "scope":{"people":{"kind":"all"}},
        "qualificationMinimums":[{
            "qualifications":{"allQualificationIds":[id(12)],"anyQualificationIds":[]},
            "minimum":1
        }]
    });
    let original: ScenarioDocument = serde_json::from_value(value)?;
    let selected = [pair(1, 8)?, pair(20, 22)?];
    assert!(!original_accepts(&original, &selected)?);
    let mut relaxed = original
        .domain
        .rules
        .get(&id(34).parse()?)
        .ok_or("skill-mix rule")?
        .clone();
    relaxed["qualificationMinimums"][0]["minimum"] = json!(0);
    let change = WorkforcePack.apply_batch(
        &original,
        &DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: original.domain_pack.id.clone(),
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: commands::UPDATE_RULE.to_owned(),
                payload: json!({"rule":relaxed}),
            }],
        },
        &CancellationToken::new(),
    )?;
    assert!(original_accepts(&change.document, &selected)?);
    let undone =
        WorkforcePack.apply_batch(&change.document, &change.inverse, &CancellationToken::new())?;
    assert_eq!(undone.document, original);
    let portable = WorkforcePack.export_portable(&original)?;
    let mut shell = original.clone();
    shell.domain = ScenarioDomain::default();
    shell.extensions.clear();
    let restored = WorkforcePack.import_portable(
        &serde_json::from_slice(&serde_json::to_vec(&portable)?)?,
        &PortableImportContext {
            scenario_shell: shell,
        },
    )?;
    assert_eq!(restored, original);
    let compiled = compile_assignment_rules(&restored, &context())?;
    for selection in selections()? {
        assert_eq!(
            mathematics_accepts(&compiled, &selection)?,
            original_accepts(&original, &selection)?
        );
    }
    Ok(())
}

#[test]
fn rest_command_inverse_and_portable_roundtrip_preserve_assignment_semantics() -> TestResult {
    use eutheto_domain_api::{
        DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPack, PortableImportContext,
    };
    use eutheto_types::{DomainCommandEnvelope, ScenarioDomain};
    use eutheto_workforce::{WorkforcePack, commands};

    let original: ScenarioDocument = serde_json::from_value(rest_fixture()?)?;
    let selected = [pair(1, 8)?, pair(1, 22)?];
    assert!(!original_accepts(&original, &selected)?);
    let mut relaxed = original
        .domain
        .rules
        .get(&id(34).parse()?)
        .ok_or("rest rule")?
        .clone();
    relaxed["minimumMinutes"] = json!(599);
    let change = WorkforcePack.apply_batch(
        &original,
        &DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: original.domain_pack.id.clone(),
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: commands::UPDATE_RULE.to_owned(),
                payload: json!({"rule":relaxed}),
            }],
        },
        &CancellationToken::new(),
    )?;
    assert!(original_accepts(&change.document, &selected)?);
    let undone =
        WorkforcePack.apply_batch(&change.document, &change.inverse, &CancellationToken::new())?;
    assert_eq!(undone.document, original);
    assert!(!original_accepts(&undone.document, &selected)?);

    let portable = WorkforcePack.export_portable(&original)?;
    let wire = serde_json::to_vec(&portable)?;
    let mut shell = original.clone();
    shell.domain = ScenarioDomain::default();
    shell.extensions.clear();
    let restored = WorkforcePack.import_portable(
        &serde_json::from_slice(&wire)?,
        &PortableImportContext {
            scenario_shell: shell,
        },
    )?;
    assert_eq!(restored, original);
    let compiled = compile_assignment_rules(&restored, &context())?;
    for selection in selections()? {
        assert_eq!(
            mathematics_accepts(&compiled, &selection)?,
            original_accepts(&original, &selection)?
        );
    }
    Ok(())
}

#[test]
fn directional_rest_identities_match_frozen_vectors() -> TestResult {
    let mut value = rest_fixture()?;
    value["domain"]["entities"][id(22)]["startsAt"] = endpoint("08:00:00");
    value["domain"]["entities"][id(22)]["endsAt"] = endpoint("09:00:00");
    for (reverse, constraint, provenance) in [
        (
            false,
            "official.workforce.constraint.8b4fbd99b990ecb97899a8575c3963d6a9ed209e39fcfd1cf058cf830b29f23c",
            "official.workforce.provenance.f238b99439ad38ee4fbc21af420358e300bd6f4bd3ad24e40f49f495755554d8",
        ),
        (
            true,
            "official.workforce.constraint.78529b53b934b292be43b5e4e04da46848115b2291c74488ce667c2c8b0ef091",
            "official.workforce.provenance.6005be31f5bd93811600310cf4b71a973a2bd2a7d95b4762b22c64d8041ab909",
        ),
    ] {
        if reverse {
            value["domain"]["rules"][id(34)]["afterScope"]["categories"] = json!(["clinic"]);
            value["domain"]["rules"][id(34)]["beforeScope"]["categories"] = json!(["call"]);
        }
        let document = serde_json::from_value(value.clone())?;
        let compiled = compile_assignment_rules(&document, &context())?;
        let record = compiled
            .constraints
            .iter()
            .find(|record| record.id.as_str() == constraint)
            .ok_or("directional rest identity changed")?;
        assert_eq!(record.provenance.as_str(), provenance);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker"]
#[allow(clippy::too_many_lines)]
async fn real_worker_complete_workforce_rank_projection_and_infeasibility() -> TestResult {
    let mut value = rest_fixture()?;
    // Exactly two source people, one unavailable for the first shift. The nine-hour,
    // fifty-nine-minute gap is below the required ten hours, so person 1 cannot take both.
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(21));
    let document: ScenarioDocument = serde_json::from_value(value.clone())?;
    let mut compile_context = context();
    compile_context.scenario_revision = 17;
    let compiled = compile_workforce(&document, &compile_context)?;
    let model_hash = canonical_ir_hash(&compiled.problem, PlanningIrLimitsV1::DEFAULT)?;
    let summary = summarize(&compiled.problem, PlanningIrLimitsV1::DEFAULT)?;
    let result = solve_with_real_worker(compiled.problem.clone()).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed,
        "unexpected real-worker outcome: {:?}",
        result.outcome
    );
    assert_eq!(result.candidates.len(), 1);
    let candidate = &result.candidates[0];
    assert_eq!(
        candidate
            .objective
            .as_ref()
            .ok_or("missing backend rank evidence")?
            .objective_values,
        vec![5]
    );
    let solution_id = id(500).parse()?;
    // The generic projection is the reference result for the domain-specific projector.
    let solution = project_candidate(
        &compiled.problem,
        &candidate.values,
        solution_id,
        PlanningIrLimitsV1::DEFAULT,
    )?;
    let workforce_solution = eutheto_workforce::assignment_rules::project_workforce_candidate(
        &compiled.problem,
        &candidate.values,
        solution_id,
        PlanningIrLimitsV1::DEFAULT,
        &context().control,
    )?;
    assert_eq!(workforce_solution, solution);
    let structural = eutheto_verify::validate_structure(
        &compiled.problem,
        &model_hash,
        &candidate.values,
        solution_id,
        &workforce_solution,
    )
    .map_err(|failure| failure.code)?;
    assert_eq!(structural.assignment_count, 3);
    assert_eq!(solution.scenario_id, document.scenario_id);
    assert_eq!(solution.scenario_revision, 17);
    assert_complete_workforce_pairs(&document, &solution)?;
    // Source-universe ranks are [1,2,3,4], including rejected (20,8), hence 1+4.
    // These are backend/model evidence, not independently accepted Workforce scores.
    let [level] = compiled.problem.objectives.levels.as_slice() else {
        return Err("expected one rank objective".into());
    };
    assert_eq!((level.lower_bound, level.upper_bound), (0, 7));
    let mut evaluated_rank = 0;
    for term in &level.terms {
        evaluated_rank += term.expression.constant;
        for coefficient in &term.expression.terms {
            evaluated_rank += coefficient.coefficient
                * candidate
                    .values
                    .integers
                    .get(&coefficient.variable)
                    .ok_or("missing rank integer")?;
        }
    }
    assert_eq!(evaluated_rank, 5);

    assert_complete_projection_mutations(
        &compiled.problem,
        &model_hash,
        &candidate.values,
        &solution,
    );
    assert_real_workforce_acceptance(
        &document,
        &compiled.problem,
        candidate,
        &solution,
        &model_hash,
    )?;
    assert_real_worker_coverage_mutant_quarantined(&document, &compiled.problem).await?;
    eprintln!(
        "WF007 unregistered worker evidence: accepted rank=5 feasibility=0, source-bound share, weakened-coverage candidate quarantined, model_hash={model_hash}, summary={summary:?}"
    );

    value["domain"]["entities"][id(20)]["eligibleAssignmentTypeIds"] = json!([]);
    let impossible = serde_json::from_value(value)?;
    let impossible = compile_workforce(&impossible, &compile_context)?;
    let result = solve_with_real_worker(impossible.problem).await?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::InfeasibilityClaimed,
        "unexpected real-worker infeasibility outcome: {:?}",
        result.outcome
    );
    assert!(result.candidates.is_empty());
    Ok(())
}

fn assert_real_workforce_acceptance(
    document: &ScenarioDocument,
    problem: &PlanningProblem,
    candidate: &eutheto_solver_api::BackendCandidate,
    solution: &eutheto_domain_ir::NormalizedSolution,
    model_hash: &str,
) -> TestResult {
    let clock = eutheto_verify::SystemVerificationClock::default();
    let reviewer = eutheto_verify::AcceptanceReviewer::new(
        &eutheto_workforce::WorkforcePack,
        document,
        17,
        problem,
        None,
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Accepted {
        result,
        objective_reconciliation,
        ..
    } = reviewer.review(candidate, solution.solution_id, &context().control)
    else {
        return Err("real Workforce candidate was not independently accepted".into());
    };
    assert_eq!(
        objective_reconciliation,
        eutheto_verify::BackendObjectiveReconciliation::Matched
    );
    assert_eq!(result.solution, *solution);
    assert_eq!(result.verification.score.feasibility, 0);
    assert_eq!(result.verification.score.levels[0].value, 5);
    assert_eq!(result.verification.planning_model_hash, model_hash);
    for include_evidence_references in [false, true] {
        let share = eutheto_workforce::assignment_rules::build_workforce_share_result(
            document,
            &result,
            None,
            eutheto_domain_api::ShareResultOptions {
                include_evidence_references,
            },
        )?;
        assert_eq!(
            share.payload["assignments"]
                .as_array()
                .ok_or("share assignments")?
                .len(),
            2
        );
        assert_eq!(
            share.payload.get("evidenceReferences").is_some(),
            include_evidence_references
        );
    }
    Ok(())
}

async fn assert_real_worker_coverage_mutant_quarantined(
    document: &ScenarioDocument,
    original: &PlanningProblem,
) -> TestResult {
    let mut problem = original.clone();
    authority::weaken_coverage(&mut problem)?;
    let output = solve_with_real_worker(problem.clone()).await?;
    let candidate = output
        .candidates
        .first()
        .ok_or("mutant must yield a real candidate")?;
    let clock = eutheto_verify::SystemVerificationClock::default();
    let reviewer = eutheto_verify::AcceptanceReviewer::new(
        &eutheto_workforce::WorkforcePack,
        document,
        17,
        &problem,
        None,
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Quarantined { alarm, .. } =
        reviewer.review(candidate, id(501).parse()?, &context().control)
    else {
        return Err("bad compiler semantics crossed real-worker acceptance".into());
    };
    assert_eq!(
        alarm.category,
        eutheto_verify::CorrectnessAlarmCategory::RequiredRuleRejected
    );
    Ok(())
}

fn assert_complete_workforce_pairs(
    document: &ScenarioDocument,
    solution: &eutheto_domain_ir::NormalizedSolution,
) -> TestResult {
    let mut selected = BTreeSet::new();
    let mut observed = BTreeMap::new();
    for assignment in &solution.assignments {
        let (person, shift) = assignment
            .entity
            .id
            .as_str()
            .split_once('.')
            .ok_or("pair encoding")?;
        let pair = AssignmentPair {
            person_id: person.parse()?,
            shift_id: shift.parse()?,
        };
        let AssignmentValue::Boolean(chosen) = assignment.value else {
            return Err("rank auxiliaries or absent values leaked into assignments".into());
        };
        assert_eq!(
            assignment.entity.kind.as_str(),
            "official.workforce.assignment"
        );
        assert_eq!(
            assignment.id.as_str(),
            format!(
                "official.workforce.assignment.{}.{}",
                pair.person_id, pair.shift_id
            )
        );
        observed.insert(pair, chosen);
        if chosen {
            selected.insert(pair);
        }
    }
    assert_eq!(
        observed,
        BTreeMap::from([
            (pair(1, 8)?, true),
            (pair(1, 22)?, false),
            (pair(20, 22)?, true),
        ])
    );
    assert_eq!(selected, BTreeSet::from([pair(1, 8)?, pair(20, 22)?]));
    assert!(original_accepts(
        document,
        &selected.into_iter().collect::<Vec<_>>()
    )?);
    Ok(())
}

fn assert_complete_projection_mutations(
    problem: &PlanningProblem,
    model_hash: &str,
    values: &eutheto_planning_ir::CandidateValues,
    solution: &eutheto_domain_ir::NormalizedSolution,
) {
    let solution_id = solution.solution_id;
    let mut omitted_false = solution.clone();
    omitted_false
        .assignments
        .retain(|assignment| assignment.value != AssignmentValue::Boolean(false));
    assert!(
        eutheto_verify::validate_structure(
            problem,
            model_hash,
            values,
            solution_id,
            &omitted_false,
        )
        .is_err()
    );
    let mut stale = solution.clone();
    stale.scenario_revision -= 1;
    assert!(
        eutheto_verify::validate_structure(problem, model_hash, values, solution_id, &stale,)
            .is_err()
    );
    for variable in values.booleans.keys() {
        let mut missing = values.clone();
        missing.booleans.remove(variable);
        assert!(
            project_candidate(problem, &missing, solution_id, PlanningIrLimitsV1::DEFAULT,)
                .is_err()
        );
    }
}
