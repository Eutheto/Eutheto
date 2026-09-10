use crate::DomainPackError;
use eutheto_types::{PortableJsonLimits, validate_nonsecret_portable_json};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

#[cfg(test)]
mod tests;

const ALLOWED_SCHEMA_KEYWORDS: [&str; 16] = [
    "$schema",
    "type",
    "additionalProperties",
    "required",
    "properties",
    "const",
    "items",
    "oneOf",
    "format",
    "minimum",
    "maximum",
    "pattern",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
];

/// Bounds for generated contract schemas and untrusted values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractJsonLimits {
    pub max_serialized_bytes: usize,
    pub max_depth: usize,
    pub max_string_bytes: usize,
    pub max_collection_items: usize,
}

impl ContractJsonLimits {
    /// Phase-02 pack boundary limits, no wider than the shared scenario limit.
    pub const DEFAULT: Self = Self {
        max_serialized_bytes: 16 * 1024 * 1024,
        max_depth: 32,
        max_string_bytes: 16 * 1024,
        max_collection_items: 100_000,
    };
}

/// Measures compact JSON bytes without materializing another serialized copy.
/// Stops at the inclusive byte ceiling. Callers must separately bound recursive input depth.
///
/// # Errors
///
/// Rejects serialization failure or a value exceeding `max_bytes`, without echoing its data.
pub fn bounded_json_size<T: serde::Serialize + ?Sized>(
    value: &T,
    max_bytes: usize,
) -> Result<usize, DomainPackError> {
    let mut counter = JsonByteCounter {
        bytes: 0,
        max_bytes,
    };
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| invalid_error("$", "JSON serialization failed or exceeds its byte bound"))?;
    Ok(counter.bytes)
}

struct JsonByteCounter {
    bytes: usize,
    max_bytes: usize,
}

impl std::io::Write for JsonByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|size| *size <= self.max_bytes)
            .ok_or_else(|| std::io::Error::other("JSON byte limit exceeded"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Checks that a generated schema uses only the deliberately small Phase-02 vocabulary.
///
/// Empty schemas are allowed for opaque before/after values, but every object schema declaring
/// properties must explicitly declare `additionalProperties`.
///
/// # Errors
///
/// Returns [`DomainPackError::InvalidPayload`] when the schema is not an object, exceeds the
/// nesting limit, or uses an unsupported or malformed schema construct.
pub fn validate_contract_schema(schema: &Value) -> Result<(), DomainPackError> {
    validate_schema_at(schema, "$", 0)
}

fn validate_schema_at(schema: &Value, path: &str, depth: usize) -> Result<(), DomainPackError> {
    if depth > ContractJsonLimits::DEFAULT.max_depth {
        return invalid(path, "schema nesting limit exceeded");
    }
    let object = schema
        .as_object()
        .ok_or_else(|| invalid_error(path, "schema must be an object"))?;
    if let Some(key) = object
        .keys()
        .find(|key| !ALLOWED_SCHEMA_KEYWORDS.contains(&key.as_str()))
    {
        return invalid(path, format!("unsupported schema keyword {key}"));
    }
    validate_schema_type(object, path)?;
    validate_schema_required(object, path)?;
    validate_schema_properties(object, path, depth)?;
    validate_schema_children(object, path, depth)?;
    validate_schema_constraints(object, path)
}

fn validate_schema_type(object: &Map<String, Value>, path: &str) -> Result<(), DomainPackError> {
    if let Some(kind) = object.get("type") {
        let Some(kind) = kind.as_str() else {
            return invalid(path, "type must be a string");
        };
        if !matches!(
            kind,
            "object" | "array" | "string" | "integer" | "boolean" | "null"
        ) {
            return invalid(path, format!("unsupported schema type {kind}"));
        }
    }
    Ok(())
}

fn validate_schema_required(
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), DomainPackError> {
    if let Some(required) = object.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| invalid_error(path, "required must be an array"))?;
        let mut names = BTreeSet::new();
        for name in required {
            let name = name
                .as_str()
                .ok_or_else(|| invalid_error(path, "required names must be strings"))?;
            if !names.insert(name) {
                return invalid(path, format!("duplicate required property {name}"));
            }
        }
    }
    Ok(())
}

