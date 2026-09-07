mod support;

use eutheto_domain_api::{
    CommandDescriptor, ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand,
    DomainMutation, validate_contract_value,
};
use eutheto_types::{DomainCommandEnvelope, EntityId, PackId, ScenarioDocument};
use eutheto_workforce::{
    commands, generated_workforce_pack_contract::WORKFORCE_PACK_CONTRACT_JSON,
};
use serde_json::{Value, json};
use std::error::Error;
use support::{fixture, id};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn envelope(command: &str, payload: Value) -> DomainCommandEnvelope {
    DomainCommandEnvelope {
        command_type: command.to_owned(),
        payload,
    }
}

fn batch(commands: Vec<DomainCommandEnvelope>) -> TestResult<DomainBatchCommand> {
    Ok(DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: PackId::new("official.workforce")?,
        scenario_schema_version: 1,
        label: None,
        commands,
    })
}

fn entity(document: &ScenarioDocument, index: u32) -> TestResult<&Value> {
    document
        .domain
        .entities
        .get(&id(index).parse::<EntityId>()?)
        .ok_or_else(|| "missing fixture entity".into())
}

fn entity_mut(document: &mut ScenarioDocument, index: u32) -> TestResult<&mut Value> {
    document
        .domain
        .entities
        .get_mut(&id(index).parse::<EntityId>()?)
        .ok_or_else(|| "missing fixture entity".into())
}

fn checked(
    document: &ScenarioDocument,
    command: &str,
    payload: Value,
) -> TestResult<DomainMutation> {
    let contracts: Value = serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON)?;
    let descriptors: Vec<CommandDescriptor> =
        serde_json::from_value(contracts["commands"].clone())?;
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.id == command)
        .ok_or("missing descriptor")?;
    let mutation = commands::apply_batch(document, &batch(vec![envelope(command, payload)])?)?;
    let [result] = mutation.results.as_slice() else {
        return Err("expected one result".into());
    };
    validate_contract_value(
        &descriptor.result_schema,
        result,
        ContractJsonLimits::DEFAULT,
    )?;
    for change in &mutation.changes {
        assert_eq!(change.command_index, 0);
        validate_contract_value(
            &descriptor.change_schema,
            &change.value,
            ContractJsonLimits::DEFAULT,
        )?;
    }
    let [inverse] = mutation.inverse.commands.as_slice() else {
        return Err("expected one ordinary inverse".into());
    };
    let inverse_descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.id == inverse.command_type)
        .ok_or("missing inverse descriptor")?;
    validate_contract_value(
        &inverse_descriptor.payload_schema,
        &inverse.payload,
        ContractJsonLimits::DEFAULT,
    )?;
    Ok(mutation)
}

fn detach_payload(document: &ScenarioDocument) -> TestResult<Value> {
    let mut instance = entity(document, 8)?.clone();
    instance["id"] = json!(id(7).to_uppercase());
    instance["origin"] =
        json!({"kind":"detached","templateId":id(6),"occurrenceDate":"2026-11-01"});
    instance["tags"] = json!(["explicit-one-off"]);
    Ok(json!({"templateId":id(6),"instance":instance}))
}

fn targets(template: u32, shifts: &[u32]) -> Value {
    json!({"templates":[{"templateId":id(template),"shiftIds":shifts.iter().map(|shift| id(*shift)).collect::<Vec<_>>()}]})
}

fn additions(template: u32, shift: u32, date: &str) -> Value {
    json!({"templates":[{"templateId":id(template),"occurrenceIdentities":{(id(shift)):{"id":id(shift),"localStartDate":date}}}]})
}

#[test]
fn bulk_remove_inverse_preserves_independent_raw_uuid_spellings() -> TestResult {
    for (key, value_id) in [(id(7).to_uppercase(), id(7)), (id(7), id(7).to_uppercase())] {
        let mut original = fixture()?;
        original.domain.locked_assignments.clear();
        entity_mut(&mut original, 6)?["occurrenceIdentities"] =
            json!({(key):{"id":value_id,"localStartDate":"2026-11-01"}});
        let removed = checked(
            &original,
            commands::REMOVE_OCCURRENCE_IDENTITIES,
            targets(6, &[7]),
        )?;
        assert_eq!(
            removed.results,
            [json!({"occurrenceCount":1,"templateCount":1})]
        );
        let restored = checked(
            &removed.document,
            commands::ADD_OCCURRENCE_IDENTITIES,
            removed.inverse.commands[0].payload.clone(),
        )?;
        assert_eq!(restored.document, original);
        let redone = commands::apply_batch(&restored.document, &restored.inverse)?;
        assert_eq!(redone.document, removed.document);
    }
    Ok(())
}

