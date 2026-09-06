#![forbid(unsafe_code)]

//! Cross-layer evidence for the WF-003 contribution, not registered-pack acceptance or scoring.

#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

use eutheto_domain_api::CompileContext;
use eutheto_planning_ir::{
    BoolVariableId, CompilerId, Constraint, Literal, ObjectivePlan, PLANNING_IR_SCHEMA_VERSION,
    PROJECTION_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningMetadata, PlanningProblem, Variable,
    feature_usage, summarize,
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
        AssignmentRuleCompilation, compile_assignment_rules, evaluate_assignment_rules,
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
    Ok(result.evaluations.iter().all(|item| item.satisfied))
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