fn validate_schema_properties(
    object: &Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<(), DomainPackError> {
    let Some(properties_value) = object.get("properties") else {
        return Ok(());
    };
    let properties = properties_value
        .as_object()
        .ok_or_else(|| invalid_error(path, "properties must be an object"))?;
    if !object.contains_key("additionalProperties") {
        return invalid(
            path,
            "object properties require explicit additionalProperties",
        );
    }
    if object
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .any(|name| !properties.contains_key(name))
        })
    {
        return invalid(path, "required property is absent from properties");
    }
    for (name, child) in properties {
        validate_schema_at(child, &format!("{path}/properties/{name}"), depth + 1)?;
    }
    Ok(())
}

fn validate_schema_children(
    object: &Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<(), DomainPackError> {
    if let Some(additional) = object.get("additionalProperties")
        && !additional.is_boolean()
    {
        validate_schema_at(
            additional,
            &format!("{path}/additionalProperties"),
            depth + 1,
        )?;
    }
    if let Some(items) = object.get("items") {
        validate_schema_at(items, &format!("{path}/items"), depth + 1)?;
    }
    if let Some(options) = object.get("oneOf") {
        let options = options
            .as_array()
            .ok_or_else(|| invalid_error(path, "oneOf must be an array"))?;
        if options.is_empty() {
            return invalid(path, "oneOf must not be empty");
        }
        for (index, option) in options.iter().enumerate() {
            validate_schema_at(option, &format!("{path}/oneOf/{index}"), depth + 1)?;
        }
    }
    Ok(())
}

fn validate_schema_constraints(
    object: &Map<String, Value>,
    path: &str,
) -> Result<(), DomainPackError> {
    if let Some(format) = object.get("format")
        && !matches!(format.as_str(), Some("uuid" | "scenario-change-path"))
    {
        return invalid(path, "unsupported schema string format");
    }
    if let Some(pattern) = object.get("pattern") {
        let pattern = pattern
            .as_str()
            .ok_or_else(|| invalid_error(path, "pattern must be a string"))?;
        if !pattern.starts_with('^')
            || pattern[1..].chars().any(|character| {
                matches!(character, '^' | '$' | '[' | '(' | '*' | '+' | '?' | '\\')
            })
        {
            return invalid(path, "only anchored literal-prefix patterns are supported");
        }
    }
    for bound in ["minimum", "maximum"] {
        if object
            .get(bound)
            .is_some_and(|value| value.as_i64().is_none())
        {
            return invalid(path, format!("{bound} must be an i64 integer"));
        }
    }
    for bound in ["minLength", "maxLength", "minItems", "maxItems"] {
        if object
            .get(bound)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return invalid(path, format!("{bound} must be a nonnegative integer"));
        }
    }
    Ok(())
}

/// An immutable owned schema checked once for repeated operation-local validation.
/// Value validation retains the same byte, portability and structural guards as
/// [`validate_contract_value`]; this type carries no global state.
pub struct ValidatedContractSchema(Value);

impl ValidatedContractSchema {
    /// Takes ownership of a schema without permitting later mutation.
    ///
    /// # Errors
    ///
    /// Rejects unsupported or malformed schema content.
    pub fn new(schema: Value) -> Result<Self, DomainPackError> {
        validate_contract_schema(&schema)?;
        Ok(Self(schema))
    }

    /// Validates one value against the already-checked schema.
    ///
    /// # Errors
    ///
    /// Rejects the same unsafe, oversized or nonconforming values as
    /// [`validate_contract_value`].
    pub fn validate(
        &self,
        value: &Value,
        limits: ContractJsonLimits,
    ) -> Result<(), DomainPackError> {
        validate_value_with_schema(&self.0, value, limits)
    }
}

/// Strictly and boundedly validates an untrusted value against a validated generated schema.
///
/// # Errors
///
/// Returns [`DomainPackError::InvalidPayload`] when the schema is invalid, the value exceeds a
/// configured bound, contains prohibited portable JSON, or does not satisfy the schema.
pub fn validate_contract_value(
    schema: &Value,
    value: &Value,
    limits: ContractJsonLimits,
) -> Result<(), DomainPackError> {
    validate_contract_schema(schema)?;
    validate_value_with_schema(schema, value, limits)
}

