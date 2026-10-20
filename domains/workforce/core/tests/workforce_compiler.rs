#![forbid(unsafe_code)]

//! Exhaustive mathematics for the unregistered complete compiler, not accepted scores.

mod support;

use eutheto_domain_api::{
    CompileContext, CounterfactualCompileContext, DomainPack, DomainPackError,
};
use eutheto_domain_ir::{
    AssignmentValue, CounterfactualConditionPayloadV1, CounterfactualConditionV1,
    OptimizationDirection,
};
use eutheto_planning_ir::{
    BoolVariableId, CandidateValues, ComparisonOp, Constraint, LinearExpression, Literal,
    PlanningIrLimitsV1, PlanningProblem, ProjectionExpression, Variable, canonical_ir_hash,
    project_candidate, validate,
};
use eutheto_types::{
    CancellationToken, DurationMillis, FixedMonotonicClock, ParentSolveBudget, ScenarioDocument,
};
use eutheto_workforce::{
    WorkforcePack,
    assignment_rules::{compile_workforce, evaluate_assignment_rules},
    model::AssignmentPair,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    sync::Arc,
};
use support::id;

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

fn context() -> CompileContext {
    CompileContext {
        objective_context: None,
        scenario_revision: 17,
        semantic_metadata: BTreeMap::new(),
        control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
        planning_limits: PlanningIrLimitsV1::DEFAULT,
    }
}

fn endpoint(time: &str) -> Value {
    json!({"instant":format!("2026-11-01T{time}Z"),"local":format!("2026-11-01T{time}"),"offsetSeconds":0})
}

fn fixture() -> Result<Value> {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["lockedAssignments"] = json!({});
    let entities = value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?;
    entities.remove(&id(6));
    for person in [20, 21] {
        let mut record = entities[&id(1)].clone();
        record["id"] = json!(id(person));
        record["name"] = json!(format!("Person {person}"));
        record["externalId"] = json!(format!("staff-{person}"));
        if person == 21 {
            record["qualificationGrants"] = json!([]);
        }
        entities.insert(id(person), record);
    }
    let mut shift = entities[&id(8)].clone();
    shift["startsAt"] = endpoint("08:00:00");
    shift["endsAt"] = endpoint("10:00:00");
    entities.insert(id(8), shift.clone());
    shift["id"] = json!(id(22));
    shift["startsAt"] = endpoint("19:00:00");
    shift["endsAt"] = endpoint("21:00:00");
    entities.insert(id(22), shift);
    entities.insert(id(23), json!({
        "kind":"availability","id":id(23),"personId":id(20),"availabilityKind":"unavailable",
        "timeWindow":{"kind":"instant","startsAt":"2026-11-01T08:00:00Z","endsAt":"2026-11-01T08:30:00Z"},
        "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},"source":"manual","note":""
    }));
    for (index, kind) in [
        (30, "eligibility"),
        (31, "availability"),
        (32, "coverage"),
        (33, "noOverlap"),
        (34, "minimumRest"),
    ] {
        let mut rule = json!({"id":id(index),"kind":kind,"active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
        if kind == "noOverlap" {
            rule["compatibleCategoryPairs"] = json!([]);
        }
        if kind == "minimumRest" {
            rule["afterScope"] = json!({"people":{"kind":"all"}});
            rule["beforeScope"] = json!({"people":{"kind":"all"}});
            rule["minimumMinutes"] = json!(600);
        }
        value["domain"]["rules"][id(index)] = rule;
    }
    Ok(value)
}

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(person).parse()?,
        shift_id: id(shift).parse()?,
    })
}

// These fixtures contain only manual, in-horizon shifts. Read the original source IDs,
// never the compiler's candidates, provenance rank parameter, or objective coefficients.
fn original_universe(document: &ScenarioDocument) -> Result<Vec<AssignmentPair>> {
    let mut people = Vec::new();
    let mut shifts = Vec::new();
    for entity in document.domain.entities.values() {
        let text = entity["id"].as_str().ok_or("source identity")?;
        match entity["kind"].as_str() {
            Some("person") => people.push(text.parse()?),
            Some("shiftInstance") => shifts.push(text.parse()?),
            Some("shiftTemplate") => {
                return Err("fixture must resolve templates before using this oracle".into());
            }
            _ => {}
        }
    }
    people.sort();
    shifts.sort();
    Ok(people
        .into_iter()
        .flat_map(|person_id| {
            shifts.iter().map(move |&shift_id| AssignmentPair {
                person_id,
                shift_id,
            })
        })
        .collect())
}

fn rank(universe: &[AssignmentPair], selected: &[AssignmentPair]) -> Result<i64> {
    selected.iter().try_fold(0, |sum, pair| {
        let index = universe
            .iter()
            .position(|entry| entry == pair)
            .ok_or("pair outside source universe")?;
        Ok(sum + i64::try_from(index)? + 1)
    })
}

fn selection(universe: &[AssignmentPair], mask: usize) -> Vec<AssignmentPair> {
    universe
        .iter()
        .enumerate()
        .filter_map(|(index, pair)| (mask & (1 << index) != 0).then_some(*pair))
        .collect()
}

fn projected_pairs(problem: &PlanningProblem) -> Result<BTreeMap<AssignmentPair, BoolVariableId>> {
    problem
        .projections
        .iter()
        .map(|projection| {
            let (person, shift) = projection
                .entity
                .id
                .as_str()
                .split_once('.')
                .ok_or("pair identity")?;
            let pair = AssignmentPair {
                person_id: person.parse()?,
                shift_id: shift.parse()?,
            };
            let ProjectionExpression::Boolean(variable) = &projection.expression else {
                return Err("assignment must project a Boolean".into());
            };
            Ok((pair, variable.clone()))
        })
        .collect()
}