#[test]
fn detach_inverse_redo_preserves_one_offs_raw_ledger_and_base_lock_references() -> TestResult {
    let mut original = fixture()?;
    entity_mut(&mut original, 6)?["occurrenceIdentities"] =
        json!({(id(7).to_uppercase()):{"id":id(7),"localStartDate":"2026-11-01"}});
    original.domain.entities.insert(id(30).parse()?, json!({"kind":"baseSchedule","id":id(30),"sourceSolutionId":id(31),"sourceRevision":1,"assignments":[{"personId":id(1),"shiftId":id(7)}]}));
    let payload = detach_payload(&original)?;
    let detached = checked(&original, commands::DETACH_SHIFT, payload.clone())?;
    assert_eq!(
        detached.results,
        [
            json!({"templateId":id(6),"shiftId":id(7),"occurrenceDate":"2026-11-01","owner":"stored"})
        ]
    );
    assert_eq!(detached.changes.len(), 2);
    assert_eq!(entity(&detached.document, 7)?, &payload["instance"]);
    assert_eq!(entity(&detached.document, 30)?, entity(&original, 30)?);
    assert_eq!(
        detached.document.domain.locked_assignments,
        original.domain.locked_assignments
    );
    let reattached = checked(
        &detached.document,
        commands::REATTACH_SHIFT,
        detached.inverse.commands[0].payload.clone(),
    )?;
    assert_eq!(
        reattached.results,
        [
            json!({"templateId":id(6),"shiftId":id(7),"occurrenceDate":"2026-11-01","owner":"template"})
        ]
    );
    assert_eq!(reattached.changes.len(), 2);
    assert_eq!(reattached.document, original);
    let redone = commands::apply_batch(&reattached.document, &reattached.inverse)?;
    assert_eq!(redone.document, detached.document);

    // Reattach must capture the current explicit one-off, not regenerate the template interval.
    let mut edited = detached.document;
    entity_mut(&mut edited, 7)?["startsAt"] = json!({"instant":"2026-11-01T06:30:00Z","local":"2026-11-01T01:30:00","offsetSeconds":-18000});
    entity_mut(&mut edited, 7)?["tags"] = json!(["edited-again"]);
    let reattached = commands::apply_batch(&edited, &detached.inverse)?;
    let restored = commands::apply_batch(&reattached.document, &reattached.inverse)?;
    assert_eq!(restored.document, edited);
    Ok(())
}

#[test]
fn dormant_and_unresolvable_template_ownership_transfers_remain_reversible() -> TestResult {
    let mut excluded = fixture()?;
    entity_mut(&mut excluded, 6)?["recurrence"]["excludedDates"] = json!(["2026-11-01"]);
    let mut inactive = fixture()?;
    entity_mut(&mut inactive, 6)?["recurrence"]["weekdays"] = json!(["monday"]);
    let mut out_of_range = fixture()?;
    entity_mut(&mut out_of_range, 6)?["recurrence"]["effectiveRange"] =
        json!({"startDate":"2027-01-01","endDateExclusive":"2027-02-01"});
    // Fixture's Sunday 01:30 is unresolved under overlapPolicy=reject, but explicit folds are valid.
    for original in [fixture()?, excluded, inactive, out_of_range] {
        let detached = checked(
            &original,
            commands::DETACH_SHIFT,
            detach_payload(&original)?,
        )?;
        let restored = commands::apply_batch(&detached.document, &detached.inverse)?;
        assert_eq!(restored.document, original);
    }
    Ok(())
}