pub(super) fn validate_query_parameters(
    schema: &Value,
    value: &Value,
    limits: ContractJsonLimits,
) -> Result<(), DomainPackError> {
    validate_contract_schema(schema)?;
    validate_query_structure(value, 0, limits)?;
    bounded_json_size(value, limits.max_serialized_bytes)?;
    validate_value_at(schema, value, "$", 0, limits)
}

fn validate_query_structure(
    value: &Value,
    depth: usize,
    limits: ContractJsonLimits,
) -> Result<(), DomainPackError> {
    if depth > limits.max_depth {
        return invalid("$", "query nesting limit exceeded");
    }
    match value {
        Value::String(text) if text.len() > limits.max_string_bytes => {
            return invalid("$", "query string byte limit exceeded");
        }
        Value::Array(items) => {
            if items.len() > limits.max_collection_items {
                return invalid("$", "query collection limit exceeded");
            }
            for item in items {
                validate_query_structure(item, depth + 1, limits)?;
            }
        }
        Value::Object(fields) => {
            if fields.len() > limits.max_collection_items {
                return invalid("$", "query collection limit exceeded");
            }
            for (key, item) in fields {
                if key.len() > limits.max_string_bytes {
                    return invalid("$", "query key byte limit exceeded");
                }
                validate_query_structure(item, depth + 1, limits)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn validate_value_with_schema(
    schema: &Value,
    value: &Value,
    limits: ContractJsonLimits,
) -> Result<(), DomainPackError> {
    bounded_json_size(value, limits.max_serialized_bytes)?;
    let safety_value = scrub_json_pointers(schema, value);
    validate_nonsecret_portable_json(
        &safety_value,
        &PortableJsonLimits {
            max_depth: limits.max_depth,
            max_string_bytes: limits.max_string_bytes,
            max_collection_items: limits.max_collection_items,
        },
    )
    .map_err(|error| invalid_error("$", error.to_string()))?;
    validate_value_at(schema, value, "$", 0, limits)
}

fn scrub_json_pointers(schema: &Value, value: &Value) -> Value {
    if schema.get("format").and_then(Value::as_str) == Some("scenario-change-path")
        && value.as_str().is_some_and(is_scenario_change_path)
    {
        return Value::String("scenario-field-reference".to_owned());
    }
    if schema
        .get("pattern")
        .and_then(Value::as_str)
        .is_some_and(|pattern| pattern.starts_with("^/domain/"))
        && value.is_string()
    {
        return Value::String("domain-field-reference".to_owned());
    }
    if let (Some(properties), Some(values)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        return Value::Object(
            values
                .iter()
                .map(|(key, item)| {
                    let child_schema = properties
                        .get(key)
                        .or_else(|| schema.get("additionalProperties"));
                    (
                        key.clone(),
                        child_schema
                            .map_or_else(|| item.clone(), |child| scrub_json_pointers(child, item)),
                    )
                })
                .collect(),
        );
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        return Value::Array(
            values
                .iter()
                .map(|item| scrub_json_pointers(items, item))
                .collect(),
        );
    }
    value.clone()
}

/// These are the field-reference roots emitted by the current typed scenario commands,
/// not filesystem paths. Only a schema-declared change-path field receives this treatment.
fn is_scenario_change_path(value: &str) -> bool {
    value == "/settings" || value.starts_with("/domain/")
}

fn validate_value_at(
    schema: &Value,
    value: &Value,
    path: &str,
    depth: usize,
    limits: ContractJsonLimits,
) -> Result<(), DomainPackError> {
    if depth > limits.max_depth {
        return invalid(path, "value nesting limit exceeded");
    }
    let object = schema
        .as_object()
        .ok_or_else(|| invalid_error(path, "schema must be an object"))?;
    if let Some(expected) = object.get("const")
        && value != expected
    {
        return invalid(path, "value does not match const");
    }
    if let Some(options) = object.get("oneOf").and_then(Value::as_array) {
        let matches = options
            .iter()
            .filter(|option| validate_value_at(option, value, path, depth + 1, limits).is_ok())
            .count();
        if matches != 1 {
            return invalid(path, "value must match exactly one oneOf branch");
        }
    }
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        let valid = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => {
                value.as_i64().is_some()
                    || value
                        .as_u64()
                        .is_some_and(|item| i64::try_from(item).is_ok())
            }
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        };
        if !valid {
            return invalid(path, format!("expected {kind}"));
        }
    }
    if let Some(format) = object.get("format").and_then(Value::as_str)
        && format == "uuid"
        && value
            .as_str()
            .and_then(|text| Uuid::parse_str(text).ok())
            .is_none()
    {
        return invalid(path, "expected UUID");
    }
    if object.get("format").and_then(Value::as_str) == Some("scenario-change-path")
        && !value.as_str().is_some_and(is_scenario_change_path)
    {
        return invalid(path, "expected scenario change field reference");
    }
    if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
        let prefix = &pattern[1..];
        if !value.as_str().is_some_and(|text| text.starts_with(prefix)) {
            return invalid(path, format!("expected prefix {prefix}"));
        }
    }
    if object.contains_key("minimum") || object.contains_key("maximum") {
        let number = value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|item| i64::try_from(item).ok()))
            .ok_or_else(|| invalid_error(path, "expected bounded integer"))?;
        if object
            .get("minimum")
            .and_then(Value::as_i64)
            .is_some_and(|minimum| number < minimum)
            || object
                .get("maximum")
                .and_then(Value::as_i64)
                .is_some_and(|maximum| number > maximum)
        {
            return invalid(path, "integer is outside declared bounds");
        }
    }
    validate_length_constraints(object, value, path)?;
    if let Some(values) = value.as_object() {
        validate_object(object, values, path, depth, limits)?;
    }
    if let Some(values) = value.as_array() {
        if values.len() > limits.max_collection_items {
            return invalid(path, "collection limit exceeded");
        }
        if let Some(items) = object.get("items") {
            for (index, item) in values.iter().enumerate() {
                validate_value_at(items, item, &format!("{path}/{index}"), depth + 1, limits)?;
            }
        }
    }
    Ok(())
}