fn literal(literal: &Literal, values: &CandidateValues) -> Result<bool> {
    Ok(*values
        .booleans
        .get(&literal.variable)
        .ok_or("missing Boolean")?
        == literal.positive)
}

fn linear(expression: &LinearExpression, values: &CandidateValues) -> Result<i64> {
    expression
        .terms
        .iter()
        .try_fold(expression.constant, |sum, term| {
            Ok(sum
                + term.coefficient
                    * values
                        .integers
                        .get(&term.variable)
                        .ok_or("missing integer")?)
        })
}

// Deliberately limited to the emitted primitive vocabulary. It knows no Workforce rule.
fn accepts(problem: &PlanningProblem, values: &CandidateValues) -> Result<bool> {
    for variable in &problem.variables {
        match variable {
            Variable::Boolean(variable) => {
                values.booleans.get(&variable.id).ok_or("missing Boolean")?;
            }
            Variable::Integer(variable) => {
                let value = values.integers.get(&variable.id).ok_or("missing integer")?;
                if !variable
                    .domain
                    .inclusive_ranges
                    .iter()
                    .any(|range| range.start <= *value && *value <= range.end)
                {
                    return Ok(false);
                }
            }
            Variable::Interval(_) => return Err("unexpected interval".into()),
        }
    }
    for record in &problem.constraints {
        let mut enforced = true;
        for item in &record.enforcement {
            enforced &= literal(item, values)?;
        }
        if !enforced {
            continue;
        }
        let (literals, min, max) = match &record.body {
            Constraint::LinearComparison(comparison) => {
                let lhs = linear(&comparison.expression, values)?;
                let satisfied = match comparison.op {
                    ComparisonOp::Equal => lhs == comparison.rhs,
                    ComparisonOp::LessOrEqual => lhs <= comparison.rhs,
                    ComparisonOp::GreaterOrEqual => lhs >= comparison.rhs,
                };
                if !satisfied {
                    return Ok(false);
                }
                continue;
            }
            Constraint::BoolOr { literals } => (literals, 1, u64::MAX),
            Constraint::BoolAnd { literals } => {
                (literals, u64::try_from(literals.len())?, u64::MAX)
            }
            Constraint::AtMostOne { literals } => (literals, 0, 1),
            Constraint::ExactlyOne { literals } => (literals, 1, 1),
            Constraint::CardinalityRange { literals, min, max } => (literals, *min, *max),
            _ => return Err("unexpected complete-model primitive".into()),
        };
        let mut count = 0;
        for item in literals {
            count += u64::from(literal(item, values)?);
        }
        if count < min || count > max {
            return Ok(false);
        }
    }
    Ok(true)
}

fn objective(problem: &PlanningProblem, values: &CandidateValues) -> Result<i64> {
    let [level] = problem.objectives.levels.as_slice() else {
        return Err("expected one final rank level".into());
    };
    assert_eq!(
        level.id.as_str(),
        "official.workforce.objective.assignment.rank"
    );
    assert_eq!(level.direction, OptimizationDirection::Minimize);
    level.terms.iter().try_fold(0, |sum, term| {
        assert_eq!(
            term.category.as_str(),
            "official.workforce.score.assignment.rank"
        );
        Ok(sum + linear(&term.expression, values)?)
    })
}

fn extensions(
    problem: &PlanningProblem,
    selected: &[AssignmentPair],
) -> Result<Vec<CandidateValues>> {
    const MAX_EXTENSIONS: usize = 16_384;
    let pairs = projected_pairs(problem)?;
    if selected.iter().any(|pair| !pairs.contains_key(pair)) {
        return Ok(Vec::new());
    }
    let mut booleans: BTreeMap<_, _> = pairs
        .into_iter()
        .map(|(pair, variable)| (variable, selected.contains(&pair)))
        .collect();
    let helpers: Vec<_> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Boolean(variable) if !booleans.contains_key(&variable.id) => {
                Some(variable.id.clone())
            }
            _ => None,
        })
        .collect();
    for helper in &helpers {
        booleans.insert(helper.clone(), false);
    }
    let integers: Vec<_> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Integer(variable) => Some(variable),
            _ => None,
        })
        .collect();
    assert!(
        integers.len() <= 8,
        "only exhaustively enumerate tiny fixtures"
    );
    let domains = integers
        .iter()
        .map(|variable| {
            let mut domain = Vec::new();
            for range in &variable.domain.inclusive_ranges {
                let width = usize::try_from(i128::from(range.end) - i128::from(range.start) + 1)?;
                assert!(width > 0 && width <= 4096 && domain.len() + width <= 4096);
                domain.extend(range.start..=range.end);
            }
            Ok(domain)
        })
        .collect::<Result<Vec<_>>>()?;
    // Include every helper Boolean and full integer domain, not just the expected state.
    let boolean_combinations = helpers.iter().try_fold(1_usize, |count, _| {
        let next = count.checked_mul(2).ok_or("fixture Boolean product")?;
        assert!(next <= MAX_EXTENSIONS, "only enumerate tiny helper domains");
        Ok::<_, Box<dyn Error>>(next)
    })?;
    let combinations = domains
        .iter()
        .try_fold(boolean_combinations, |count, domain| {
            let next = count
                .checked_mul(domain.len())
                .ok_or("fixture domain product")?;
            assert!(next <= MAX_EXTENSIONS, "only enumerate tiny helper domains");
            Ok::<_, Box<dyn Error>>(next)
        })?;
    let mut values = CandidateValues {
        booleans,
        integers: integers
            .iter()
            .map(|variable| (variable.id.clone(), 0))
            .collect(),
    };
    let mut accepted = Vec::new();
    for mut index in 0..combinations {
        for helper in &helpers {
            *values.booleans.get_mut(helper).ok_or("fixture Boolean")? = index % 2 != 0;
            index /= 2;
        }
        for (variable, domain) in integers.iter().zip(&domains) {
            *values
                .integers
                .get_mut(&variable.id)
                .ok_or("fixture integer")? = domain[index % domain.len()];
            index /= domain.len();
        }
        if accepts(problem, &values)? {
            accepted.push(values.clone());
        }
    }
    Ok(accepted)
}

