use eutheto_command::{
    CODE_DUPLICATE_ENTITY, CODE_MISSING_ENTITY, CODE_PROHIBITED_DATA, CODE_RECORD_ID_MISMATCH,
    CommandError, OFFICIAL_TEST_PACK_ID, apply_command, apply_command_with_registry, command_type,
    human_summary, official_registry,
};
use eutheto_types::{
    ActorRef, AddEntity, AddRule, AssignmentId, CommandBatch, CommandEnvelope, CommandId,
    CommandSource, DomainCommandEnvelope, EntityId, LockAssignment, Revision, RuleId,
    ScenarioCommand, ScenarioDocument, SetPreference, UnlockAssignment, UpdateEntity, UpdateRule,
};
use serde_json::{Value, json};
use std::error::Error;
use std::str::FromStr;

const SCENARIO_ID: &str = "0195a5e4-7c00-7000-8000-000000000001";
const COMMAND_ID: &str = "0195a5e4-7c00-7000-8000-000000000002";
const ENTITY_ID: &str = "0195a5e4-7c00-7000-8000-000000000003";
const RULE_ID: &str = "0195a5e4-7c00-7000-8000-000000000004";
const PREFERENCE_ID: &str = "0195a5e4-7c00-7000-8000-000000000005";
const ASSIGNMENT_ID: &str = "0195a5e4-7c00-7000-8000-000000000006";
const UNSUPPORTED_PACK_ID: &str = "vendor.future";

fn document(pack_id: &str) -> Result<ScenarioDocument, serde_json::Error> {
    serde_json::from_value(json!({
        "format": "eutheto/scenario",
        "formatVersion": 1,
        "scenarioId": SCENARIO_ID,
        "domainPack": { "id": pack_id, "schemaVersion": 1 },
        "metadata": {
            "title": "Command tests",
            "description": "",
            "createdAt": "2026-08-29T12:00:00Z",
            "updatedAt": "2026-08-29T12:00:00Z"
        },
        "settings": {
            "timeZone": "UTC",
            "locale": "en-US",
            "units": "metric",
            "horizon": {
                "start": "2026-08-29T12:00:00Z",
                "end": "2026-09-05T12:00:00Z"
            },
            "gapPolicy": "reject",
            "overlapPolicy": "earlier"
        },
        "domain": {
            "entities": {},
            "rules": {},
            "preferences": {},
            "lockedAssignments": {}
        },
        "extensions": { "vendor.example": { "preserved": true } }
    }))
}

fn envelope(
    scenario_id: eutheto_types::ScenarioId,
    revision: Revision,
    command: ScenarioCommand,
) -> Result<CommandEnvelope, Box<dyn Error>> {
    Ok(CommandEnvelope {
        command_id: CommandId::from_str(COMMAND_ID)?,
        scenario_id,
        expected_revision: revision,
        actor: ActorRef {
            actor_id: Some("test-user".to_owned()),
            display_name: "Test User".to_owned(),
        },
        source: CommandSource::Desktop,
        command,
    })
}

fn apply(
    document: &ScenarioDocument,
    revision: Revision,
    command: ScenarioCommand,
) -> Result<eutheto_command::AppliedCommand, Box<dyn Error>> {
    let command_envelope = envelope(document.scenario_id, revision, command)?;
    Ok(apply_command(document, revision, &command_envelope)?)
}

fn assert_round_trip(
    document: &ScenarioDocument,
    revision: Revision,
    command: ScenarioCommand,
) -> Result<ScenarioDocument, Box<dyn Error>> {
    let original_json = serde_json::to_value(document)?;
    let applied = apply(document, revision, command)?;
    let inverse = applied
        .result
        .inverse
        .clone()
        .ok_or("successful test commands must have an inverse")?;
    let restored = apply(&applied.document, applied.result.new_revision, inverse)?;
    assert_eq!(serde_json::to_value(&restored.document)?, original_json);
    Ok(applied.document)
}

