use super::{fixture_value, id};
use eutheto_domain_api::{DiagnosticCopyContext, DomainPack, DomainPackError};
use eutheto_domain_ir::{
    AssignmentValue, AssumptionGroupId, ConflictGroupV1, CounterfactualConditionPayloadV1,
    CounterfactualConditionV1, DomainAssignmentId,
};
use eutheto_types::{
    CancellationToken, FixedIdGenerator, OperationControl, RuleId, ScenarioDocument,
};
use eutheto_workforce::{WorkforcePack, commands};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const ASSIGNMENT_KIND: &str = "official.workforce.assignment";
const SELECTED_RULE: u32 = 34;

fn rule_id(index: u32) -> Result<RuleId, Box<dyn Error>> {
    Ok(id(index).parse()?)
}

fn source_document(shift: u32) -> Result<ScenarioDocument, Box<dyn Error>> {
    let mut value = if shift == 7 {
        serde_json::to_value(super::workforce_fixture::fixture()?)?
    } else {
        fixture_value()?
    };
    value["domain"]["rules"][id(SELECTED_RULE)] = json!({
        "kind":"fixedAssignment",
        "id":id(SELECTED_RULE),
        "active":true,
        "strength":"required",
        "scope":{"people":{"kind":"all"}},
        "personId":id(1),
        "shiftId":id(shift)
    });
    let mut document: ScenarioDocument = serde_json::from_value(value)?;
    if shift == 7 {
        document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    }
    Ok(document)
}

fn remapped_destination(
    source: &ScenarioDocument,
) -> Result<(ScenarioDocument, BTreeMap<Uuid, Uuid>), Box<dyn Error>> {
    let mut remap = BTreeMap::new();
    for index in 1..=1_000 {
        remap.insert(id(index).parse()?, id(index + 1_000).parse()?);
    }
    let mut value = serde_json::to_value(source)?;
    remap_json(&mut value, &remap);
    Ok((serde_json::from_value(value)?, remap))
}

fn remap_json(value: &mut Value, remap: &BTreeMap<Uuid, Uuid>) {
    match value {
        Value::Array(values) => {
            for value in values {
                remap_json(value, remap);
            }
        }
        Value::Object(object) => {
            let entries = std::mem::take(object);
            for (key, mut value) in entries {
                remap_json(&mut value, remap);
                let key = key
                    .parse::<Uuid>()
                    .ok()
                    .and_then(|uuid| remap.get(&uuid).copied())
                    .map_or(key, |uuid| uuid.to_string());
                object.insert(key, value);
            }
        }
        Value::String(text) => {
            if let Ok(uuid) = text.parse::<Uuid>()
                && let Some(mapped) = remap.get(&uuid)
            {
                *text = mapped.to_string();
            }
        }
        Value::Bool(_) | Value::Number(_) | Value::Null => {}
    }
}

fn group(rule: u32) -> Result<ConflictGroupV1, Box<dyn Error>> {
    Ok(ConflictGroupV1 {
        group_id: AssumptionGroupId::new(format!("tests.diagnostic.group.{rule}"))?,
        required_rules: vec![rule_id(rule)?],
    })
}

fn assignment_id(person: u32, shift: u32) -> Result<DomainAssignmentId, Box<dyn Error>> {
    Ok(format!("{ASSIGNMENT_KIND}.{}.{}", id(person), id(shift)).parse()?)
}

fn boolean_condition(
    forbid: bool,
    person: u32,
    shift: u32,
    value: bool,
) -> Result<CounterfactualConditionV1, Box<dyn Error>> {
    let assignment_id = assignment_id(person, shift)?;
    let payload = if forbid {
        CounterfactualConditionPayloadV1::ForbidAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(value),
        }
    } else {
        CounterfactualConditionPayloadV1::ForceAssignmentValue {
            assignment_id,
            value: AssignmentValue::Boolean(value),
        }
    };
    Ok(CounterfactualConditionV1::new(payload)?)
}