fn original_accepts(document: &ScenarioDocument, selected: &[AssignmentPair]) -> Result<bool> {
    let evaluation = evaluate_assignment_rules(document, selected, None)?;
    assert!(evaluation.obligations.remaining.is_empty());
    Ok(evaluation.evaluations.iter().all(|rule| rule.satisfied))
}

fn feasible_models(
    document: &ScenarioDocument,
    problem: &PlanningProblem,
) -> Result<BTreeMap<Vec<AssignmentPair>, i64>> {
    let universe = original_universe(document)?;
    let retained: Vec<_> = projected_pairs(problem)?.into_keys().collect();
    let [level] = problem.objectives.levels.as_slice() else {
        return Err("rank level".into());
    };
    assert_eq!(
        (level.lower_bound, level.upper_bound),
        (0, rank(&universe, &retained)?)
    );
    let mut feasible = BTreeMap::new();
    for mask in 0..(1 << universe.len()) {
        let selected = selection(&universe, mask);
        let expected = original_accepts(document, &selected)?;
        let observed = extensions(problem, &selected)?;
        assert_eq!(!observed.is_empty(), expected, "selection {selected:?}");
        for values in observed {
            let expected_rank = rank(&universe, &selected)?;
            assert_eq!(objective(problem, &values)?, expected_rank);
            assert!((level.lower_bound..=level.upper_bound).contains(&expected_rank));
            feasible.insert(selected.clone(), expected_rank);
        }
    }
    Ok(feasible)
}

#[test]
fn complete_model_matches_source_and_unpruned_reference() -> Result {
    let value = fixture()?;
    let document = serde_json::from_value(value.clone())?;
    let compiled = compile_workforce(&document, &context())?;
    let feasible = feasible_models(&document, &compiled.problem)?;
    assert_eq!(
        feasible,
        BTreeMap::from([(vec![pair(1, 8)?, pair(20, 22)?], 5)])
    );

    // Build the same hard coverage/rest model without unary pruning, then fix every
    // source-rejected pair false. This is an actual unpruned model, not a count oracle.
    let mut unpruned = value.clone();
    for rule in [30, 31] {
        unpruned["domain"]["rules"][id(rule)]["active"] = json!(false);
    }
    let reference = compile_workforce(&serde_json::from_value(unpruned)?, &context())?;
    let universe = original_universe(&document)?;
    assert_eq!(
        projected_pairs(&reference.problem)?
            .into_keys()
            .collect::<Vec<_>>(),
        universe
    );
    let rejected: BTreeSet<_> = compiled.rejections.iter().map(|entry| entry.pair).collect();
    assert_eq!(
        rejected,
        BTreeSet::from([pair(20, 8)?, pair(21, 8)?, pair(21, 22)?])
    );
    let mut reference_feasible = BTreeMap::new();
    for mask in 0..(1 << universe.len()) {
        let selected = selection(&universe, mask);
        if selected.iter().any(|pair| rejected.contains(pair)) {
            continue;
        }
        for values in extensions(&reference.problem, &selected)? {
            let original_rank = rank(&universe, &selected)?;
            assert_eq!(objective(&reference.problem, &values)?, original_rank);
            reference_feasible.insert(selected.clone(), original_rank);
        }
    }
    assert_eq!(reference_feasible, feasible);

    let mut impossible = value;
    impossible["domain"]["entities"][id(20)]["eligibleAssignmentTypeIds"] = json!([]);
    let impossible = serde_json::from_value(impossible)?;
    let compiled = compile_workforce(&impossible, &context())?;
    assert!(feasible_models(&impossible, &compiled.problem)?.is_empty());
    Ok(())
}

#[test]
fn every_boolean_selection_has_exactly_one_rank_extension_and_required_projection() -> Result {
    let mut value = fixture()?;
    value["domain"]["rules"] = json!({});
    let document = serde_json::from_value(value)?;
    let compiled = compile_workforce(&document, &context())?;
    let universe = original_universe(&document)?;
    assert_eq!(feasible_models(&document, &compiled.problem)?.len(), 64);
    for mask in 0..64 {
        let selected = selection(&universe, mask);
        let mut observed = extensions(&compiled.problem, &selected)?;
        assert_eq!(observed.len(), 1);
        let values = observed.pop().ok_or("missing extension")?;
        let solution = project_candidate(
            &compiled.problem,
            &values,
            id(500).parse()?,
            PlanningIrLimitsV1::DEFAULT,
        )?;
        let expected: BTreeMap<_, _> = universe
            .iter()
            .map(|pair| {
                (
                    format!(
                        "official.workforce.assignment.{}.{}",
                        pair.person_id, pair.shift_id
                    ),
                    AssignmentValue::Boolean(selected.contains(pair)),
                )
            })
            .collect();
        assert_eq!(
            solution
                .assignments
                .iter()
                .map(|item| (item.id.to_string(), item.value.clone()))
                .collect::<BTreeMap<_, _>>(),
            expected
        );
        for variable in values.booleans.keys() {
            let mut missing = values.clone();
            missing.booleans.remove(variable);
            assert!(
                project_candidate(
                    &compiled.problem,
                    &missing,
                    id(500).parse()?,
                    PlanningIrLimitsV1::DEFAULT
                )
                .is_err()
            );
        }
        for variable in values.integers.keys() {
            for outside in [-1, 2] {
                let mut invalid = values.clone();
                invalid.integers.insert(variable.clone(), outside);
                assert!(!accepts(&compiled.problem, &invalid)?);
                assert!(
                    project_candidate(
                        &compiled.problem,
                        &invalid,
                        id(500).parse()?,
                        PlanningIrLimitsV1::DEFAULT
                    )
                    .is_err()
                );
            }
        }
    }
    Ok(())
}

