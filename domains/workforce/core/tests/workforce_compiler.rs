#![forbid(unsafe_code)]

//! Exhaustive mathematics for the unregistered complete compiler, not accepted scores.

mod support;

use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::{AssignmentValue, OptimizationDirection};
use eutheto_planning_ir::{
    BoolVariableId, CandidateValues, ComparisonOp, Constraint, IntVariableId, LinearExpression,
    Literal, PlanningIrLimitsV1, PlanningProblem, ProjectionExpression, Variable,
    canonical_ir_hash, project_candidate,
};
use eutheto_types::{CancellationToken, ScenarioDocument};
use eutheto_workforce::{
    assignment_rules::{compile_workforce, evaluate_assignment_rules},
    model::AssignmentPair,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
};
use support::id;

type Result<T = (), E = Box<dyn Error>> = std::result::Result<T, E>;

fn context() -> CompileContext {
    CompileContext {
        scenario_revision: 17,
        semantic_metadata: BTreeMap::new(),
        cancellation: CancellationToken::new(),
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
    let pairs = projected_pairs(problem)?;
    if selected.iter().any(|pair| !pairs.contains_key(pair)) {
        return Ok(Vec::new());
    }
    let booleans = pairs
        .into_iter()
        .map(|(pair, variable)| (variable, selected.contains(&pair)))
        .collect();
    let integers: Vec<IntVariableId> = problem
        .variables
        .iter()
        .filter_map(|variable| match variable {
            Variable::Integer(variable) => Some(variable.id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        integers.len() <= 8,
        "only exhaustively enumerate tiny fixtures"
    );
    let mut values = CandidateValues {
        booleans,
        integers: BTreeMap::new(),
    };
    let mut accepted = Vec::new();
    for mask in 0..(1 << integers.len()) {
        values.integers = integers
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), i64::from(mask & (1 << index) != 0)))
            .collect();
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
        assert_eq!(
            observed.len(),
            usize::from(expected),
            "selection {selected:?}"
        );
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
        let values = extensions(&compiled.problem, &selected)?
            .pop()
            .ok_or("missing extension")?;
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