fn replacement_condition(
    target_person: u32,
    target_shift: u32,
    replaced_person: u32,
    replaced_shift: u32,
) -> Result<CounterfactualConditionV1, Box<dyn Error>> {
    Ok(CounterfactualConditionV1::new(
        CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: assignment_id(target_person, target_shift)?,
            replaced_assignment_id: assignment_id(replaced_person, replaced_shift)?,
        },
    )?)
}

fn prepare(
    source: &ScenarioDocument,
    destination: &ScenarioDocument,
    remap: &BTreeMap<Uuid, Uuid>,
    selected_rule: u32,
    condition: Option<&CounterfactualConditionV1>,
    ids: &FixedIdGenerator,
) -> Result<eutheto_domain_api::DomainBatchCommand, DomainPackError> {
    let conflict = group(selected_rule).map_err(|error| DomainPackError::InvalidPayload {
        path: "/group".to_owned(),
        message: error.to_string(),
    })?;
    let control = OperationControl::Cancellation(CancellationToken::new());
    WorkforcePack.prepare_diagnostic_copy_changes(
        source,
        &DiagnosticCopyContext {
            source_revision: 1,
            destination,
            id_remap: remap,
            group: &conflict,
            condition,
            ids,
            control: &control,
        },
    )
}

fn invalid_path(
    result: Result<eutheto_domain_api::DomainBatchCommand, DomainPackError>,
    expected_path: &str,
) -> TestResult {
    match result {
        Err(DomainPackError::InvalidPayload { path, .. }) => {
            if path == expected_path {
                Ok(())
            } else {
                Err(format!("expected InvalidPayload at {expected_path}, got path {path}").into())
            }
        }
        Err(error) => {
            Err(format!("expected InvalidPayload at {expected_path}, got {error:?}").into())
        }
        Ok(_) => Err(format!("expected InvalidPayload at {expected_path}").into()),
    }
}

fn expected_selected_rule(destination: &ScenarioDocument, selected_destination: RuleId) -> Value {
    let mut expected = destination.domain.rules[&selected_destination].clone();
    expected["active"] = Value::Bool(false);
    expected
}

fn apply_and_undo(
    destination: &ScenarioDocument,
    batch: &eutheto_domain_api::DomainBatchCommand,
    selected_destination: RuleId,
    generated_rule: Option<Value>,
) -> Result<eutheto_domain_api::DomainMutation, Box<dyn Error>> {
    let changed = WorkforcePack.apply_batch(destination, batch, &CancellationToken::new())?;
    let mut expected = destination.clone();
    expected.domain.rules.insert(
        selected_destination,
        expected_selected_rule(destination, selected_destination),
    );
    if let Some(generated) = generated_rule {
        let generated_id: RuleId = generated["id"]
            .as_str()
            .ok_or("generated rule id")?
            .parse()?;
        expected.domain.rules.insert(generated_id, generated);
    }
    assert_eq!(changed.document, expected);
    let undone = WorkforcePack.apply_batch(
        &changed.document,
        &changed.inverse,
        &CancellationToken::new(),
    )?;
    assert_eq!(undone.document, *destination);
    Ok(changed)
}

fn assert_added_assignment(
    batch: &eutheto_domain_api::DomainBatchCommand,
    expected_person: Uuid,
    expected_shift: Uuid,
    expected_assigned: bool,
) -> Result<Value, Box<dyn Error>> {
    assert_eq!(batch.commands.len(), 2);
    assert_eq!(batch.commands[0].command_type, commands::UPDATE_RULE);
    assert_eq!(batch.commands[1].command_type, commands::ADD_RULE);
    let rule = batch.commands[1]
        .payload
        .get("rule")
        .ok_or("missing generated rule")?
        .clone();
    assert_eq!(rule["personId"], json!(expected_person.to_string()));
    assert_eq!(rule["shiftId"], json!(expected_shift.to_string()));
    assert_eq!(rule["active"], Value::Bool(true));
    assert_eq!(rule["strength"], json!("required"));
    assert_eq!(rule["scope"]["people"]["kind"], json!("selected"));
    assert_eq!(
        rule["scope"]["people"]["personIds"],
        json!([expected_person.to_string()])
    );
    if expected_assigned {
        assert!(
            rule.get("assigned").is_none(),
            "true pin must use native omitted default"
        );
    } else {
        assert_eq!(rule["assigned"], Value::Bool(false));
    }
    Ok(rule)
}

