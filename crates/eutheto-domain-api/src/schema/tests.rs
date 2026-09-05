use super::{
    ContractJsonLimits, DomainPackError, validate_contract_schema, validate_contract_value,
};
use serde_json::{Value, json};

fn validate(schema: &Value, value: &Value) -> Result<(), DomainPackError> {
    validate_contract_value(schema, value, ContractJsonLimits::DEFAULT)
}

#[test]
fn serialized_byte_ceiling_includes_utf8_and_json_escaping() -> Result<(), DomainPackError> {
    let value = json!(["é", "\"\n"]);
    assert_eq!(super::bounded_json_size(&value, 13)?, 13);
    assert!(super::bounded_json_size(&value, 12).is_err());
    Ok(())
}

#[test]
fn string_bounds_are_inclusive_and_count_unicode_scalars() -> Result<(), DomainPackError> {
    let schema = json!({"type": "string", "minLength": 1, "maxLength": 2});
    validate(&schema, &json!("a"))?;
    validate(&schema, &json!("é\u{1d11e}"))?;
    validate(&schema, &json!("e\u{301}"))?;
    assert!(validate(&schema, &json!("")).is_err());
    assert!(validate(&schema, &json!("abc")).is_err());
    // Two grapheme clusters, but three Unicode scalar values.
    assert!(validate(&schema, &json!("e\u{301}\u{1d11e}")).is_err());

    let empty = json!({"minLength": 0, "maxLength": 0});
    validate(&empty, &json!(""))?;
    assert!(validate(&empty, &json!("a")).is_err());
    Ok(())
}

#[test]
fn array_bounds_are_inclusive_and_allow_empty_arrays() -> Result<(), DomainPackError> {
    let schema = json!({"type": "array", "minItems": 1, "maxItems": 2});
    validate(&schema, &json!([null]))?;
    validate(&schema, &json!([null, null]))?;
    assert!(validate(&schema, &json!([])).is_err());
    assert!(validate(&schema, &json!([null, null, null])).is_err());

    let empty = json!({"minItems": 0, "maxItems": 0});
    validate(&empty, &json!([]))?;
    assert!(validate(&empty, &json!([null])).is_err());
    Ok(())
}

#[test]
fn bounds_require_nonnegative_integer_schema_values() {
    for keyword in ["minLength", "maxLength", "minItems", "maxItems"] {
        for malformed in [
            json!(-1),
            json!(1.5),
            json!(1.0),
            json!("1"),
            json!(true),
            Value::Null,
            json!([]),
            json!({}),
        ] {
            let schema = json!({keyword: malformed});
            assert!(validate_contract_schema(&schema).is_err());
        }
    }
}

#[test]
fn standalone_bounds_keep_the_existing_strict_constraint_applicability()
-> Result<(), DomainPackError> {
    for (keyword, value, wrong_type) in [
        ("minLength", json!("ab"), json!([1, 2])),
        ("maxLength", json!("ab"), json!(2)),
        ("minItems", json!([1, 2]), json!("ab")),
        ("maxItems", json!([1, 2]), json!({})),
    ] {
        let schema = json!({keyword: 2});
        validate(&schema, &value)?;
        assert!(validate(&schema, &wrong_type).is_err());
    }
    Ok(())
}

#[test]
fn unsigned_bounds_do_not_narrow_to_signed_or_platform_sized_integers()
-> Result<(), DomainPackError> {
    validate(&json!({"maxLength": u64::MAX}), &json!("text"))?;
    validate(&json!({"maxItems": u64::MAX}), &json!([null]))?;
    let string_minimum = json!({"minLength": u64::MAX});
    let array_minimum = json!({"minItems": u64::MAX});
    validate_contract_schema(&string_minimum)?;
    validate_contract_schema(&array_minimum)?;
    assert!(validate(&string_minimum, &json!("text")).is_err());
    assert!(validate(&array_minimum, &json!([null])).is_err());
    Ok(())
}