#[test]
fn equal_sum_rank_ties_preserve_distinct_people() -> Result {
    let mut value = fixture()?;
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(21));
    value["domain"]["rules"][id(31)]["active"] = json!(false);
    let document = serde_json::from_value(value)?;
    let compiled = compile_workforce(&document, &context())?;
    assert_eq!(
        feasible_models(&document, &compiled.problem)?,
        BTreeMap::from([
            (vec![pair(1, 8)?, pair(20, 22)?], 5),
            (vec![pair(1, 22)?, pair(20, 8)?], 5),
        ])
    );
    Ok(())
}

#[test]
fn inactive_and_noncandidate_source_people_keep_rank_slots_without_variables() -> Result {
    let value = fixture()?;
    let before_document = serde_json::from_value(value.clone())?;
    let before = compile_workforce(&before_document, &context())?;
    let selected = vec![pair(1, 8)?, pair(20, 22)?];
    assert_eq!(rank(&original_universe(&before_document)?, &selected)?, 5);
    assert_eq!(
        feasible_models(&before_document, &before.problem)?,
        BTreeMap::from([(selected.clone(), 5)])
    );
    for inactive in [false, true] {
        let mut augmented = value.clone();
        let mut noncandidate = augmented["domain"]["entities"][id(1)].clone();
        noncandidate["id"] = json!(id(15));
        noncandidate["externalId"] = json!("never-candidate");
        if inactive {
            noncandidate["activeRange"] = json!({"kind":"dateRange","startDate":"2026-10-01","endDateExclusive":"2026-11-01"});
        } else {
            noncandidate["eligibleAssignmentTypeIds"] = json!([]);
        }
        augmented["domain"]["entities"][id(15)] = noncandidate;
        let after_document = serde_json::from_value(augmented)?;
        let after = compile_workforce(&after_document, &context())?;
        assert_eq!(
            projected_pairs(&before.problem)?,
            projected_pairs(&after.problem)?
        );
        assert_eq!(before.problem.variables, after.problem.variables);
        assert_eq!(rank(&original_universe(&after_document)?, &selected)?, 7);
        assert_eq!(
            feasible_models(&after_document, &after.problem)?,
            BTreeMap::from([(selected.clone(), 7)])
        );
    }
    Ok(())
}

#[test]
fn display_and_collection_order_preserve_semantics_but_revision_and_people_bind() -> Result {
    let value = fixture()?;
    let document = serde_json::from_value(value.clone())?;
    let compiled = compile_workforce(&document, &context())?;
    let hash = canonical_ir_hash(&compiled.problem, PlanningIrLimitsV1::DEFAULT)?;
    let mut renamed = value.clone();
    renamed["metadata"]["title"] = json!("Renamed scenario");
    renamed["domain"]["entities"][id(1)]["name"] = json!("Zulu");
    renamed["domain"]["entities"][id(20)]["name"] = json!("Alpha");
    for collection in ["entities", "rules"] {
        let records = renamed["domain"][collection]
            .as_object_mut()
            .ok_or("collection")?;
        let reverse: Vec<_> = records
            .iter()
            .rev()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        records.clear();
        records.extend(reverse);
    }
    // Also permute an authored collection, not only map insertion order.
    renamed["domain"]["entities"][id(9)]["priorityMapping"]
        .as_array_mut()
        .ok_or("priority mapping")?
        .reverse();
    let renamed = compile_workforce(&serde_json::from_value(renamed)?, &context())?;
    assert_ne!(compiled.source_document_hash, renamed.source_document_hash);
    assert_eq!(
        canonical_ir_hash(&renamed.problem, PlanningIrLimitsV1::DEFAULT)?,
        hash
    );
    assert_eq!(compiled.problem.variables, renamed.problem.variables);
    assert_eq!(compiled.problem.constraints, renamed.problem.constraints);
    assert_eq!(compiled.problem.objectives, renamed.problem.objectives);
    assert_eq!(compiled.problem.projections, renamed.problem.projections);

    let mut revised_context = context();
    revised_context.scenario_revision += 1;
    let revised = compile_workforce(&document, &revised_context)?;
    assert_eq!(revised.source_document_hash, compiled.source_document_hash);
    assert_ne!(
        canonical_ir_hash(&revised.problem, PlanningIrLimitsV1::DEFAULT)?,
        hash
    );
    let selected = vec![pair(1, 8)?, pair(20, 22)?];
    let values = extensions(&compiled.problem, &selected)?
        .pop()
        .ok_or("extension")?;
    for (problem, revision) in [(&compiled.problem, 17), (&revised.problem, 18)] {
        let solution = project_candidate(
            problem,
            &values,
            id(500).parse()?,
            PlanningIrLimitsV1::DEFAULT,
        )?;
        assert_eq!(
            (solution.scenario_id, solution.scenario_revision),
            (document.scenario_id, revision)
        );
    }

    // Names do not distinguish people, but availability ownership does: swapping the
    // unavailable person must swap the only valid typed schedule, not break symmetry.
    let mut swapped = value;
    swapped["domain"]["entities"][id(23)]["personId"] = json!(id(1));
    let swapped = serde_json::from_value(swapped)?;
    let swapped_model = compile_workforce(&swapped, &context())?;
    assert_ne!(
        canonical_ir_hash(&swapped_model.problem, PlanningIrLimitsV1::DEFAULT)?,
        hash
    );
    assert_eq!(
        feasible_models(&swapped, &swapped_model.problem)?,
        BTreeMap::from([(vec![pair(1, 22)?, pair(20, 8)?], 5)])
    );
    Ok(())
}