#[test]
fn native_copy_relaxes_only_selected_raw_rule_and_inverse_restores_it() -> TestResult {
    let source = source_document(8)?;
    let (destination, remap) = remapped_destination(&source)?;
    let selected_destination = rule_id(SELECTED_RULE + 1_000)?;
    let before_source = source.clone();
    let before_destination = destination.clone();
    let ids = FixedIdGenerator::new([]);
    let batch = prepare(&source, &destination, &remap, SELECTED_RULE, None, &ids)?;
    assert_eq!(batch.commands.len(), 1);
    assert_eq!(batch.commands[0].command_type, commands::UPDATE_RULE);
    assert_eq!(
        batch.commands[0].payload["rule"],
        expected_selected_rule(&destination, selected_destination)
    );
    let changed = WorkforcePack.apply_batch(&destination, &batch, &CancellationToken::new())?;
    let mut expected = destination.clone();
    expected.domain.rules.insert(
        selected_destination,
        expected_selected_rule(&destination, selected_destination),
    );
    assert_eq!(changed.document, expected);
    assert_eq!(source, before_source);
    assert_eq!(destination, before_destination);
    let undone = WorkforcePack.apply_batch(
        &changed.document,
        &changed.inverse,
        &CancellationToken::new(),
    )?;
    assert_eq!(undone.document, destination);
    Ok(())
}

#[test]
fn boolean_force_and_forbid_pins_remap_native_pair_and_preserve_inverse() -> TestResult {
    let source = source_document(8)?;
    let (destination, remap) = remapped_destination(&source)?;
    let selected_destination = rule_id(SELECTED_RULE + 1_000)?;
    let mapped_person = *remap.get(&id(1).parse()?).ok_or("mapped person")?;
    let mapped_shift = *remap.get(&id(8).parse()?).ok_or("mapped shift")?;
    for (case, forbid, value) in [
        (0_u32, false, false),
        (1, false, true),
        (2, true, false),
        (3, true, true),
    ] {
        let condition = boolean_condition(forbid, 1, 8, value)?;
        let ids = FixedIdGenerator::single(id(1_800 + case).parse()?);
        let batch = prepare(
            &source,
            &destination,
            &remap,
            SELECTED_RULE,
            Some(&condition),
            &ids,
        )?;
        let expected_assigned = if forbid { !value } else { value };
        let generated =
            assert_added_assignment(&batch, mapped_person, mapped_shift, expected_assigned)?;
        apply_and_undo(&destination, &batch, selected_destination, Some(generated))?;
    }
    assert_eq!(source, source_document(8)?);
    Ok(())
}

