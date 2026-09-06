mod support;

use eutheto_domain_api::{
    CommandDescriptor, ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand,
    DomainMutation, DomainPackError, validate_contract_value,
};
use eutheto_domain_ir::blake3_hex;
use eutheto_types::{DomainCommandEnvelope, PackId, ScenarioDocument};
use eutheto_workforce::{
    commands, generated_workforce_pack_contract::WORKFORCE_PACK_CONTRACT_JSON,
    validation::validate_document,
};
use serde_json::{Value, json};
use std::error::Error;
use support::{fixture, id};

fn envelope(command_type: &str, payload: Value) -> DomainCommandEnvelope {
    DomainCommandEnvelope {
        command_type: command_type.to_owned(),
        payload,
    }
}

fn batch(commands: Vec<DomainCommandEnvelope>) -> Result<DomainBatchCommand, Box<dyn Error>> {
    Ok(DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: PackId::new("official.workforce")?,
        scenario_schema_version: 1,
        label: Some("Reviewed Workforce changes".to_owned()),
        commands,
    })
}

fn hash(document: &ScenarioDocument) -> Result<String, Box<dyn Error>> {
    Ok(blake3_hex(&serde_json::to_vec(document)?))
}

fn entity(document: &mut ScenarioDocument, index: u32) -> Result<&mut Value, Box<dyn Error>> {
    document
        .domain
        .entities
        .get_mut(&id(index).parse()?)
        .ok_or_else(|| "missing fixture entity".into())
}

#[test]
fn inverse_restores_exact_record_spelling_and_leaves_unrelated_state_untouched()
-> Result<(), Box<dyn Error>> {
    let mut original = fixture()?;
    entity(&mut original, 1)?["id"] = json!(id(1).to_ascii_uppercase());
    entity(&mut original, 1)?["eligibleAssignmentTypeIds"] = json!([id(4).to_ascii_uppercase()]);
    entity(&mut original, 8)?["startsAt"]["instant"] = json!("2026-11-01T05:30:00+00:00");
    let mut typed = validate_document(&original)?;
    let eutheto_workforce::model::WorkforceEntity::Person(person) = typed
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("missing typed person")?
    else {
        return Err("wrong fixture entity kind".into());
    };
    person.name = "River revised".to_owned();
    let payload = commands::EntityPayload {
        entity: typed
            .entities
            .remove(&id(1).parse()?)
            .ok_or("missing typed person")?,
    };
    let before_hash = hash(&original)?;
    let mutation = commands::apply_batch(
        &original,
        &batch(vec![envelope(
            commands::UPDATE_ENTITY,
            serde_json::to_value(payload)?,
        )])?,
    )?;
    assert_eq!(
        mutation.document.domain.entities.get(&id(8).parse()?),
        original.domain.entities.get(&id(8).parse()?)
    );
    assert_eq!(mutation.document.metadata, original.metadata);
    assert_eq!(mutation.document.settings, original.settings);
    assert_eq!(mutation.document.extensions, original.extensions);
    assert_eq!(
        mutation
            .document
            .domain
            .entities
            .get(&id(1).parse()?)
            .ok_or("missing updated person")?["name"],
        "River revised"
    );
    assert_ne!(hash(&mutation.document)?, before_hash);
    let restored = commands::apply_batch(&mutation.document, &mutation.inverse)?;
    assert_eq!(restored.document, original);
    assert_eq!(hash(&restored.document)?, before_hash);
    Ok(())
}

struct Lifecycle {
    map: &'static str,
    record: &'static str,
    target: &'static str,
    add: &'static str,
    update: &'static str,
    remove: &'static str,
    initial: Value,
    updated: Value,
}

fn apply_contract_checked(
    document: &ScenarioDocument,
    command: &str,
    payload: Value,
    contracts: &[CommandDescriptor],
) -> Result<DomainMutation, Box<dyn Error>> {
    let descriptor = contracts
        .iter()
        .find(|item| item.id == command)
        .ok_or("missing command contract")?;
    validate_contract_value(
        &descriptor.payload_schema,
        &payload,
        ContractJsonLimits::DEFAULT,
    )?;
    let mutation = commands::apply_batch(document, &batch(vec![envelope(command, payload)])?)?;
    let [result] = mutation.results.as_slice() else {
        return Err("one command must produce exactly one result".into());
    };
    let [change] = mutation.changes.as_slice() else {
        return Err("one command must produce exactly one change".into());
    };
    validate_contract_value(
        &descriptor.result_schema,
        result,
        ContractJsonLimits::DEFAULT,
    )?;
    validate_contract_value(
        &descriptor.change_schema,
        &change.value,
        ContractJsonLimits::DEFAULT,
    )?;
    for inverse in &mutation.inverse.commands {
        let inverse_descriptor = contracts
            .iter()
            .find(|item| item.id == inverse.command_type)
            .ok_or("missing inverse contract")?;
        validate_contract_value(
            &inverse_descriptor.payload_schema,
            &inverse.payload,
            ContractJsonLimits::DEFAULT,
        )?;
    }
    Ok(mutation)
}