#[test]
fn replacement_condition_changes_only_the_requested_shift_pins_and_retains_source_rules() -> Result
{
    let mut value = fixture()?;
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(23));
    // Minimum coverage allows both people on the same shift: forcing a target alone
    // is not a replacement. Preserve all source Required rules, including rest.
    value["domain"]["entities"][id(8)]["coverage"] =
        json!({"kind":"atLeast","minimum":1,"qualificationMinimums":[]});
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compile_context = context();
    let base = WorkforcePack.compile(&document, &compile_context)?;
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    let phase = budget.phase_view();
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: format!("official.workforce.assignment.{}.{}", id(20), id(8))
                .parse()?,
            replaced_assignment_id: format!("official.workforce.assignment.{}.{}", id(1), id(8))
                .parse()?,
        })?;
    let compile = |condition: &CounterfactualConditionV1, original: &PlanningProblem| {
        WorkforcePack.compile_counterfactual(
            &document,
            condition,
            &CounterfactualCompileContext {
                base_problem: original,
                compile_context: &compile_context,
                budget: phase.clone(),
            },
        )
    };
    let derived = compile(&condition, &base)?;
    assert_eq!(base.variables, derived.variables);
    assert_eq!(base.objectives, derived.objectives);
    assert_eq!(base.projections, derived.projections);
    assert_eq!(base.declared_capabilities, derived.declared_capabilities);
    assert_eq!(base.constraints.len() + 2, derived.constraints.len());
    assert_ne!(
        canonical_ir_hash(&base, PlanningIrLimitsV1::DEFAULT)?,
        canonical_ir_hash(&derived, PlanningIrLimitsV1::DEFAULT)?,
    );
    let universe = original_universe(&document)?;
    let mut found_alternative = false;
    for mask in 0..(1 << universe.len()) {
        let selected = selection(&universe, mask);
        let base_candidates = extensions(&base, &selected)?;
        let replacement = extensions(&derived, &selected)?;
        if base_candidates.is_empty() {
            assert!(replacement.is_empty());
            continue;
        }
        let matches_condition =
            selected.contains(&pair(20, 8)?) && !selected.contains(&pair(1, 8)?);
        assert_eq!(
            replacement.len(),
            usize::from(matches_condition),
            "{selected:?}"
        );
        if matches_condition {
            found_alternative = true;
            assert!(original_accepts(&document, &selected)?);
        }
    }
    assert!(found_alternative);

    let wrong_shift =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: format!("official.workforce.assignment.{}.{}", id(20), id(22))
                .parse()?,
            replaced_assignment_id: format!("official.workforce.assignment.{}.{}", id(1), id(8))
                .parse()?,
        })?;
    assert!(matches!(
        compile(&wrong_shift, &base),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    let missing =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: format!("official.workforce.assignment.{}.{}", id(99), id(8))
                .parse()?,
            replaced_assignment_id: format!("official.workforce.assignment.{}.{}", id(1), id(8))
                .parse()?,
        })?;
    assert!(matches!(
        compile(&missing, &base),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    Ok(())
}

#[test]
fn single_boolean_conditions_preserve_required_rules_and_invert_forbidden_values() -> Result {
    let mut value = fixture()?;
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(23));
    value["domain"]["entities"][id(8)]["coverage"] =
        json!({"kind":"atLeast","minimum":1,"qualificationMinimums":[]});
    let document: ScenarioDocument = serde_json::from_value(value)?;
    let compile_context = context();
    let base = WorkforcePack.compile(&document, &compile_context)?;
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    let universe = original_universe(&document)?;
    let target = pair(20, 8)?;
    let pair_id = format!("{}.{}", id(20), id(8));
    let assignment_id = base
        .projections
        .iter()
        .find(|projection| projection.entity.id.as_str() == pair_id)
        .ok_or("target projection")?
        .assignment_id
        .clone();
    for (forbid, selected_value) in [(false, false), (false, true), (true, false), (true, true)] {
        let value = AssignmentValue::Boolean(selected_value);
        let payload = if forbid {
            CounterfactualConditionPayloadV1::ForbidAssignmentValue {
                assignment_id: assignment_id.clone(),
                value,
            }
        } else {
            CounterfactualConditionPayloadV1::ForceAssignmentValue {
                assignment_id: assignment_id.clone(),
                value,
            }
        };
        let condition = CounterfactualConditionV1::new(payload)?;
        let derived = WorkforcePack.compile_counterfactual(
            &document,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &base,
                compile_context: &compile_context,
                budget: budget.phase_view(),
            },
        )?;
        for mask in 0..(1 << universe.len()) {
            let selected = selection(&universe, mask);
            let matches_condition = (selected.contains(&target) == selected_value) != forbid;
            let expected = matches_condition && original_accepts(&document, &selected)?;
            assert_eq!(
                !extensions(&derived, &selected)?.is_empty(),
                expected,
                "forbid={forbid}, value={selected_value}, selected={selected:?}",
            );
        }
    }
    Ok(())
}

