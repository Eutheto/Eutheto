mod support;

use eutheto_domain_api::{CommandDescriptor, ContractJsonLimits, validate_contract_value};
use eutheto_workforce::{
    commands, generated_workforce_pack_contract::WORKFORCE_PACK_CONTRACT_JSON,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::error::Error;

fn typed_roundtrip<T: DeserializeOwned + Serialize>(value: Value) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::to_value(serde_json::from_value::<T>(value)?)?)
}

fn payload_roundtrip(command: &str, value: Value) -> Result<Value, Box<dyn Error>> {
    match command {
        commands::ADD_ENTITY | commands::UPDATE_ENTITY => {
            typed_roundtrip::<commands::EntityPayload>(value)
        }
        commands::REMOVE_ENTITY => typed_roundtrip::<commands::EntityTarget>(value),
        commands::ADD_RULE | commands::UPDATE_RULE => {
            typed_roundtrip::<commands::RulePayload>(value)
        }
        commands::REMOVE_RULE => typed_roundtrip::<commands::RuleTarget>(value),
        commands::ADD_PREFERENCE | commands::UPDATE_PREFERENCE => {
            typed_roundtrip::<commands::PreferencePayload>(value)
        }
        commands::REMOVE_PREFERENCE => typed_roundtrip::<commands::PreferenceTarget>(value),
        commands::ADD_LOCK | commands::UPDATE_LOCK => {
            typed_roundtrip::<commands::LockPayload>(value)
        }
        commands::REMOVE_LOCK => typed_roundtrip::<commands::LockTarget>(value),
        _ => Err(format!("generated command has no typed decoder: {command}").into()),
    }
}

#[test]
fn generated_payload_examples_roundtrip_through_real_typed_codecs() -> Result<(), Box<dyn Error>> {
    let mut contract: Value = serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON)?;
    let descriptors: Vec<CommandDescriptor> = serde_json::from_value(contract["commands"].take())?;
    for descriptor in descriptors {
        for (index, value) in descriptor.valid_examples.into_iter().enumerate() {
            validate_contract_value(
                &descriptor.payload_schema,
                &value,
                ContractJsonLimits::DEFAULT,
            )?;
            let encoded = payload_roundtrip(&descriptor.id, value)
                .map_err(|error| format!("{} example {index}: {error}", descriptor.id))?;
            validate_contract_value(
                &descriptor.payload_schema,
                &encoded,
                ContractJsonLimits::DEFAULT,
            )
            .map_err(|error| format!("{} encoded example {index}: {error}", descriptor.id))?;
        }
        for value in descriptor.invalid_examples {
            assert!(
                validate_contract_value(
                    &descriptor.payload_schema,
                    &value,
                    ContractJsonLimits::DEFAULT
                )
                .is_err()
            );
        }
    }
    let document = support::fixture()?;
    let domain = serde_json::to_value(&document.domain)?;
    validate_contract_value(
        &contract["internalSchema"],
        &domain,
        ContractJsonLimits::DEFAULT,
    )?;
    let mut invalid = domain;
    invalid["entities"][support::id(1)]["unexpectedSemanticField"] = Value::Bool(true);
    assert!(
        validate_contract_value(
            &contract["internalSchema"],
            &invalid,
            ContractJsonLimits::DEFAULT
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn document_and_command_boundaries_reject_undeclared_json_representations()
-> Result<(), Box<dyn Error>> {
    use eutheto_domain_api::{DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand};
    use eutheto_types::{DomainCommandEnvelope, PackId};
    use eutheto_workforce::validation::{decode_document, validate_document};
    use serde_json::json;

    let original = support::fixture()?;
    let person_id = support::id(1).parse()?;
    let rule_id = support::id(30).parse()?;
    let lock_id = support::id(14).parse()?;
    let mut accepted = Vec::new();
    for representation in [
        "enumObject",
        "structArray",
        "optionalNull",
        "unitExtraField",
    ] {
        let mut document = original.clone();
        let (command_type, payload) = match representation {
            "enumObject" => {
                let rule = json!({"kind":"eligibility","id":rule_id,"active":true,"strength":{"required":null},"scope":{"people":{"kind":"all"}}});
                document.domain.rules.insert(rule_id, rule.clone());
                (commands::ADD_RULE, json!({"rule":rule}))
            }
            "structArray" | "optionalNull" => {
                let person = document
                    .domain
                    .entities
                    .get_mut(&person_id)
                    .ok_or("missing person")?;
                if representation == "structArray" {
                    person["workloadWeight"] = json!([1, 1]);
                } else {
                    person["externalId"] = Value::Null;
                }
                (commands::UPDATE_ENTITY, json!({"entity":person}))
            }
            _ => {
                let lock = document
                    .domain
                    .locked_assignments
                    .get_mut(&lock_id)
                    .ok_or("missing lock")?;
                lock["state"] = json!({"kind":"hard","stabilityWeight":3});
                (commands::UPDATE_LOCK, json!({"lock":lock}))
            }
        };
        if validate_document(&document).is_ok() {
            accepted.push(format!("{representation}: existing document"));
        }
        if decode_document(&serde_json::to_vec(&document)?).is_ok() {
            accepted.push(format!("{representation}: byte ingress"));
        }
        let batch = DomainBatchCommand {
            schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
            pack_id: PackId::new("official.workforce")?,
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: command_type.to_owned(),
                payload,
            }],
        };
        if commands::apply_batch(&original, &batch).is_ok() {
            accepted.push(format!("{representation}: command"));
        }
    }
    assert!(
        accepted.is_empty(),
        "undeclared representations accepted: {accepted:?}"
    );
    Ok(())
}
