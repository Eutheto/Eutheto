#![forbid(unsafe_code)]

//! Cross-layer WF-003 through WF-007 evidence; production pack registration remains out of scope.

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

#[path = "workforce_assignment_rules/authority.rs"]
mod authority;

use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::AssignmentValue;
use eutheto_planning_ir::{
    BoolVariableId, CompilerId, Constraint, Literal, ObjectivePlan, PLANNING_IR_SCHEMA_VERSION,
    PROJECTION_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningMetadata, PlanningProblem, Variable,
    canonical_ir_hash, feature_usage, project_candidate, summarize,
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
    PreservationPolicy, ReproducibilityMode, ResourceLimits, ScenarioDocument, SolveMode,
    SolveOptions, SystemMonotonicClock, WorkerThreadPolicy,
};
use eutheto_workforce::{
    assignment_rules::{
        AssignmentRuleCompilation, compile_assignment_rules, compile_workforce,
        evaluate_assignment_rules,
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

fn context() -> CompileContext {
    CompileContext {
        scenario_revision: 1,
        semantic_metadata: BTreeMap::new(),
        cancellation: CancellationToken::new(),
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

fn detects_mutation(document: &ScenarioDocument, mutant: &AssignmentRuleCompilation) -> TestResult {
    for selected in selections()? {
        if mathematics_accepts(mutant, &selected)? && !original_accepts(document, &selected)? {
            return Ok(());
        }
    }
    Err("independent evaluator failed to detect weakened compiler mathematics".into())
}

#[test]
fn original_evaluator_detects_pruning_overlap_and_coverage_mutants() -> TestResult {
    let document: ScenarioDocument = serde_json::from_value(fixture_value()?)?;
    let compiled = compile_assignment_rules(&document, &context())?;
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
    assert!(backend.compatibility(&summary, &options).compatible());
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
        &budget,
        None,
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
    )?;
    assert!(original_accepts(&change.document, &selected)?);
    let undone = WorkforcePack.apply_batch(&change.document, &change.inverse)?;
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
        BackendTerminationReason::OptimalityClaimed
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
        BackendTerminationReason::InfeasibilityClaimed
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
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Accepted {
        result,
        objective_reconciliation,
        ..
    } = reviewer.review(candidate, solution.solution_id)
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
        &clock,
    )
    .map_err(|alarm| alarm.diagnostic_code)?;
    let eutheto_verify::AcceptanceDecision::Quarantined { alarm, .. } =
        reviewer.review(candidate, id(501).parse()?)
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