#[test]
fn single_boolean_conditions_reject_unmodeled_values_and_exhausted_constraint_capacity() -> Result {
    let document: ScenarioDocument = serde_json::from_value(fixture()?)?;
    let compile_context = context();
    let base = WorkforcePack.compile(&document, &compile_context)?;
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    let compile = |condition: &CounterfactualConditionV1, compile_context: &CompileContext| {
        WorkforcePack.compile_counterfactual(
            &document,
            condition,
            &CounterfactualCompileContext {
                base_problem: &base,
                compile_context,
                budget: budget.phase_view(),
            },
        )
    };
    // A statically excluded pair has no saved Boolean decision. Requiring false
    // must not invent an assignment or claim that the absent decision is false.
    let unmodeled =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id: format!("official.workforce.assignment.{}.{}", id(20), id(8)).parse()?,
            value: AssignmentValue::Boolean(false),
        })?;
    assert!(matches!(
        compile(&unmodeled, &compile_context),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    let assignment_id = base
        .projections
        .first()
        .ok_or("projection")?
        .assignment_id
        .clone();
    let unsupported =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id: assignment_id.clone(),
            value: AssignmentValue::Integer(1),
        })?;
    assert!(matches!(
        compile(&unsupported, &compile_context),
        Err(DomainPackError::UnsupportedExplanationCapability(_))
    ));
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(true),
        })?;
    let mut limited = compile_context;
    limited.planning_limits.max_constraints = base.constraints.len() as u64;
    assert!(matches!(
        compile(&condition, &limited),
        Err(DomainPackError::ResourceLimitExceeded)
    ));
    Ok(())
}

#[test]
fn counterfactual_rejects_expanded_provenance_depth_before_resolving_condition() -> Result {
    let document: ScenarioDocument = serde_json::from_value(fixture()?)?;
    let mut compile_context = context();
    let mut base = WorkforcePack.compile(&document, &compile_context)?;
    let root = base
        .provenance
        .iter()
        .position(|record| record.parent.is_none())
        .ok_or("provenance root")?;
    let template = base.provenance[root].clone();
    let mut parent = None;
    for index in 0..=PlanningIrLimitsV1::DEFAULT.max_provenance_depth {
        let mut ancestor = template.clone();
        ancestor.id = format!("regression.ancestor.{index:03}").parse()?;
        ancestor.parent = parent;
        parent = Some(ancestor.id.clone());
        base.provenance.push(ancestor);
    }
    base.provenance[root].parent = parent;
    base.canonicalize()?;
    compile_context.planning_limits.max_provenance_depth = 100;
    // Every added ancestor remains covered. The only invalidity at normal limits is
    // excessive depth; expanded caller limits must not defer refusal to condition lookup.
    validate(&base, compile_context.planning_limits)?;
    let assignment_id = format!("official.workforce.assignment.{}.{}", id(20), id(8)).parse()?;
    assert!(
        !base
            .projections
            .iter()
            .any(|projection| projection.assignment_id == assignment_id)
    );
    let condition =
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(false),
        })?;
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        Arc::new(FixedMonotonicClock::default()),
        CancellationToken::new(),
    )?;
    assert!(matches!(
        WorkforcePack.compile_counterfactual(
            &document,
            &condition,
            &CounterfactualCompileContext {
                base_problem: &base,
                compile_context: &compile_context,
                budget: budget.phase_view(),
            },
        ),
        Err(DomainPackError::InvalidPayload { path, .. }) if path == "/context/baseProblem"
    ));
    Ok(())
}

fn quota_fixture() -> Result<Value> {
    let mut value = serde_json::to_value(support::fixture()?)?;
    value["settings"]["timeZone"] = json!("UTC");
    value["settings"]["horizon"] =
        json!({"start":"2026-11-01T00:00:00Z","end":"2026-11-02T00:00:00Z"});
    value["domain"]["lockedAssignments"] = json!({});
    value["domain"]["entities"]
        .as_object_mut()
        .ok_or("entities")?
        .remove(&id(6));
    value["domain"]["entities"][id(8)]["startsAt"] = endpoint("08:00:00");
    value["domain"]["rules"][id(20)] = json!({
        "id":id(20),"kind":"workloadQuota","active":true,"strength":"required",
        "scope":{"people":{"kind":"all"}},"calendarId":id(2),
        "membership":"intersection","measure":{"kind":"bucketMinutes","bucketId":id(3)},
        "minimum":60,"maximum":60
    });
    Ok(value)
}

#[test]
fn minute_quota_compilation_preserves_exact_nanosecond_boundaries() -> Result {
    let mut value = quota_fixture()?;
    for contribution in ["sum", "union"] {
        value["domain"]["entities"][id(3)]["overlappingContribution"] = json!(contribution);
        for (end, accepted) in [
            ("08:59:59.999999999", false),
            ("09:00:00", true),
            ("09:00:00.000000001", false),
        ] {
            value["domain"]["entities"][id(8)]["endsAt"] = endpoint(end);
            let document: ScenarioDocument = serde_json::from_value(value.clone())?;
            let mut compile_context = context();
            if accepted {
                // Whole seconds need only the actual 3600-second coefficient, not 1e9.
                compile_context.planning_limits.max_abs_coefficient = 3600;
            }
            if !accepted {
                let mut tight = compile_context.clone();
                tight.planning_limits.max_abs_coefficient = 3600;
                assert!(matches!(
                    compile_workforce(&document, &tight),
                    Err(
                        eutheto_workforce::assignment_rules::AssignmentRuleError::LimitExceeded(
                            eutheto_workforce::assignment_rules::AssignmentRuleLimit::PerRecord
                        )
                    )
                ));
            }
            let compiled = compile_workforce(&document, &compile_context)?;
            let expected = if accepted {
                BTreeMap::from([(vec![pair(1, 8)?], 1)])
            } else {
                BTreeMap::new()
            };
            assert_eq!(feasible_models(&document, &compiled.problem)?, expected);
            assert_eq!(original_accepts(&document, &[pair(1, 8)?])?, accepted);
            assert!(!original_accepts(&document, &[])?);
        }
    }
    Ok(())
}