#[test]
fn one_compound_command_reconciles_more_templates_than_the_batch_command_cap() -> TestResult {
    let mut original = fixture()?;
    let template = entity(&original, 6)?.clone();
    let mut templates = Vec::new();
    for index in 0..1001 {
        let template_id = id(2000 + index);
        let shift_id = id(10000 + index);
        let mut record = template.clone();
        record["id"] = json!(template_id);
        record["occurrenceIdentities"] = json!({});
        original
            .domain
            .entities
            .insert(template_id.parse()?, record);
        templates.push(json!({"templateId":template_id,"occurrenceIdentities":{(shift_id.clone()):{"id":shift_id,"localStartDate":"2026-11-01"}}}));
    }
    let added = checked(
        &original,
        commands::ADD_OCCURRENCE_IDENTITIES,
        json!({"templates":templates}),
    )?;
    assert_eq!(
        added.results,
        [json!({"occurrenceCount":1001,"templateCount":1001})]
    );
    assert_eq!(added.changes.len(), 1001);
    let removed = checked(
        &added.document,
        commands::REMOVE_OCCURRENCE_IDENTITIES,
        added.inverse.commands[0].payload.clone(),
    )?;
    assert_eq!(removed.document, original);
    let restored = commands::apply_batch(&removed.document, &removed.inverse)?;
    assert_eq!(restored.document, added.document);
    Ok(())
}

#[test]
fn malformed_duplicate_missing_and_wrong_kind_targets_are_atomic_errors() -> TestResult {
    let original = fixture()?;
    let mut duplicate_add = additions(6, 20, "2026-11-08");
    let repeated = duplicate_add["templates"][0].clone();
    duplicate_add["templates"]
        .as_array_mut()
        .ok_or("templates missing")?
        .push(repeated);
    let mut duplicate_key = additions(6, 20, "2026-11-08");
    duplicate_key["templates"][0]["occurrenceIdentities"][id(20).to_uppercase()] =
        json!({"id":id(20),"localStartDate":"2026-11-08"});
    let mut mismatch = additions(6, 20, "2026-11-08");
    mismatch["templates"][0]["occurrenceIdentities"][id(20)]["id"] = json!(id(21));
    let mut invalid_uuid = additions(6, 20, "2026-11-08");
    invalid_uuid["templates"][0]["templateId"] = json!("not-an-id");
    let duplicate_remove = json!({"templates":[{"templateId":id(6),"shiftIds":[id(7)]},{"templateId":id(6).to_uppercase(),"shiftIds":[id(7)]}]});
    let before = serde_json::to_vec(&original)?;
    for (command, payload) in [
        (commands::ADD_OCCURRENCE_IDENTITIES, json!({"templates":[]})),
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            json!({"templates":[{"templateId":id(6),"occurrenceIdentities":{}}]}),
        ),
        (commands::ADD_OCCURRENCE_IDENTITIES, duplicate_add),
        (commands::ADD_OCCURRENCE_IDENTITIES, duplicate_key),
        (commands::ADD_OCCURRENCE_IDENTITIES, mismatch),
        (commands::ADD_OCCURRENCE_IDENTITIES, invalid_uuid),
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            additions(1, 20, "2026-11-08"),
        ),
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            additions(99, 20, "2026-11-08"),
        ),
        (commands::REMOVE_OCCURRENCE_IDENTITIES, targets(6, &[])),
        (commands::REMOVE_OCCURRENCE_IDENTITIES, targets(6, &[7, 7])),
        (commands::REMOVE_OCCURRENCE_IDENTITIES, duplicate_remove),
        (commands::REMOVE_OCCURRENCE_IDENTITIES, targets(6, &[99])),
    ] {
        assert!(
            commands::apply_batch(&original, &batch(vec![envelope(command, payload)])?).is_err()
        );
        assert_eq!(serde_json::to_vec(&original)?, before);
    }
    Ok(())
}