fn validate_length_constraints(
    schema: &Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), DomainPackError> {
    if schema.contains_key("minLength") || schema.contains_key("maxLength") {
        let text = value
            .as_str()
            .ok_or_else(|| invalid_error(path, "expected bounded string"))?;
        // JSON Schema counts Unicode scalar values, independently of the portable byte cap.
        let length = text.chars().count() as u64;
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| length > maximum)
        {
            return invalid(path, "string is outside declared length bounds");
        }
    }
    if schema.contains_key("minItems") || schema.contains_key("maxItems") {
        let values = value
            .as_array()
            .ok_or_else(|| invalid_error(path, "expected bounded array"))?;
        let length = values.len() as u64;
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum)
            || schema
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| length > maximum)
        {
            return invalid(path, "array is outside declared length bounds");
        }
    }
    Ok(())
}

fn validate_object(
    schema: &Map<String, Value>,
    values: &Map<String, Value>,
    path: &str,
    depth: usize,
    limits: ContractJsonLimits,
) -> Result<(), DomainPackError> {
    if values.len() > limits.max_collection_items {
        return invalid(path, "object item limit exceeded");
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !values.contains_key(name) {
                return invalid(path, format!("missing required property {name}"));
            }
        }
    }
    for (name, item) in values {
        if let Some(child) = properties.and_then(|items| items.get(name)) {
            validate_value_at(child, item, &format!("{path}/{name}"), depth + 1, limits)?;
            continue;
        }
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => {
                return invalid(path, format!("unknown property {name}"));
            }
            Some(Value::Object(_)) => validate_value_at(
                &schema["additionalProperties"],
                item,
                &format!("{path}/{name}"),
                depth + 1,
                limits,
            )?,
            Some(Value::Bool(true)) | None => {}
            Some(_) => return invalid(path, "invalid additionalProperties schema"),
        }
    }
    Ok(())
}

fn invalid(path: &str, message: impl Into<String>) -> Result<(), DomainPackError> {
    Err(invalid_error(path, message))
}

fn invalid_error(path: &str, message: impl Into<String>) -> DomainPackError {
    DomainPackError::InvalidPayload {
        path: path.to_owned(),
        message: message.into(),
    }
}