#[test]
fn empty_quota_population_preserves_zero_and_positive_minima() -> Result {
    let mut value = quota_fixture()?;
    value["domain"]["entities"][id(8)]["endsAt"] = endpoint("09:00:00");
    value["domain"]["entities"][id(4)]["workloadBucketIds"] = json!([]);
    value["domain"]["rules"][id(20)]
        .as_object_mut()
        .ok_or("quota")?
        .remove("maximum");
    for minimum in [0, 1] {
        value["domain"]["rules"][id(20)]["minimum"] = json!(minimum);
        let document = serde_json::from_value(value.clone())?;
        let compiled = compile_workforce(&document, &context())?;
        let expected = if minimum == 0 {
            BTreeMap::from([(Vec::new(), 0), (vec![pair(1, 8)?], 1)])
        } else {
            BTreeMap::new()
        };
        assert_eq!(feasible_models(&document, &compiled.problem)?, expected);
    }
    Ok(())
}

#[test]
fn scheduled_hours_keep_signed_fold_credits_and_zero_bounds() -> Result {
    let endpoint = |instant: &str, local: &str, offset: i32| {
        json!({
            "instant":format!("2026-11-01T{instant}Z"),
            "local":format!("2026-11-01T{local}"),"offsetSeconds":offset
        })
    };
    for (start, start_local, end, end_local, nanos) in [
        ("05:30:00", "01:30:00", "06:30:00", "01:30:00", 0_i64),
        (
            "05:50:00",
            "01:50:00",
            "06:10:00",
            "01:10:00",
            -2_400_000_000_000,
        ),
        (
            "05:30:00.000000001",
            "01:30:00.000000001",
            "06:30:00",
            "01:30:00",
            -1,
        ),
        (
            "05:30:00",
            "01:30:00",
            "06:30:00.000000001",
            "01:30:00.000000001",
            1,
        ),
    ] {
        for (quota, maximum) in [(true, None), (true, Some(0)), (false, Some(0))] {
            let mut value = quota_fixture()?;
            value["settings"]["timeZone"] = json!("America/New_York");
            value["settings"]["horizon"] = json!({
                "start":"2026-11-01T04:00:00Z","end":"2026-11-02T05:00:00Z"
            });
            value["domain"]["entities"][id(3)]["overlappingContribution"] = json!("sum");
            value["domain"]["entities"][id(3)]["measurement"] = json!("scheduledMinutes");
            value["domain"]["entities"][id(8)]["startsAt"] = endpoint(start, start_local, -14400);
            value["domain"]["entities"][id(8)]["endsAt"] = endpoint(end, end_local, -18000);
            if quota {
                value["domain"]["rules"][id(20)]["membership"] = json!("reportingDate");
                value["domain"]["rules"][id(20)]["minimum"] = json!(0);
                if let Some(maximum) = maximum {
                    value["domain"]["rules"][id(20)]["maximum"] = json!(maximum);
                } else {
                    value["domain"]["rules"][id(20)]
                        .as_object_mut()
                        .ok_or("quota")?
                        .remove("maximum");
                }
            } else {
                value["domain"]["rules"][id(20)] = json!({
                    "id":id(20),"kind":"maximumHours","active":true,"strength":"required",
                    "scope":{"people":{"kind":"all"}},"bucketId":id(3),
                    "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},
                    "maximumMinutes":0
                });
            }
            let document = serde_json::from_value(value)?;
            let compiled = compile_workforce(&document, &context())?;
            let mut expected = BTreeMap::from([(Vec::new(), 0)]);
            if (!quota || nanos >= 0) && (maximum.is_none() || nanos <= 0) {
                expected.insert(vec![pair(1, 8)?], 1);
            }
            assert_eq!(feasible_models(&document, &compiled.problem)?, expected);
        }
    }
    Ok(())
}