#[test]
fn whole_state_rejects_identity_date_collisions_and_reference_invalid_removal() -> TestResult {
    let original = fixture()?;
    for (command, payload) in [
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            additions(6, 7, "2026-11-08"),
        ),
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            additions(6, 8, "2026-11-08"),
        ),
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            additions(6, 1, "2026-11-08"),
        ),
        (
            commands::ADD_OCCURRENCE_IDENTITIES,
            additions(6, 20, "2026-11-01"),
        ),
        (commands::REMOVE_OCCURRENCE_IDENTITIES, targets(6, &[7])),
    ] {
        assert!(
            commands::apply_batch(&original, &batch(vec![envelope(command, payload)])?).is_err()
        );
    }
    let detached = checked(
        &original,
        commands::DETACH_SHIFT,
        detach_payload(&original)?,
    )?;
    assert!(
        commands::apply_batch(
            &detached.document,
            &batch(vec![envelope(
                commands::ADD_OCCURRENCE_IDENTITIES,
                additions(6, 20, "2026-11-01")
            )])?
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn ownership_transfer_rejects_wrong_id_date_owner_and_non_singleton_reattach() -> TestResult {
    let original = fixture()?;
    let payload = detach_payload(&original)?;
    for (field, value) in [
        ("origin", json!({"kind":"manual"})),
        (
            "origin",
            json!({"kind":"detached","templateId":id(5),"occurrenceDate":"2026-11-01"}),
        ),
        (
            "origin",
            json!({"kind":"detached","templateId":id(6),"occurrenceDate":"2026-11-08"}),
        ),
        ("id", json!(id(20))),
    ] {
        let mut wrong = payload.clone();
        wrong["instance"][field] = value;
        assert!(
            commands::apply_batch(
                &original,
                &batch(vec![envelope(commands::DETACH_SHIFT, wrong)])?
            )
            .is_err()
        );
    }
    let manual_target = json!({"templateId":id(6),"occurrenceIdentities":{(id(8)):{"id":id(8),"localStartDate":"2026-11-01"}}});
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(commands::REATTACH_SHIFT, manual_target)])?
        )
        .is_err()
    );
    let detached = checked(&original, commands::DETACH_SHIFT, payload)?;
    let valid = detached.inverse.commands[0].payload.clone();
    let mut wrong_date = valid.clone();
    let key = wrong_date["occurrenceIdentities"]
        .as_object()
        .and_then(|map| map.keys().next())
        .ok_or("missing key")?
        .clone();
    wrong_date["occurrenceIdentities"][&key]["localStartDate"] = json!("2026-11-08");
    let mut multiple = valid.clone();
    multiple["occurrenceIdentities"][id(20)] = json!({"id":id(20),"localStartDate":"2026-11-08"});
    let mut mismatch = valid;
    mismatch["occurrenceIdentities"][&key]["id"] = json!(id(20));
    for invalid in [
        wrong_date,
        multiple,
        mismatch,
        json!({"templateId":id(6),"occurrenceIdentities":{}}),
    ] {
        assert!(
            commands::apply_batch(
                &detached.document,
                &batch(vec![envelope(commands::REATTACH_SHIFT, invalid)])?
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn late_operation_and_batch_failure_never_expose_partial_mutations() -> TestResult {
    let original = fixture()?;
    let mut compound = additions(6, 20, "2026-11-08");
    compound["templates"].as_array_mut().ok_or("missing templates")?.push(json!({"templateId":id(99),"occurrenceIdentities":{(id(21)):{"id":id(21),"localStartDate":"2026-11-08"}}}));
    let before = serde_json::to_vec(&original)?;
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(
                commands::ADD_OCCURRENCE_IDENTITIES,
                compound
            )])?
        )
        .is_err()
    );
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![
                envelope(commands::DETACH_SHIFT, detach_payload(&original)?),
                envelope(commands::REMOVE_OCCURRENCE_IDENTITIES, targets(6, &[7])),
            ])?
        )
        .is_err()
    );
    assert_eq!(serde_json::to_vec(&original)?, before);
    Ok(())
}