#[test]
fn nested_bounds_apply_in_properties_items_additional_properties_and_one_of()
-> Result<(), DomainPackError> {
    let schema = json!({
        "type": "object",
        "required": ["labels"],
        "properties": {
            "labels": {
                "type": "array", "minItems": 1, "maxItems": 2,
                "items": {"oneOf": [
                    {"type": "string", "minLength": 1, "maxLength": 2},
                    {"const": null}
                ]}
            }
        },
        "additionalProperties": {"type": "string", "minLength": 1, "maxLength": 2}
    });
    validate(
        &schema,
        &json!({"labels": ["é\u{1d11e}", null], "note": "ok"}),
    )?;
    for invalid in [
        json!({"labels": []}),
        json!({"labels": [null, null, null]}),
        json!({"labels": [""]}),
        json!({"labels": ["abc"]}),
        json!({"labels": [null], "note": "abc"}),
    ] {
        assert!(validate(&schema, &invalid).is_err());
    }
    let malformed = json!({"items": {"oneOf": [{"minLength": -1}, {"const": null}]}});
    assert!(validate_contract_schema(&malformed).is_err());

    let overlapping = json!({"oneOf": [{"minLength": 1}, {"maxLength": 2}]});
    assert!(validate(&overlapping, &json!("a")).is_err());
    validate(&overlapping, &json!("abc"))?;
    Ok(())
}

#[test]
fn field_bounds_do_not_widen_portable_safety_ceilings() -> Result<(), DomainPackError> {
    let schema = json!({"minLength": 1, "maxLength": 2});
    let byte_limits = ContractJsonLimits {
        max_string_bytes: 2,
        ..ContractJsonLimits::DEFAULT
    };
    validate_contract_value(&schema, &json!("é"), byte_limits)?;
    validate(&schema, &json!("éé"))?;
    assert!(validate_contract_value(&schema, &json!("éé"), byte_limits).is_err());

    let array = json!({"maxItems": 3});
    let collection_limits = ContractJsonLimits {
        max_collection_items: 3,
        ..ContractJsonLimits::DEFAULT
    };
    validate_contract_value(&array, &json!([null, null]), collection_limits)?;
    // The root also counts toward the existing aggregate node ceiling.
    assert!(
        validate_contract_value(&array, &json!([null, null, null]), collection_limits).is_err()
    );

    let nested = json!({"maxItems": 1, "items": {"maxItems": 1}});
    let depth_limits = ContractJsonLimits {
        max_depth: 1,
        ..ContractJsonLimits::DEFAULT
    };
    validate_contract_value(&nested, &json!([[]]), depth_limits)?;
    assert!(validate_contract_value(&nested, &json!([[null]]), depth_limits).is_err());

    let serialized_limits = ContractJsonLimits {
        max_serialized_bytes: 3,
        ..ContractJsonLimits::DEFAULT
    };
    validate_contract_value(&schema, &json!("a"), serialized_limits)?;
    assert!(validate_contract_value(&schema, &json!("ab"), serialized_limits).is_err());
    Ok(())
}

#[test]
fn existing_numeric_uuid_prefix_const_and_closed_object_constraints_remain_in_force()
-> Result<(), DomainPackError> {
    let schema = json!({
        "type": "object", "additionalProperties": false,
        "required": ["id", "count", "kind"],
        "properties": {
            "id": {"type": "string", "format": "uuid", "minLength": 36, "maxLength": 36},
            "count": {"type": "integer", "minimum": -1, "maximum": 1},
            "kind": {"const": "record"},
            "label": {"type": "string", "pattern": "^label-", "maxLength": 8}
        }
    });
    let value = json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "count": -1, "kind": "record", "label": "label-ok"
    });
    validate(&schema, &value)?;
    let mut upper = value.clone();
    upper["count"] = json!(1);
    validate(&schema, &upper)?;
    for (field, replacement) in [
        ("id", json!("z0000000-0000-0000-0000-000000000001")),
        ("count", json!(-2)),
        ("count", json!(2)),
        ("count", json!(1.5)),
        ("kind", json!("other")),
        ("label", json!("other-ok")),
        ("extra", json!(true)),
    ] {
        let mut invalid = value.clone();
        invalid[field] = replacement;
        assert!(validate(&schema, &invalid).is_err());
    }
    assert!(validate(&schema, &json!({"count": 0, "kind": "record"})).is_err());
    assert!(validate_contract_schema(&json!({"minimum": 1.5})).is_err());
    assert!(validate_contract_schema(&json!({"uniqueItems": true})).is_err());
    Ok(())
}