fn assert_leaf_command_family<const N: usize>(
    revision: Revision,
    add: ScenarioCommand,
    prepare_existing: impl FnOnce(&mut ScenarioDocument),
    commands: [ScenarioCommand; N],
) -> Result<ScenarioDocument, Box<dyn Error>> {
    let scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let mut scenario = assert_round_trip(&scenario, revision, add)?;
    prepare_existing(&mut scenario);
    for command in commands {
        assert_round_trip(&scenario, revision, command)?;
    }
    Ok(scenario)
}

#[test]
fn every_generic_leaf_command_has_an_exact_inverse() -> Result<(), Box<dyn Error>> {
    let revision = Revision::new(0);
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    let rule_id = RuleId::from_str(RULE_ID)?;
    let preference_id = RuleId::from_str(PREFERENCE_ID)?;
    let assignment_id = AssignmentId::from_str(ASSIGNMENT_ID)?;

    assert_leaf_command_family(
        revision,
        ScenarioCommand::AddEntity(AddEntity {
            entity_id,
            value: json!({ "name": "Ada" }),
        }),
        |scenario| {
            scenario
                .domain
                .entities
                .insert(entity_id, json!({ "name": "Ada" }));
        },
        [
            ScenarioCommand::UpdateEntity(UpdateEntity {
                entity_id,
                value: json!({ "name": "Grace" }),
            }),
            ScenarioCommand::RemoveEntity(eutheto_types::RemoveEntity { entity_id }),
        ],
    )?;

    assert_leaf_command_family(
        revision,
        ScenarioCommand::AddRule(AddRule {
            rule_id,
            value: json!({ "kind": "required" }),
        }),
        |scenario| {
            scenario
                .domain
                .rules
                .insert(rule_id, json!({ "kind": "required" }));
        },
        [
            ScenarioCommand::UpdateRule(UpdateRule {
                rule_id,
                value: json!({ "kind": "maximum" }),
            }),
            ScenarioCommand::RemoveRule(eutheto_types::RemoveRule { rule_id }),
        ],
    )?;

    assert_leaf_command_family(
        revision,
        ScenarioCommand::SetPreference(SetPreference {
            preference_id,
            value: Some(json!({ "weight": 7 })),
        }),
        |scenario| {
            scenario
                .domain
                .preferences
                .insert(preference_id, json!({ "weight": 3 }));
        },
        [
            ScenarioCommand::SetPreference(SetPreference {
                preference_id,
                value: Some(json!({ "weight": 9 })),
            }),
            ScenarioCommand::SetPreference(SetPreference {
                preference_id,
                value: None,
            }),
        ],
    )?;

    let scenario = assert_leaf_command_family(
        revision,
        ScenarioCommand::LockAssignment(LockAssignment {
            assignment_id,
            value: json!({ "personId": ENTITY_ID }),
        }),
        |scenario| {
            scenario
                .domain
                .locked_assignments
                .insert(assignment_id, json!({ "personId": ENTITY_ID }));
        },
        [ScenarioCommand::UnlockAssignment(UnlockAssignment {
            assignment_id,
        })],
    )?;

    assert_eq!(
        scenario.extensions.get("vendor.example"),
        Some(&json!({ "preserved": true }))
    );
    Ok(())
}

