mod support;

use eutheto_domain_api::{ContractJsonLimits, PortableImportContext, validate_contract_value};
use eutheto_types::{PackId, ScenarioDomain, SemanticCapability};
use eutheto_workforce::{
    generated_workforce_pack_contract::WORKFORCE_PACK_CONTRACT_JSON,
    portable::{export_portable, import_portable},
};
use serde_json::{Value, json};
use std::error::Error;
use support::{fixture, id};

#[test]
fn portable_roundtrip_preserves_raw_semantics_extensions_and_explicit_host_shell()
-> Result<(), Box<dyn Error>> {
    let mut original = fixture()?;
    let person = original
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?;
    person["id"] = json!(id(1).to_ascii_uppercase());
    person["externalId"] = json!(id(99));
    person["name"] = json!(id(98));
    original.domain.rules.insert(
        id(30).parse()?,
        json!({
            "kind":"maximumHours", "id":id(30), "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}}, "bucketId":id(3),
            "window":{"kind":"calendar","calendarId":id(2),"membership":"intersection"},
            "maximumMinutes":480
        }),
    );
    original.domain.preferences.insert(
        id(31).parse()?,
        json!({
            "kind":"workloadBalance", "id":id(31), "active":true,
            "scope":{"people":{"kind":"all"}}, "priority":"normal", "weight":3,
            "workloadPolicyId":id(10)
        }),
    );
    original.extensions.insert(
        "nonsemantic.future.presentation".to_owned(),
        json!({
            "note":"Literal `shift` ${review} r#\"tag\"#", "ordering":[3,1,2],
            "empty":null, "label":id(77)
        }),
    );
    let portable = export_portable(&original)?;
    let contract: Value = serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON)?;
    validate_contract_value(
        &contract["portableSchema"],
        &portable.payload,
        ContractJsonLimits::DEFAULT,
    )?;
    let wire = serde_json::to_vec(&portable)?;
    let decoded = serde_json::from_slice(&wire)?;
    let mut shell = original.clone();
    shell.scenario_id = id(101).parse()?;
    shell.metadata.title = "Explicit destination title".to_owned();
    shell.domain = ScenarioDomain::default();
    shell.extensions.clear();
    let context = PortableImportContext {
        scenario_shell: shell.clone(),
    };
    let imported = import_portable(&decoded, &context)?;
    let mut expected = shell.clone();
    expected.domain = original.domain.clone();
    expected.extensions = original.extensions.clone();
    assert_eq!(imported, expected);
    assert_eq!(context.scenario_shell, shell);
    assert_eq!(export_portable(&imported)?, portable);
    Ok(())
}

#[test]
fn portable_versions_and_semantic_requirements_never_fall_back() -> Result<(), Box<dyn Error>> {
    let original = fixture()?;
    let portable = export_portable(&original)?;
    let context = PortableImportContext {
        scenario_shell: original.clone(),
    };
    for version in [0, 2, u32::MAX] {
        let mut changed = portable.clone();
        changed.schema_version = version;
        assert!(import_portable(&changed, &context).is_err());
        changed = portable.clone();
        changed.payload["schemaVersion"] = json!(version);
        assert!(import_portable(&changed, &context).is_err());
        let mut shell = context.clone();
        shell.scenario_shell.domain_pack.schema_version = version;
        assert!(import_portable(&portable, &shell).is_err());
        assert!(export_portable(&shell.scenario_shell).is_err());
    }
    for capabilities in [
        vec![],
        vec![SemanticCapability {
            id: "official.workforce.portable".to_owned(),
            version: 2,
        }],
        vec![SemanticCapability {
            id: "official.workforce.future".to_owned(),
            version: 1,
        }],
        vec![
            SemanticCapability {
                id: "official.workforce.portable".to_owned(),
                version: 1,
            },
            SemanticCapability {
                id: "official.workforce.future".to_owned(),
                version: 1,
            },
        ],
    ] {
        let mut changed = portable.clone();
        changed.required_capabilities = capabilities.into_iter().collect();
        assert!(import_portable(&changed, &context).is_err());
    }
    let mut wrong_pack = portable.clone();
    wrong_pack.pack_id = PackId::new("official.test")?;
    assert!(import_portable(&wrong_pack, &context).is_err());
    let mut wrong_shell = context;
    wrong_shell.scenario_shell.domain_pack.id = PackId::new("official.test")?;
    assert!(import_portable(&portable, &wrong_shell).is_err());
    Ok(())
}

