mod support;

use eutheto_domain_api::{CompileContext, DomainPackError};
use eutheto_domain_ir::{VerificationFactId, VerificationValue};
use eutheto_planning_ir::{Constraint, PlanningIrLimitsV1, ProvenanceSourceKind};
use eutheto_types::{CancellationToken, OverlapPolicy, RuleId, ScenarioDocument};
use eutheto_workforce::{
    assignment_rules::{
        AssignmentRuleCompilation, AssignmentRuleEvaluation, compile_assignment_rules,
        evaluate_assignment_rules,
    },
    model::AssignmentPair,
    validation::validate_document,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error};
use support::{fixture, id};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn scenario() -> Result<ScenarioDocument> {
    let mut document = fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    document.domain.locked_assignments.clear();
    Ok(document)
}

fn fixed(document: &mut ScenarioDocument, scope: &serde_json::Value) -> Result {
    document.domain.rules.insert(
        id(20).parse()?,
        json!({
            "kind":"fixedAssignment", "id":id(20), "active":true, "strength":"required",
            "scope":scope, "personId":id(1), "shiftId":id(8), "assigned":true
        }),
    );
    Ok(())
}

fn hard(document: &mut ScenarioDocument) -> Result {
    document.domain.locked_assignments.insert(
        id(50).parse()?,
        json!({"id":id(50),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}}),
    );
    Ok(())
}

fn compiled(document: &ScenarioDocument) -> Result<AssignmentRuleCompilation> {
    Ok(compile_assignment_rules(
        document,
        &CompileContext {
            objective_context: None,
            scenario_revision: 1,
            semantic_metadata: BTreeMap::new(),
            control: eutheto_types::OperationControl::Cancellation(CancellationToken::new()),
            planning_limits: PlanningIrLimitsV1::DEFAULT,
        },
    )?)
}

fn pair() -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id(1).parse()?,
        shift_id: id(8).parse()?,
    })
}

fn allows(model: &AssignmentRuleCompilation, selected: &[AssignmentPair]) -> Result<bool> {
    if selected
        .iter()
        .any(|pair| !model.variables.iter().any(|item| item.pair == *pair))
    {
        return Ok(false);
    }
    let values: BTreeMap<_, _> = model
        .variables
        .iter()
        .map(|item| (&item.variable.id, selected.contains(&item.pair)))
        .collect();
    for record in &model.constraints {
        let Constraint::BoolOr { literals } = &record.body else {
            return Err("unexpected forced-pair constraint primitive".into());
        };
        if !record.enforcement.is_empty()
            || !literals.iter().any(|literal| {
                values
                    .get(&literal.variable)
                    .is_some_and(|value| *value == literal.positive)
            })
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn violations(result: &AssignmentRuleEvaluation, index: u32) -> Result<i64> {
    let owner: RuleId = id(index).parse()?;
    let record = result
        .evaluations
        .iter()
        .find(|record| record.rule_id == owner)
        .ok_or("missing required obligation evaluation")?;
    record.validate()?;
    match record.observed.get(&VerificationFactId::new(
        "official.workforce.fact.violation_count",
    )?) {
        Some(VerificationValue::Integer(value)) => Ok(*value),
        _ => Err("missing typed violation count".into()),
    }
}

#[test]
fn named_fixed_rule_and_hard_lock_each_force_the_same_pair_without_a_base_schedule() -> Result {
    let mut document = scenario()?;
    fixed(&mut document, &json!({"people":{"kind":"all"}}))?;
    hard(&mut document)?;
    validate_document(&document)?;
    let model = compiled(&document)?;
    for owner in [20, 50] {
        assert!(model.obligations.handled.contains(&id(owner).parse()?));
    }
    assert!(model.obligations.remaining.is_empty());
    assert_eq!(model.constraints.len(), 2);
    let source_kinds = model
        .constraints
        .iter()
        .map(|constraint| {
            let child = model
                .provenance
                .iter()
                .find(|record| record.id == constraint.provenance)
                .ok_or("missing forced-pair provenance")?;
            let parent_id = child.parent.as_ref().ok_or("missing authored parent")?;
            let parent = model
                .provenance
                .iter()
                .find(|record| &record.id == parent_id)
                .ok_or("missing authored source")?;
            Ok((parent.source_id.clone(), parent.source_kind))
        })
        .collect::<Result<Vec<_>>>()?;
    assert!(source_kinds.contains(&(id(20), ProvenanceSourceKind::RequiredRule)));
    assert!(source_kinds.contains(&(id(50), ProvenanceSourceKind::Fact)));
    let selected = [pair()?];
    assert!(allows(&model, &selected)?);
    assert!(!allows(&model, &[])?);
    for (selection, expected) in [(selected.as_slice(), 0), (&[][..], 1)] {
        let result = evaluate_assignment_rules(&document, selection, None)?;
        assert_eq!(violations(&result, 20)?, expected);
        assert_eq!(violations(&result, 50)?, expected);
        assert!(result.obligations.remaining.is_empty());
    }
    Ok(())
}

#[test]
fn fixed_target_outside_its_active_scope_fails_native_admission() -> Result {
    let mut document = scenario()?;
    fixed(
        &mut document,
        &json!({"people":{"kind":"filter","allTags":["not-present"],"anyTags":[]}}),
    )?;
    let error = validate_document(&document)
        .err()
        .ok_or("mismatched fixed scope admitted")?;
    match error {
        DomainPackError::InvalidPayload { path, .. } => {
            assert!(
                path.starts_with(&format!("domain.rules.{}.", id(20))),
                "{path}"
            );
        }
        other => return Err(format!("unexpected fixed-scope error: {other:?}").into()),
    }
    document
        .domain
        .rules
        .get_mut(&id(20).parse()?)
        .ok_or("missing rule")?["active"] = json!(false);
    validate_document(&document)?;
    Ok(())
}

#[test]
fn pruned_hard_lock_and_fixed_target_are_explicitly_infeasible() -> Result {
    let mut document = scenario()?;
    fixed(&mut document, &json!({"people":{"kind":"all"}}))?;
    hard(&mut document)?;
    document.domain.rules.insert(
        id(21).parse()?,
        json!({"kind":"eligibility","id":id(21),"active":true,"strength":"required",
            "scope":{"people":{"kind":"all"}}}),
    );
    document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("missing person")?["qualificationGrants"] = json!([]);
    let model = compiled(&document)?;
    assert_eq!(model.constraints.len(), 2);
    assert!(model.constraints.iter().all(|record| matches!(
        &record.body,
        Constraint::BoolOr { literals } if literals.is_empty()
    )));
    assert!(!allows(&model, &[])?);
    let evaluation = evaluate_assignment_rules(&document, &[], None)?;
    assert_eq!(violations(&evaluation, 20)?, 1);
    assert_eq!(violations(&evaluation, 50)?, 1);
    Ok(())
}