#[test]
fn replacement_remaps_distinct_people_on_one_shift_and_rejects_other_shapes() -> TestResult {
    let source = source_document(8)?;
    let (destination, remap) = remapped_destination(&source)?;
    let selected_destination = rule_id(SELECTED_RULE + 1_000)?;
    let mapped_target_person = *remap.get(&id(20).parse()?).ok_or("mapped target person")?;
    let mapped_replaced_person = *remap.get(&id(1).parse()?).ok_or("mapped replaced person")?;
    let mapped_shift = *remap.get(&id(8).parse()?).ok_or("mapped shift")?;
    let condition = replacement_condition(20, 8, 1, 8)?;
    let ids = FixedIdGenerator::new([id(1_900).parse()?, id(1_901).parse()?]);
    let batch = prepare(
        &source,
        &destination,
        &remap,
        SELECTED_RULE,
        Some(&condition),
        &ids,
    )?;
    assert_eq!(batch.commands.len(), 3);
    assert_eq!(batch.commands[0].command_type, commands::UPDATE_RULE);
    assert_eq!(batch.commands[1].command_type, commands::ADD_RULE);
    assert_eq!(batch.commands[2].command_type, commands::ADD_RULE);
    let target = &batch.commands[1].payload["rule"];
    assert_eq!(target["personId"], json!(mapped_target_person.to_string()));
    assert_eq!(target["shiftId"], json!(mapped_shift.to_string()));
    assert!(target.get("assigned").is_none());
    let replaced = &batch.commands[2].payload["rule"];
    assert_eq!(
        replaced["personId"],
        json!(mapped_replaced_person.to_string())
    );
    assert_eq!(replaced["shiftId"], json!(mapped_shift.to_string()));
    assert_eq!(replaced["assigned"], Value::Bool(false));
    let mut expected = destination.clone();
    expected.domain.rules.insert(
        selected_destination,
        expected_selected_rule(&destination, selected_destination),
    );
    let target_id: RuleId = target["id"].as_str().ok_or("target id")?.parse()?;
    let replaced_id: RuleId = replaced["id"].as_str().ok_or("replaced id")?.parse()?;
    expected.domain.rules.insert(target_id, target.clone());
    expected.domain.rules.insert(replaced_id, replaced.clone());
    let changed = WorkforcePack.apply_batch(&destination, &batch, &CancellationToken::new())?;
    assert_eq!(changed.document, expected);
    let undone = WorkforcePack.apply_batch(
        &changed.document,
        &changed.inverse,
        &CancellationToken::new(),
    )?;
    assert_eq!(undone.document, destination);
    assert_eq!(source, source_document(8)?);

    let wrong_shift = replacement_condition(20, 22, 1, 8)?;
    let ids = FixedIdGenerator::new([id(1_902).parse()?, id(1_903).parse()?]);
    invalid_path(
        prepare(
            &source,
            &destination,
            &remap,
            SELECTED_RULE,
            Some(&wrong_shift),
            &ids,
        ),
        "/condition",
    )?;
    let same_person = replacement_condition(1, 22, 1, 8)?;
    let ids = FixedIdGenerator::new([id(1_904).parse()?, id(1_905).parse()?]);
    invalid_path(
        prepare(
            &source,
            &destination,
            &remap,
            SELECTED_RULE,
            Some(&same_person),
            &ids,
        ),
        "/condition",
    )?;
    assert!(
        CounterfactualConditionV1::new(CounterfactualConditionPayloadV1::ReplaceAssignment {
            target_assignment_id: assignment_id(1, 8)?,
            replaced_assignment_id: assignment_id(1, 8)?,
        },)
        .is_err()
    );
    Ok(())
}

#[test]
fn generated_occurrence_targets_remap_and_missing_targets_fail_closed() -> TestResult {
    let source = source_document(7)?;
    let (destination, remap) = remapped_destination(&source)?;
    let selected_destination = rule_id(SELECTED_RULE + 1_000)?;
    let mapped_person = *remap.get(&id(1).parse()?).ok_or("mapped person")?;
    let mapped_shift = *remap.get(&id(7).parse()?).ok_or("mapped occurrence")?;
    let condition = boolean_condition(false, 1, 7, false)?;
    let ids = FixedIdGenerator::single(id(1_920).parse()?);
    let batch = prepare(
        &source,
        &destination,
        &remap,
        SELECTED_RULE,
        Some(&condition),
        &ids,
    )
    .map_err(|error| format!("generated prepare: {error:?}"))?;
    let generated = assert_added_assignment(&batch, mapped_person, mapped_shift, false)?;
    let changed = WorkforcePack
        .apply_batch(&destination, &batch, &CancellationToken::new())
        .map_err(|error| format!("generated apply: {error:?}"))?;
    let mut expected = destination.clone();
    expected.domain.rules.insert(
        selected_destination,
        expected_selected_rule(&destination, selected_destination),
    );
    let generated_id: RuleId = generated["id"]
        .as_str()
        .ok_or("generated rule id")?
        .parse()?;
    expected.domain.rules.insert(generated_id, generated);
    assert_eq!(changed.document, expected);
    let undone = WorkforcePack
        .apply_batch(
            &changed.document,
            &changed.inverse,
            &CancellationToken::new(),
        )
        .map_err(|error| format!("generated inverse: {error:?}"))?;
    assert_eq!(undone.document, destination);
    assert_eq!(source, source_document(7)?);

    let mut missing_destination = remap.clone();
    missing_destination.insert(id(7).parse()?, id(9_999).parse()?);
    let ids = FixedIdGenerator::single(id(1_921).parse()?);
    invalid_path(
        prepare(
            &source,
            &destination,
            &missing_destination,
            SELECTED_RULE,
            Some(&condition),
            &ids,
        ),
        "/condition",
    )?;
    let missing_source = boolean_condition(false, 99, 7, false)?;
    let ids = FixedIdGenerator::single(id(1_922).parse()?);
    invalid_path(
        prepare(
            &source,
            &destination,
            &remap,
            SELECTED_RULE,
            Some(&missing_source),
            &ids,
        ),
        "/condition",
    )?;
    Ok(())
}