#[test]
fn large_ledgers_emit_only_addressed_entry_and_instance_deltas() -> TestResult {
    let mut original = fixture()?;
    let ledger = entity_mut(&mut original, 6)?["occurrenceIdentities"]
        .as_object_mut()
        .ok_or("missing ledger")?;
    let first: jiff::civil::Date = "2027-01-01".parse()?;
    for index in 0..4095 {
        // Civil day arithmetic keeps all dormant occurrence dates distinct.
        let date = first.checked_add(jiff::Span::new().days(i64::from(index)))?;
        ledger.insert(
            id(1000 + index),
            json!({"id":id(1000 + index),"localStartDate":date}),
        );
    }
    let detached = checked(
        &original,
        commands::DETACH_SHIFT,
        detach_payload(&original)?,
    )?;
    assert_eq!(detached.changes.len(), 2);
    assert!(serde_json::to_vec(&detached.changes)?.len() < 2048);
    assert!(serde_json::to_vec(&detached.inverse)?.len() < 512);
    assert_eq!(
        commands::apply_batch(&detached.document, &detached.inverse)?.document,
        original
    );
    let mut removable = original;
    removable.domain.locked_assignments.clear();
    let shift_ids: Vec<u32> = std::iter::once(7).chain(1000..5095).collect();
    let removed = checked(
        &removable,
        commands::REMOVE_OCCURRENCE_IDENTITIES,
        targets(6, &shift_ids),
    )?;
    assert_eq!(removed.changes.len(), 4096);
    assert_eq!(
        commands::apply_batch(&removed.document, &removed.inverse)?.document,
        removable
    );
    Ok(())
}

#[test]
fn compound_count_limits_reject_excess_templates_entries_and_total() -> TestResult {
    let mut original = fixture()?;
    let template = entity(&original, 6)?.clone();
    let mut templates = Vec::new();
    let first: jiff::civil::Date = "2027-01-01".parse()?;
    for template_index in 0..9 {
        let template_id = id(2000 + template_index);
        let mut record = template.clone();
        record["id"] = json!(template_id);
        record["occurrenceIdentities"] = json!({});
        original
            .domain
            .entities
            .insert(template_id.parse()?, record);
        let mut entries = serde_json::Map::new();
        let count = if template_index == 8 { 1 } else { 4096 };
        for index in 0..count {
            let shift_id = id(10000 + template_index * 4096 + index);
            let date = first.checked_add(jiff::Span::new().days(i64::from(index)))?;
            entries.insert(
                shift_id.clone(),
                json!({"id":shift_id,"localStartDate":date}),
            );
        }
        templates.push(json!({"templateId":template_id,"occurrenceIdentities":entries}));
    }
    // Every individual target fits, but the aggregate input contains 32,769 definitions.
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(
                commands::ADD_OCCURRENCE_IDENTITIES,
                json!({"templates":templates})
            )])?
        )
        .is_err()
    );
    let entries = (0..4097)
        .map(|index| {
            let shift_id = id(50000 + index);
            let date = first.checked_add(jiff::Span::new().days(i64::from(index)))?;
            Ok((
                shift_id.clone(),
                json!({"id":shift_id,"localStartDate":date}),
            ))
        })
        .collect::<TestResult<serde_json::Map<String, Value>>>()?;
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(
                commands::ADD_OCCURRENCE_IDENTITIES,
                json!({"templates":[{"templateId":id(2000),"occurrenceIdentities":entries}]})
            )])?
        )
        .is_err()
    );
    let too_many = (0..10001)
        .map(|index| json!({"templateId":id(60000 + index),"shiftIds":[id(7)]}))
        .collect::<Vec<_>>();
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(
                commands::REMOVE_OCCURRENCE_IDENTITIES,
                json!({"templates":too_many})
            )])?
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn cross_template_collisions_and_base_only_dangling_references_are_rejected() -> TestResult {
    let mut original = fixture()?;
    let mut other = entity(&original, 6)?.clone();
    other["id"] = json!(id(20));
    other["occurrenceIdentities"] = json!({});
    original.domain.entities.insert(id(20).parse()?, other);
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(
                commands::ADD_OCCURRENCE_IDENTITIES,
                additions(20, 7, "2026-11-08")
            )])?
        )
        .is_err()
    );
    original.domain.locked_assignments.clear();
    original.domain.entities.insert(id(30).parse()?, json!({"kind":"baseSchedule","id":id(30),"sourceSolutionId":id(31),"sourceRevision":1,"assignments":[{"personId":id(1),"shiftId":id(7)}]}));
    assert!(
        commands::apply_batch(
            &original,
            &batch(vec![envelope(
                commands::REMOVE_OCCURRENCE_IDENTITIES,
                targets(6, &[7])
            )])?
        )
        .is_err()
    );
    Ok(())
}