fn prove_lifecycle(
    original: &ScenarioDocument,
    case: &Lifecycle,
    contracts: &[CommandDescriptor],
) -> Result<(), Box<dyn Error>> {
    let identity = case.initial["id"]
        .as_str()
        .ok_or("fixture identity missing")?;
    let added = apply_contract_checked(
        original,
        case.add,
        json!({(case.record):case.initial}),
        contracts,
    )?;
    assert_eq!(added.results, [json!({(case.target):identity})]);
    assert_eq!(
        serde_json::to_value(&added.document.domain)?[case.map][identity],
        case.initial
    );
    let updated = apply_contract_checked(
        &added.document,
        case.update,
        json!({(case.record):case.updated}),
        contracts,
    )?;
    assert_eq!(updated.results, [json!({(case.target):identity})]);
    assert_eq!(
        serde_json::to_value(&updated.document.domain)?[case.map][identity],
        case.updated
    );
    let removed = apply_contract_checked(
        &updated.document,
        case.remove,
        json!({(case.target):identity}),
        contracts,
    )?;
    assert_eq!(removed.results, [json!({(case.target):identity})]);
    assert_eq!(removed.document, *original);
    let restore_removed = commands::apply_batch(&removed.document, &removed.inverse)?;
    assert_eq!(restore_removed.document, updated.document);
    let restore_updated = commands::apply_batch(&restore_removed.document, &updated.inverse)?;
    assert_eq!(restore_updated.document, added.document);
    let restore_added = commands::apply_batch(&restore_updated.document, &added.inverse)?;
    assert_eq!(hash(&restore_added.document)?, hash(original)?);
    Ok(())
}

#[test]
fn every_record_collection_supports_typed_reversible_lifecycle() -> Result<(), Box<dyn Error>> {
    let mut contract: Value = serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON)?;
    let contracts: Vec<CommandDescriptor> = serde_json::from_value(contract["commands"].take())?;
    let original = fixture()?;
    let qualification = json!({"kind":"qualification","id":id(20),"name":"Extra","description":""});
    let rule = json!({"kind":"eligibility","id":id(21),"active":true,"strength":"required","scope":{"people":{"kind":"all"}}});
    let preference = json!({"kind":"assignmentType","id":id(22),"active":true,"scope":{"people":{"kind":"all"}},
        "priority":"normal","weight":1,"direction":"prefer","assignmentTypeIds":[id(4)]});
    let lock = json!({"id":id(23),"personId":id(1),"shiftId":id(8),"state":{"kind":"hard"}});
    let mut qualification_updated = qualification.clone();
    qualification_updated["description"] = json!("Reviewed description");
    let mut rule_updated = rule.clone();
    rule_updated["active"] = json!(false);
    let mut preference_updated = preference.clone();
    preference_updated["weight"] = json!(2);
    let mut lock_updated = lock.clone();
    lock_updated["state"] = json!({"kind":"soft","stabilityWeight":2});
    for case in [
        Lifecycle {
            map: "entities",
            record: "entity",
            target: "entityId",
            add: commands::ADD_ENTITY,
            update: commands::UPDATE_ENTITY,
            remove: commands::REMOVE_ENTITY,
            initial: qualification,
            updated: qualification_updated,
        },
        Lifecycle {
            map: "rules",
            record: "rule",
            target: "ruleId",
            add: commands::ADD_RULE,
            update: commands::UPDATE_RULE,
            remove: commands::REMOVE_RULE,
            initial: rule,
            updated: rule_updated,
        },
        Lifecycle {
            map: "preferences",
            record: "preference",
            target: "preferenceId",
            add: commands::ADD_PREFERENCE,
            update: commands::UPDATE_PREFERENCE,
            remove: commands::REMOVE_PREFERENCE,
            initial: preference,
            updated: preference_updated,
        },
        Lifecycle {
            map: "lockedAssignments",
            record: "lock",
            target: "assignmentId",
            add: commands::ADD_LOCK,
            update: commands::UPDATE_LOCK,
            remove: commands::REMOVE_LOCK,
            initial: lock,
            updated: lock_updated,
        },
    ] {
        prove_lifecycle(&original, &case, &contracts)?;
    }
    Ok(())
}