#[test]
fn diagnostic_copy_rejects_wrong_pack_stale_groups_and_non_authored_groups() -> TestResult {
    let source = source_document(8)?;
    let (destination, remap) = remapped_destination(&source)?;
    let no_ids = FixedIdGenerator::new([]);

    let mut wrong_pack = destination.clone();
    wrong_pack.domain_pack.id = "other.workforce".parse()?;
    invalid_path(
        prepare(&source, &wrong_pack, &remap, SELECTED_RULE, None, &no_ids),
        "/source",
    )?;

    let mut stale_destination = destination.clone();
    let stale_rule = rule_id(SELECTED_RULE + 1_000)?;
    stale_destination
        .domain
        .rules
        .get_mut(&stale_rule)
        .ok_or("stale rule")?["active"] = Value::Bool(false);
    invalid_path(
        prepare(
            &source,
            &stale_destination,
            &remap,
            SELECTED_RULE,
            None,
            &no_ids,
        ),
        "/group",
    )?;

    invalid_path(
        prepare(&source, &destination, &remap, 99, None, &no_ids),
        "/group",
    )?;

    let mut value = serde_json::to_value(&source)?;
    value["domain"]["lockedAssignments"][id(40)] = json!({
        "id":id(40),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}
    });
    value["domain"]["preferences"][id(41)] = json!({
        "kind":"assignmentType","id":id(41),"active":true,
        "scope":{"people":{"kind":"all"}},"priority":"normal","weight":1,
        "direction":"prefer","assignmentTypeIds":[id(4)]
    });
    let source_with_non_authored = serde_json::from_value(value)?;
    let (destination_with_non_authored, remap_with_non_authored) =
        remapped_destination(&source_with_non_authored)?;
    for non_authored in [1, 40, 41] {
        invalid_path(
            prepare(
                &source_with_non_authored,
                &destination_with_non_authored,
                &remap_with_non_authored,
                non_authored,
                None,
                &no_ids,
            ),
            "/group",
        )?;
    }

    let mut inactive_value = serde_json::to_value(&source)?;
    inactive_value["domain"]["rules"][id(SELECTED_RULE)]["active"] = Value::Bool(false);
    let inactive_source: ScenarioDocument = serde_json::from_value(inactive_value)?;
    let (inactive_destination, inactive_remap) = remapped_destination(&inactive_source)?;
    invalid_path(
        prepare(
            &inactive_source,
            &inactive_destination,
            &inactive_remap,
            SELECTED_RULE,
            None,
            &no_ids,
        ),
        "/group",
    )?;
    Ok(())
}

#[test]
fn diagnostic_copy_rejects_missing_remapped_manual_person() -> TestResult {
    let source = source_document(8)?;
    let (destination, mut remap) = remapped_destination(&source)?;
    remap.insert(id(1).parse()?, id(9_998).parse()?);
    let condition = boolean_condition(false, 1, 8, false)?;
    let ids = FixedIdGenerator::single(id(1_930).parse()?);
    invalid_path(
        prepare(
            &source,
            &destination,
            &remap,
            SELECTED_RULE,
            Some(&condition),
            &ids,
        ),
        "/condition",
    )?;
    Ok(())
}
