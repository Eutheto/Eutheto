mod support;

use eutheto_domain_api::CompileContext;
use eutheto_domain_ir::{VerificationFactId, VerificationValue};
use eutheto_planning_ir::{Constraint, PlanningIrLimitsV1};
use eutheto_types::{CancellationToken, OverlapPolicy, RuleId, ScenarioDocument};
use eutheto_workforce::assignment_rules::{
    AssignmentRuleCompilation, AssignmentRuleError, AssignmentRuleEvaluation, AssignmentRuleLimit,
    compile_assignment_rules, evaluate_assignment_rules,
};
use eutheto_workforce::model::AssignmentPair;
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error};
use support::{fixture, id};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn scenario() -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    document.domain.locked_assignments.clear();
    document.domain.entities.remove(&id(6).parse()?);
    document.domain.rules.insert(
        id(20).parse()?,
        json!({
            "kind":"requiredSkillMix", "id":id(20), "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "qualificationMinimums":[]
        }),
    );
    Ok(document)
}

fn minimum(qualifications: &[String], count: u16) -> Value {
    json!({
        "qualifications":{"allQualificationIds":qualifications,"anyQualificationIds":[]},
        "minimum":count
    })
}

fn set_minima(document: &mut ScenarioDocument, minima: Vec<Value>) -> Result {
    document
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("rule")?["qualificationMinimums"] = Value::Array(minima);
    Ok(())
}

fn person(index: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(index).parse()?,
        shift_id: id(shift).parse()?,
    })
}

fn compiled_with_limits(
    document: &ScenarioDocument,
    planning_limits: PlanningIrLimitsV1,
) -> std::result::Result<AssignmentRuleCompilation, AssignmentRuleError> {
    compile_assignment_rules(
        document,
        &CompileContext {
            scenario_revision: 1,
            semantic_metadata: BTreeMap::new(),
            control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
            planning_limits,
        },
    )
}

fn compiled(document: &ScenarioDocument) -> Result<AssignmentRuleCompilation> {
    Ok(compiled_with_limits(document, PlanningIrLimitsV1::DEFAULT)?)
}

fn amplified_document(shift_count: u32, minimum_count: u32) -> Result<ScenarioDocument> {
    let mut document = scenario()?;
    let qualification_ids: Vec<_> = (0..minimum_count).map(|index| id(1_000 + index)).collect();
    for qualification_id in &qualification_ids {
        document.domain.entities.insert(
            qualification_id.parse()?,
            json!({
                "kind":"qualification",
                "id":qualification_id,
                "name":format!("Qualification {qualification_id}"),
                "description":""
            }),
        );
    }
    let person = id(1).parse()?;
    let grants = document.domain.entities.get_mut(&person).ok_or("person")?["qualificationGrants"]
        .as_array_mut()
        .ok_or("qualification grants")?;
    for qualification_id in &qualification_ids {
        grants.push(json!({"qualificationId":qualification_id}));
    }
    let prototype = document
        .domain
        .entities
        .get(&id(8).parse()?)
        .ok_or("shift")?
        .clone();
    for index in 1..shift_count {
        let shift_id = id(2_000 + index);
        let mut shift = prototype.clone();
        shift["id"] = json!(shift_id.clone());
        document.domain.entities.insert(shift_id.parse()?, shift);
    }
    set_minima(
        &mut document,
        (0..minimum_count)
            .map(|index| minimum(&[id(1_000 + index)], 1))
            .collect(),
    )?;
    Ok(document)
}

// Check the emitted Boolean math, independently of the Workforce evaluator.
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
        let (literals, min, max) = match &record.body {
            Constraint::BoolOr { literals } => (literals, 1, u64::MAX),
            Constraint::AtMostOne { literals } => (literals, 0, 1),
            Constraint::CardinalityRange { literals, min, max } => (literals, *min, *max),
            _ => return Err("unexpected constraint body".into()),
        };
        let count = literals
            .iter()
            .filter(|literal| {
                values
                    .get(&literal.variable)
                    .is_some_and(|value| *value == literal.positive)
            })
            .count() as u64;
        if count < min || count > max {
            return Ok(false);
        }
    }
    Ok(true)
}

fn evaluated(
    document: &ScenarioDocument,
    selected: &[AssignmentPair],
) -> Result<AssignmentRuleEvaluation> {
    Ok(evaluate_assignment_rules(document, selected, None)?)
}

fn totals(result: &AssignmentRuleEvaluation) -> Result<(i64, i64)> {
    let rule_id: RuleId = id(20).parse()?;
    let record = result
        .evaluations
        .iter()
        .find(|entry| entry.rule_id == rule_id)
        .ok_or("missing rule evaluation")?;
    record.validate()?;
    let integer = |name| -> Result<i64> {
        match record.observed.get(&VerificationFactId::new(name)?) {
            Some(VerificationValue::Integer(value)) => Ok(*value),
            _ => Err("missing integer fact".into()),
        }
    };
    Ok((
        integer("official.workforce.fact.checked_predicate_count")?,
        integer("official.workforce.fact.violation_count")?,
    ))
}