#[test]
fn batch_reverses_repeated_edits_and_rejects_invalid_prefixes_atomically()
-> Result<(), Box<dyn Error>> {
    let mut original = fixture()?;
    let mut first = entity(&mut original, 1)?.clone();
    first["name"] = json!("First revision");
    let mut second = first.clone();
    second["name"] = json!("Second revision");
    let edits = batch(vec![
        envelope(commands::UPDATE_ENTITY, json!({"entity":first})),
        envelope(commands::UPDATE_ENTITY, json!({"entity":second})),
    ])?;
    let changed = commands::apply_batch(&original, &edits)?;
    assert_eq!(
        changed
            .document
            .domain
            .entities
            .get(&id(1).parse()?)
            .ok_or("missing person")?["name"],
        "Second revision"
    );
    assert_eq!(
        commands::apply_batch(&changed.document, &changed.inverse)?.document,
        original
    );

    let before_hash = hash(&original)?;
    let mut failing = edits;
    failing.commands.push(envelope(
        commands::REMOVE_ENTITY,
        json!({"entityId":id(11)}),
    ));
    assert!(commands::apply_batch(&original, &failing).is_err());
    assert_eq!(hash(&original)?, before_hash);
    let qualification = entity(&mut original, 11)?.clone();
    // Restoring the required qualification later cannot make an invalid earlier prefix legal.
    let invalid_prefix = batch(vec![
        envelope(commands::REMOVE_ENTITY, json!({"entityId":id(11)})),
        envelope(commands::ADD_ENTITY, json!({"entity":qualification})),
    ])?;
    assert!(commands::apply_batch(&original, &invalid_prefix).is_err());
    assert_eq!(hash(&original)?, before_hash);
    Ok(())
}

#[test]
fn update_cannot_retype_identity_or_smuggle_unknown_payload_fields() -> Result<(), Box<dyn Error>> {
    let original = fixture()?;
    let qualification = json!({"kind":"qualification","id":id(20),"name":"Extra","description":""});
    let added = commands::apply_batch(
        &original,
        &batch(vec![envelope(
            commands::ADD_ENTITY,
            json!({"entity":qualification}),
        )])?,
    )?;
    let retype = batch(vec![envelope(
        commands::UPDATE_ENTITY,
        json!({"entity":{"kind":"team","id":id(20),"name":"New meaning"}}),
    )])?;
    assert!(commands::apply_batch(&added.document, &retype).is_err());
    let unknown = batch(vec![envelope(
        commands::UPDATE_ENTITY,
        json!({"entity":qualification,"bypass":true}),
    )])?;
    assert!(commands::apply_batch(&added.document, &unknown).is_err());
    let unsupported = batch(vec![envelope("official.workforce.execute", json!({}))])?;
    assert!(matches!(
        commands::apply_batch(&original, &unsupported),
        Err(DomainPackError::UnknownCommand(_))
    ));
    Ok(())
}

#[test]
fn forward_batch_is_rejected_if_its_inverse_would_not_be_replayable() -> Result<(), Box<dyn Error>>
{
    let mut original = fixture()?;
    let tags: Vec<String> = (0..10_000).map(|index| format!("t{index:063}")).collect();
    let full_scope = json!({"people":{"kind":"filter","allTags":tags,"anyTags":tags}});
    let smaller_scope = json!({"people":{"kind":"filter","allTags":tags,"anyTags":[]}});
    let rule = json!({"kind":"minimumRest","id":id(20),"active":false,"strength":"required",
        "scope":full_scope,"afterScope":full_scope,"beforeScope":full_scope,"minimumMinutes":600});
    original.domain.rules.insert(id(20).parse()?, rule.clone());
    validate_document(&original)?;
    let mut smaller = rule;
    for scope in ["scope", "afterScope", "beforeScope"] {
        smaller[scope] = smaller_scope.clone();
    }
    let mut proposed = original.clone();
    proposed
        .domain
        .rules
        .insert(id(20).parse()?, smaller.clone());
    validate_document(&proposed)?;
    let forward = batch(
        (0..8)
            .map(|_| envelope(commands::UPDATE_RULE, json!({"rule":smaller})))
            .collect(),
    )?;
    forward.validate_bounds()?;
    let before_hash = hash(&original)?;
    assert!(matches!(
        commands::apply_batch(&original, &forward),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    assert_eq!(hash(&original)?, before_hash);
    Ok(())
}