#[test]
fn overlapping_hours_preserve_sum_and_union_at_fractional_caps() -> Result {
    for contribution in ["sum", "union"] {
        for fractional in [false, true] {
            for quota in [false, true] {
                let mut value = quota_fixture()?;
                value["domain"]["entities"][id(3)]["overlappingContribution"] = json!(contribution);
                value["domain"]["entities"][id(8)]["endsAt"] = endpoint("09:00:00");
                let mut other = value["domain"]["entities"][id(8)].clone();
                other["id"] = json!(id(22));
                other["startsAt"] = endpoint("08:30:00");
                other["endsAt"] = endpoint(if fractional {
                    "09:30:00.000000001"
                } else {
                    "09:30:00"
                });
                value["domain"]["entities"][id(22)] = other;
                if quota {
                    value["domain"]["rules"][id(20)]["minimum"] = json!(0);
                    value["domain"]["rules"][id(20)]["maximum"] = json!(90);
                } else {
                    value["domain"]["rules"][id(20)] = json!({
                        "id":id(20),"kind":"maximumHours","active":true,"strength":"required",
                        "scope":{"people":{"kind":"all"}},"bucketId":id(3),
                        "window":{"kind":"calendar","calendarId":id(2),"membership":"intersection"},
                        "maximumMinutes":90
                    });
                }
                let document = serde_json::from_value(value)?;
                let compiled = compile_workforce(&document, &context())?;
                let mut expected = BTreeMap::from([
                    (Vec::new(), 0),
                    (vec![pair(1, 8)?], 1),
                    (vec![pair(1, 22)?], 2),
                ]);
                if contribution == "union" && !fractional {
                    expected.insert(vec![pair(1, 8)?, pair(1, 22)?], 3);
                }
                assert_eq!(feasible_models(&document, &compiled.problem)?, expected);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn consecutive_assignments_match_source_for_every_tiny_selection_and_helper_extension() -> Result {
    struct Case {
        name: &'static str,
        shifts: &'static [(u32, &'static str, &'static str)],
        break_minutes: u32,
        nonmatching: Option<u32>,
        // Independently authored longest matching runs, indexed by source-ID selection mask.
        longest_runs: &'static [u32],
    }
    let cases = [
        Case {
            name: "skipped candidate carries the prior selected end and run",
            shifts: &[
                (8, "08:00:00", "09:00:00"),
                (22, "09:00:00", "09:30:00"),
                (23, "09:30:00", "10:00:00"),
            ],
            break_minutes: 30,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 2, 1, 2, 2, 3],
        },
        Case {
            name: "exact break joins",
            shifts: &[(8, "08:00:00", "10:00:00"), (22, "10:30:00", "11:30:00")],
            break_minutes: 30,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 2],
        },
        Case {
            name: "one nanosecond beyond the break resets",
            shifts: &[
                (8, "08:00:00", "10:00:00"),
                (22, "10:30:00.000000001", "11:30:00"),
            ],
            break_minutes: 30,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 1],
        },
        Case {
            name: "negative overlap joins when every end qualifies",
            shifts: &[(8, "08:00:00", "12:00:00"), (22, "09:00:00", "10:00:00")],
            break_minutes: 0,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 2],
        },
        Case {
            name: "nested decreasing ends use the last selected end, not the maximum",
            shifts: &[
                (8, "08:00:00", "12:00:00"),
                (22, "09:00:00", "10:00:00"),
                (23, "11:00:00", "11:30:00"),
            ],
            break_minutes: 30,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 2, 1, 2, 1, 2],
        },
        Case {
            name: "tied starts use shift ID order, not end order",
            shifts: &[
                (23, "10:00:00", "11:00:00"),
                (22, "08:00:00", "09:00:00"),
                (8, "08:00:00", "12:00:00"),
            ],
            break_minutes: 0,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 2, 1, 2, 1, 2],
        },
        Case {
            name: "duplicate end instants qualify identically at the exact break",
            shifts: &[
                (8, "08:00:00", "10:00:00"),
                (22, "09:00:00", "10:00:00"),
                (23, "10:30:00", "11:30:00"),
            ],
            break_minutes: 30,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 2, 1, 2, 2, 3],
        },
        Case {
            name: "no preceding end qualifies",
            shifts: &[
                (8, "08:00:00", "09:00:00"),
                (22, "10:00:00", "11:00:00"),
                (23, "15:00:00", "16:00:00"),
            ],
            break_minutes: 30,
            nonmatching: None,
            longest_runs: &[0, 1, 1, 1, 1, 1, 1, 1],
        },
        Case {
            name: "scope-nonmatching selected shifts neither count nor replace the predecessor",
            shifts: &[
                (8, "08:00:00", "10:00:00"),
                (22, "10:05:00", "10:10:00"),
                (23, "10:30:00", "11:30:00"),
            ],
            break_minutes: 30,
            nonmatching: Some(22),
            longest_runs: &[0, 1, 0, 1, 1, 2, 1, 2],
        },
    ];
    for case in cases {
        for maximum in [0, 1, 2] {
            let mut value = quota_fixture()?;
            let template = value["domain"]["entities"][id(8)].clone();
            for &(shift_id, start, end) in case.shifts {
                let mut shift = template.clone();
                shift["id"] = json!(id(shift_id));
                shift["startsAt"] = endpoint(start);
                shift["endsAt"] = endpoint(end);
                value["domain"]["entities"][id(shift_id)] = shift;
            }
            if let Some(shift_id) = case.nonmatching {
                let mut other_type = value["domain"]["entities"][id(4)].clone();
                other_type["id"] = json!(id(40));
                other_type["category"] = json!("other");
                value["domain"]["entities"][id(40)] = other_type;
                value["domain"]["entities"][id(1)]["eligibleAssignmentTypeIds"] =
                    json!([id(4), id(40)]);
                value["domain"]["entities"][id(shift_id)]["assignmentTypeId"] = json!(id(40));
            }
            value["domain"]["rules"][id(20)] = json!({
                "id":id(20),"kind":"maximumConsecutive","active":true,"strength":"required",
                "scope":{"people":{"kind":"all"},"categories":["clinic"]},
                "mode":{"kind":"assignments","breakMinutes":case.break_minutes},"maximum":maximum
            });
            let document: ScenarioDocument = serde_json::from_value(value)?;
            let compiled = compile_workforce(&document, &context())?;
            let universe = original_universe(&document)?;
            assert!(
                universe.len() <= 3,
                "only exhaustively enumerate tiny selections"
            );
            assert_eq!(case.longest_runs.len(), 1 << universe.len());
            let mut expected = BTreeMap::new();
            for (mask, &longest) in case.longest_runs.iter().enumerate() {
                let selected = selection(&universe, mask);
                let satisfied = longest <= maximum;
                assert_eq!(
                    original_accepts(&document, &selected)?,
                    satisfied,
                    "{}: maximum {maximum}, selection {selected:?}",
                    case.name
                );
                if satisfied {
                    expected.insert(selected.clone(), rank(&universe, &selected)?);
                }
            }
            assert_eq!(
                feasible_models(&document, &compiled.problem)?,
                expected,
                "{}: maximum {maximum}",
                case.name
            );
        }
    }
    Ok(())
}