#[test]
fn batch_is_atomic_and_inverse_runs_in_reverse_order() -> Result<(), Box<dyn Error>> {
    let scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let revision = Revision::new(4);
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    let command = ScenarioCommand::ApplyBatch(CommandBatch {
        label: Some("Add and rename person".to_owned()),
        commands: vec![
            ScenarioCommand::AddEntity(AddEntity {
                entity_id,
                value: json!({ "name": "Ada" }),
            }),
            ScenarioCommand::UpdateEntity(UpdateEntity {
                entity_id,
                value: json!({ "name": "Grace" }),
            }),
        ],
    });

    let applied = apply(&scenario, revision, command)?;
    assert_eq!(
        applied.document.domain.entities.get(&entity_id),
        Some(&json!({ "name": "Grace" }))
    );
    assert_eq!(applied.result.change_set.changes.len(), 2);
    assert_eq!(applied.result.new_revision, Revision::new(5));
    assert_eq!(applied.summary, "Add and rename person (2 commands)");
    assert_eq!(applied.command_type, "apply_batch");

    let Some(ScenarioCommand::ApplyBatch(inverse)) = applied.result.inverse.clone() else {
        return Err("batch must produce a batch inverse".into());
    };
    assert!(matches!(
        inverse.commands.as_slice(),
        [
            ScenarioCommand::UpdateEntity(_),
            ScenarioCommand::RemoveEntity(_)
        ]
    ));
    let restored = apply(
        &applied.document,
        applied.result.new_revision,
        ScenarioCommand::ApplyBatch(inverse),
    )?;
    assert_eq!(
        serde_json::to_value(restored.document)?,
        serde_json::to_value(&scenario)?
    );

    let failing_batch = ScenarioCommand::ApplyBatch(CommandBatch {
        label: None,
        commands: vec![
            ScenarioCommand::AddEntity(AddEntity {
                entity_id,
                value: json!({ "name": "first" }),
            }),
            ScenarioCommand::AddEntity(AddEntity {
                entity_id,
                value: json!({ "name": "duplicate" }),
            }),
        ],
    });
    let before = serde_json::to_value(&scenario)?;
    let error = apply(&scenario, revision, failing_batch)
        .err()
        .ok_or("duplicate batch must fail")?;
    let command_error = error
        .downcast_ref::<CommandError>()
        .ok_or("failure must retain its typed command error")?;
    assert_eq!(command_error.code(), CODE_DUPLICATE_ENTITY);
    assert_eq!(serde_json::to_value(&scenario)?, before);
    Ok(())
}

#[test]
fn domain_command_uses_the_phase_02_pack_and_is_reversible() -> Result<(), Box<dyn Error>> {
    let mut scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let revision = Revision::new(0);
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    scenario.domain.entities.insert(
        entity_id,
        json!({ "id": entity_id, "enabled": false, "target": 0 }),
    );
    let command = ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
        command_type: "official.test.configure_entity".to_owned(),
        payload: json!({ "entityId": entity_id, "enabled": true, "target": 4 }),
    });
    assert_eq!(
        command_type(&command),
        "domain.official.test.configure_entity"
    );
    assert_eq!(
        human_summary(&command),
        "Apply official.test.configure_entity domain command"
    );
    let applied = apply(&scenario, revision, command)?;
    assert_eq!(
        applied.command_type,
        "domain.official.test.configure_entity"
    );
    let inverse = applied.result.inverse.ok_or("domain inverse is required")?;
    assert!(matches!(inverse, ScenarioCommand::ApplyDomainCommand(_)));
    let restored = apply(&applied.document, applied.result.new_revision, inverse)?;
    assert_eq!(
        serde_json::to_value(restored.document)?,
        serde_json::to_value(&scenario)?
    );
    Ok(())
}

#[test]
fn unsupported_pack_and_domain_commands_are_typed_failures() -> Result<(), Box<dyn Error>> {
    let unsupported_pack = document(UNSUPPORTED_PACK_ID)?;
    let revision = Revision::new(0);
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    let error = apply(
        &unsupported_pack,
        revision,
        ScenarioCommand::AddEntity(AddEntity {
            entity_id,
            value: json!({ "name": "Ada" }),
        }),
    )
    .err()
    .ok_or("unregistered pack must fail")?;
    assert!(matches!(
        error.downcast_ref::<CommandError>(),
        Some(CommandError::Unsupported { pack_id, .. }) if pack_id == UNSUPPORTED_PACK_ID
    ));

    let scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let error = apply(
        &scenario,
        revision,
        ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: "official.test.future_solver_action".to_owned(),
            payload: Value::Object(serde_json::Map::default()),
        }),
    )
    .err()
    .ok_or("unknown domain command must fail")?;
    assert!(
        matches!(
            error.downcast_ref::<CommandError>(),
            Some(CommandError::Unsupported { command_type, .. })
                if command_type == "official.test.future_solver_action"
        ),
        "unexpected error: {error:?}"
    );
    Ok(())
}