#[test]
fn portable_ingress_rejects_undeclared_shapes_unsafe_data_and_broken_references()
-> Result<(), Box<dyn Error>> {
    let original = fixture()?;
    let portable = export_portable(&original)?;
    let context = PortableImportContext {
        scenario_shell: original,
    };
    for attack in [
        "unknownVariant",
        "tupleStruct",
        "unknownField",
        "nullReference",
        "wrongKind",
        "duplicateKey",
        "semanticExtension",
        "secretExtension",
        "deepExtension",
        "oversizedString",
        "hostOverride",
    ] {
        let mut changed = portable.clone();
        match attack {
            "unknownVariant" => changed.payload["entities"][id(1)]["kind"] = json!("futurePerson"),
            "tupleStruct" => changed.payload["entities"][id(1)]["workloadWeight"] = json!([1, 1]),
            "unknownField" => changed.payload["entities"][id(1)]["newEligibility"] = json!(true),
            "nullReference" => changed.payload["entities"][id(1)]["homeLocationId"] = Value::Null,
            "wrongKind" => changed.payload["entities"][id(1)]["homeLocationId"] = json!(id(2)),
            "duplicateKey" => {
                changed.payload["entities"][id(1).to_ascii_uppercase()] =
                    changed.payload["entities"][id(1)].clone();
            }
            "semanticExtension" => {
                changed.payload["extensions"]["official.workforce.future"] =
                    json!({"enabled":true});
            }
            "secretExtension" => {
                changed.payload["extensions"]["nonsemantic.example.private"] =
                    json!({"apiKey":"DO-NOT-ECHO"});
            }
            "deepExtension" => {
                let mut value = Value::Null;
                for _ in 0..=ContractJsonLimits::DEFAULT.max_depth {
                    value = json!([value]);
                }
                changed.payload["extensions"]["nonsemantic.example.deep"] = value;
            }
            "oversizedString" => {
                changed.payload["entities"][id(1)]["name"] =
                    json!("x".repeat(ContractJsonLimits::DEFAULT.max_string_bytes + 1));
            }
            _ => changed.payload["metadata"] = json!({"title":"Host override"}),
        }
        let error = import_portable(&changed, &context)
            .err()
            .ok_or_else(|| format!("accepted invalid portable input: {attack}"))?;
        assert!(!error.to_string().contains("DO-NOT-ECHO"));
        assert_eq!(export_portable(&context.scenario_shell)?, portable);
    }
    Ok(())
}

#[test]
fn caller_built_deep_values_reject_before_recursive_serialization() -> Result<(), Box<dyn Error>> {
    let original = fixture()?;
    let mut portable = export_portable(&original)?;
    let context = PortableImportContext {
        scenario_shell: original,
    };
    let mut nested = Value::Null;
    for _ in 0..4096 {
        nested = Value::Array(vec![nested]);
    }
    portable.payload["extensions"]["nonsemantic.example.deep"] = nested;
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let rejected = import_portable(&portable, &context).is_err();
            // The deliberately adversarial caller-owned tree also needs nonrecursive disposal.
            let mut value = portable.payload["extensions"]["nonsemantic.example.deep"].take();
            while let Value::Array(mut items) = value {
                value = items.pop().unwrap_or(Value::Null);
            }
            assert!(rejected);
        })?
        .join()
        .map_err(|_| "deep-input rejection panicked")?;
    Ok(())
}