#[test]
fn each_minimum_counts_distinct_people_but_one_person_can_meet_multiple_minima() -> Result {
    let mut document = scenario()?;
    document.domain.entities.insert(
        id(12).parse()?,
        json!({
            "kind":"qualification", "id":id(12), "name":"Specialist", "description":""
        }),
    );
    document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["qualificationGrants"] =
        json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    set_minima(
        &mut document,
        vec![minimum(&[id(11)], 1), minimum(&[id(12)], 1)],
    )?;
    let pair = person(1, 8)?;
    assert!(allows(&compiled(&document)?, std::slice::from_ref(&pair))?);
    assert_eq!(
        totals(&evaluated(&document, std::slice::from_ref(&pair))?)?,
        (2, 0)
    );
    set_minima(
        &mut document,
        vec![minimum(&[id(11)], 2), minimum(&[id(12)], 1)],
    )?;
    assert!(!allows(&compiled(&document)?, std::slice::from_ref(&pair))?);
    assert_eq!(
        totals(&evaluated(&document, std::slice::from_ref(&pair))?)?,
        (2, 1)
    );
    Ok(())
}

#[test]
fn zero_bound_and_no_scoped_candidates_keep_empty_population_truthful() -> Result {
    let mut document = scenario()?;
    document
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("rule")?["scope"]["people"] =
        json!({"kind":"filter","allTags":["not-present"],"anyTags":[]});
    set_minima(&mut document, vec![minimum(&[id(11)], 0)])?;
    assert!(allows(&compiled(&document)?, &[])?);
    assert_eq!(totals(&evaluated(&document, &[])?)?, (1, 0));
    set_minima(&mut document, vec![minimum(&[id(11)], 1)])?;
    let impossible = compiled(&document)?;
    assert!(!allows(&impossible, &[])?);
    assert_eq!(totals(&evaluated(&document, &[])?)?, (1, 1));
    document
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("rule")?["active"] = json!(false);
    let inactive = compiled(&document)?;
    assert!(allows(&inactive, &[])?);
    assert!(!inactive.obligations.handled.contains(&id(20).parse()?));
    Ok(())
}

#[test]
fn qualifications_must_cover_entire_elapsed_shift_and_scope_does_not_hide_selected_pairs() -> Result
{
    let mut document = scenario()?;
    set_minima(&mut document, vec![minimum(&[id(11)], 1)])?;
    let pair = person(1, 8)?;
    assert!(allows(&compiled(&document)?, std::slice::from_ref(&pair))?);
    document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["qualificationGrants"] =
        json!([{"qualificationId":id(11),"expiresAt":"2026-11-01T06:30:00Z"}]);
    assert!(!allows(&compiled(&document)?, std::slice::from_ref(&pair))?);
    assert_eq!(
        totals(&evaluated(&document, std::slice::from_ref(&pair))?)?,
        (1, 1)
    );
    document
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("rule")?["scope"]["weekdays"] = json!(["monday"]);
    assert!(allows(&compiled(&document)?, std::slice::from_ref(&pair))?);
    assert_eq!(
        totals(&evaluated(&document, std::slice::from_ref(&pair))?)?,
        (0, 0)
    );
    Ok(())
}

#[test]
fn duplicate_and_reordered_minima_canonicalize_without_removing_authored_rule() -> Result {
    let mut document = scenario()?;
    document.domain.entities.insert(
        id(12).parse()?,
        json!({
            "kind":"qualification", "id":id(12), "name":"Specialist", "description":""
        }),
    );
    document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["qualificationGrants"] =
        json!([{"qualificationId":id(11)},{"qualificationId":id(12)}]);
    let first = minimum(&[id(11), id(12)], 1);
    let second = minimum(&[id(12), id(11)], 1);
    set_minima(&mut document, vec![first, second])?;
    let model = compiled(&document)?;
    assert_eq!(model.constraints.len(), 1);
    assert_eq!(totals(&evaluated(&document, &[person(1, 8)?])?)?, (1, 0));
    set_minima(&mut document, vec![minimum(&[id(12), id(11)], 1)])?;
    assert_eq!(model.constraints, compiled(&document)?.constraints);
    Ok(())
}

#[test]
fn every_small_selection_agrees_with_independent_original_domain_evaluation() -> Result {
    let mut document = scenario()?;
    let mut second = document
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("person")?
        .clone();
    second["id"] = json!(id(30));
    second.as_object_mut().ok_or("person")?.remove("externalId");
    document.domain.entities.insert(id(30).parse()?, second);
    for required in [0, 1, 2] {
        set_minima(&mut document, vec![minimum(&[id(11)], required)])?;
        let model = compiled(&document)?;
        for selected in [
            vec![],
            vec![person(1, 8)?],
            vec![person(30, 8)?],
            vec![person(1, 8)?, person(30, 8)?],
        ] {
            let model_accepts = allows(&model, &selected)?;
            let (_, violations) = totals(&evaluated(&document, &selected)?)?;
            assert_eq!(
                model_accepts,
                violations == 0,
                "required={required} selected={selected:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn amplified_minima_and_shifts_hit_the_constraint_ceiling_before_materialization() -> Result {
    const SHIFTS: u32 = 64;
    const MINIMA: u32 = 64;
    let document = amplified_document(SHIFTS, MINIMA)?;
    let mut limits = PlanningIrLimitsV1::DEFAULT;
    limits.max_constraints = 1_000;
    assert!(matches!(
        compiled_with_limits(&document, limits),
        Err(AssignmentRuleError::LimitExceeded(
            AssignmentRuleLimit::Constraints
        ))
    ));
    Ok(())
}