#[test]
fn validation_codes_and_application_are_stable_and_deterministic() -> Result<(), Box<dyn Error>> {
    let mut scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let revision = Revision::new(7);
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    scenario
        .domain
        .entities
        .insert(entity_id, json!({ "name": "existing" }));

    let duplicate = ScenarioCommand::AddEntity(AddEntity {
        entity_id,
        value: json!({ "name": "duplicate" }),
    });
    let first = apply(&scenario, revision, duplicate.clone())
        .err()
        .ok_or("duplicate must fail")?;
    let second = apply(&scenario, revision, duplicate)
        .err()
        .ok_or("duplicate must fail deterministically")?;
    assert_eq!(first.to_string(), second.to_string());
    assert_eq!(
        first
            .downcast_ref::<CommandError>()
            .ok_or("typed error expected")?
            .code(),
        CODE_DUPLICATE_ENTITY
    );

    let missing_id = EntityId::from_str("0195a5e4-7c00-7000-8000-000000000099")?;
    let error = apply(
        &scenario,
        revision,
        ScenarioCommand::UpdateEntity(UpdateEntity {
            entity_id: missing_id,
            value: json!({ "name": "missing" }),
        }),
    )
    .err()
    .ok_or("missing update target must fail")?;
    assert_eq!(
        error
            .downcast_ref::<CommandError>()
            .ok_or("typed error expected")?
            .code(),
        CODE_MISSING_ENTITY
    );

    let mismatched = ScenarioCommand::UpdateEntity(UpdateEntity {
        entity_id,
        value: json!({ "id": "0195a5e4-7c00-7000-8000-000000000099" }),
    });
    let error = apply(&scenario, revision, mismatched)
        .err()
        .ok_or("mismatched embedded stable id must fail")?;
    assert_eq!(
        error
            .downcast_ref::<CommandError>()
            .ok_or("typed error expected")?
            .code(),
        CODE_RECORD_ID_MISMATCH
    );

    let valid = ScenarioCommand::UpdateEntity(UpdateEntity {
        entity_id,
        value: json!({ "name": "updated" }),
    });
    let first = apply(&scenario, revision, valid.clone())?;
    let second = apply(&scenario, revision, valid)?;
    assert_eq!(first, second);
    assert!(first.result.validation_delta.added.is_empty());
    assert!(first.result.validation_delta.resolved.is_empty());
    Ok(())
}

#[test]
fn fast_validation_is_derived_from_the_registered_phase_02_pack() -> Result<(), Box<dyn Error>> {
    let mut scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let revision = Revision::new(0);
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    scenario.domain.entities.insert(
        entity_id,
        json!({ "id": entity_id, "enabled": false, "target": 0 }),
    );
    let command = ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
        command_type: "official.test.configure_entity".to_owned(),
        payload: json!({ "entityId": entity_id, "enabled": true, "target": 2 }),
    });
    let command_envelope = envelope(scenario.scenario_id, revision, command)?;
    let registry = official_registry()?;
    let applied = apply_command_with_registry(
        &scenario,
        revision,
        &command_envelope,
        &registry,
        &eutheto_types::CancellationToken::new(),
    )?;
    assert!(applied.result.validation_delta.added.is_empty());
    assert!(applied.result.validation_delta.resolved.is_empty());

    let inverse = applied.result.inverse.ok_or("inverse is required")?;
    let inverse_envelope = envelope(
        applied.document.scenario_id,
        applied.result.new_revision,
        inverse,
    )?;
    let restored = apply_command_with_registry(
        &applied.document,
        applied.result.new_revision,
        &inverse_envelope,
        &registry,
        &eutheto_types::CancellationToken::new(),
    )?;
    assert!(restored.result.validation_delta.added.is_empty());
    assert!(restored.result.validation_delta.resolved.is_empty());
    Ok(())
}

#[test]
fn credential_bearing_generic_commands_fail_at_every_ingress_shape() -> Result<(), Box<dyn Error>> {
    const SENTINEL: &str = "EUTHETO_SENTINEL_SECRET";
    const PROHIBITED_KEYS: [&str; 16] = [
        "providerApiKey",
        "providerClientSecret",
        "oauthCredentialId",
        "apiKeyId",
        "credentialRef",
        "credentialReference",
        "credentialHandle",
        "credentialItem",
        "credential_ref",
        "credential_reference",
        "credential_handle",
        "credential_item",
        "credential-ref",
        "credential-reference",
        "credential-handle",
        "credential-item",
    ];
    let entity_id = EntityId::from_str(ENTITY_ID)?;

    for (index, key) in PROHIBITED_KEYS.into_iter().enumerate() {
        let mut scenario = document(OFFICIAL_TEST_PACK_ID)?;
        let mut value = json!({"id": ENTITY_ID});
        let Some(object) = value.as_object_mut() else {
            return Err(std::io::Error::other("record fixture must be an object").into());
        };
        object.insert(key.to_owned(), Value::String(SENTINEL.to_owned()));
        let command = match index % 3 {
            0 => ScenarioCommand::AddEntity(AddEntity { entity_id, value }),
            1 => {
                scenario
                    .domain
                    .entities
                    .insert(entity_id, json!({"id": ENTITY_ID, "name": "existing"}));
                ScenarioCommand::UpdateEntity(UpdateEntity { entity_id, value })
            }
            _ => ScenarioCommand::ApplyBatch(CommandBatch {
                label: Some("credential ingress regression".to_owned()),
                commands: vec![
                    ScenarioCommand::AddEntity(AddEntity { entity_id, value }),
                    ScenarioCommand::RemoveEntity(eutheto_types::RemoveEntity { entity_id }),
                ],
            }),
        };
        let original = serde_json::to_vec(&scenario)?;
        let command_envelope = envelope(scenario.scenario_id, Revision::new(0), command)?;
        let Err(error) = apply_command(&scenario, Revision::new(0), &command_envelope) else {
            return Err(
                std::io::Error::other("credential-bearing command unexpectedly succeeded").into(),
            );
        };
        assert_eq!(error.code(), CODE_PROHIBITED_DATA, "key {key}");
        assert_eq!(serde_json::to_vec(&scenario)?, original, "key {key}");
        assert!(
            !original
                .windows(SENTINEL.len())
                .any(|window| window == SENTINEL.as_bytes()),
            "key {key}"
        );
    }
    for credential in [
        "Authorization: Bearer synthetic-bearer-sentinel",
        "-----BEGIN RSA PRIVATE KEY-----\nsynthetic\n-----END RSA PRIVATE KEY-----",
    ] {
        let scenario = document(OFFICIAL_TEST_PACK_ID)?;
        let command_envelope = envelope(
            scenario.scenario_id,
            Revision::new(0),
            ScenarioCommand::AddEntity(AddEntity {
                entity_id,
                value: json!({"id": ENTITY_ID, "headers": [credential]}),
            }),
        )?;
        let Err(error) = apply_command(&scenario, Revision::new(0), &command_envelope) else {
            return Err(
                std::io::Error::other("credential-shaped value unexpectedly succeeded").into(),
            );
        };
        assert_eq!(error.code(), CODE_PROHIBITED_DATA);
    }
    Ok(())
}

#[test]
fn harmless_key_substrings_remain_valid_command_data() -> Result<(), Box<dyn Error>> {
    let scenario = document(OFFICIAL_TEST_PACK_ID)?;
    let entity_id = EntityId::from_str(ENTITY_ID)?;
    let command = ScenarioCommand::AddEntity(AddEntity {
        entity_id,
        value: json!({
            "id": ENTITY_ID,
            "grid": "weekly",
            "tokenizer": "plain-text",
            "monkey": "capuchin"
        }),
    });
    let applied = apply(&scenario, Revision::new(0), command)?;
    assert_eq!(
        applied.document.domain.entities.get(&entity_id),
        Some(&json!({
            "id": ENTITY_ID,
            "grid": "weekly",
            "tokenizer": "plain-text",
            "monkey": "capuchin"
        }))
    );
    Ok(())
}
