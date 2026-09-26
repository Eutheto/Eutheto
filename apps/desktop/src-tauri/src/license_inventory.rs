//! Bounded, offline lock-workspace metadata, not installed-artifact attribution or license approval.

use eutheto_core::bounded_json_size;
use serde_json::{Map, Value};

use super::{ApiError, boundary_error};

pub(super) const MAX_LICENSE_INVENTORY_COMPACT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PACKAGES: usize = 4096;
const MAX_METADATA_BYTES: usize = 1024;
const MAX_LICENSE_BYTES: usize = 4096;
const INVENTORY_SOURCE: &str = include_str!("../../../../xtask/generated/license-inventory.json");
const AUTHORITATIVE_INPUTS: [&str; 3] = [
    "Cargo.lock",
    "pnpm-lock.yaml",
    "xtask/supply-chain-inputs.json",
];

pub(super) fn read_inventory() -> Result<Value, ApiError> {
    decode_inventory(INVENTORY_SOURCE)
}

fn decode_inventory(source: &str) -> Result<Value, ApiError> {
    if source.len() > MAX_LICENSE_INVENTORY_COMPACT_BYTES {
        return Err(inventory_limit_error().into());
    }
    let mut inventory: Value =
        serde_json::from_str(source).map_err(|_| invalid_inventory_error())?;
    let root = inventory
        .as_object_mut()
        .ok_or_else(invalid_inventory_error)?;
    if !has_fields(
        root,
        &[
            "schemaVersion",
            "generatedBy",
            "authoritativeInputs",
            "packages",
        ],
        &[],
    ) || root.get("schemaVersion").and_then(Value::as_u64) != Some(2)
        || root.get("generatedBy").and_then(Value::as_str) != Some("cargo xtask licenses generate")
    {
        return Err(invalid_inventory_error().into());
    }
    let inputs = root
        .get("authoritativeInputs")
        .and_then(Value::as_array)
        .ok_or_else(invalid_inventory_error)?;
    if inputs.len() != AUTHORITATIVE_INPUTS.len()
        || !inputs
            .iter()
            .zip(AUTHORITATIVE_INPUTS)
            .all(|(value, expected)| value.as_str() == Some(expected))
    {
        return Err(invalid_inventory_error().into());
    }
    let packages = root
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(invalid_inventory_error)?;
    if packages.len() > MAX_PACKAGES {
        return Err(inventory_limit_error().into());
    }
    for package in packages {
        validate_package(package)?;
    }
    // Keep the generated fields unchanged, including absent checksums and NOASSERTION.
    root.insert(
        "scope".to_owned(),
        Value::String("lockedWorkspace".to_owned()),
    );
    bounded_json_size(&inventory, MAX_LICENSE_INVENTORY_COMPACT_BYTES)
        .map_err(|_| inventory_limit_error())?;
    Ok(inventory)
}

fn validate_package(value: &Value) -> Result<(), ApiError> {
    let package = value.as_object().ok_or_else(invalid_inventory_error)?;
    if !has_fields(
        package,
        &[
            "ecosystem",
            "name",
            "version",
            "kind",
            "licenseConcluded",
            "source",
        ],
        &["checksum"],
    ) || !matches!(
        package.get("ecosystem").and_then(Value::as_str),
        Some("cargo" | "npm")
    ) || !matches!(
        package.get("kind").and_then(Value::as_str),
        Some("workspace" | "dependency")
    ) {
        return Err(invalid_inventory_error().into());
    }
    for field in ["name", "version", "source"] {
        bounded_string(package.get(field), MAX_METADATA_BYTES)?;
    }
    bounded_string(package.get("licenseConcluded"), MAX_LICENSE_BYTES)?;
    // The generator always emits a string source and omits an unknown checksum;
    // neither field has a null representation in source schema 2.
    if let Some(checksum) = package.get("checksum") {
        let checksum = checksum.as_object().ok_or_else(invalid_inventory_error)?;
        if !has_fields(checksum, &["algorithm", "value"], &[]) {
            return Err(invalid_inventory_error().into());
        }
        let digits = match checksum.get("algorithm").and_then(Value::as_str) {
            Some("SHA256") => 64,
            Some("SHA512") => 128,
            _ => return Err(invalid_inventory_error().into()),
        };
        let value = bounded_string(checksum.get("value"), MAX_METADATA_BYTES)?;
        if value.len() != digits || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_inventory_error().into());
        }
    }
    Ok(())
}

fn has_fields(object: &Map<String, Value>, required: &[&str], optional: &[&str]) -> bool {
    required.iter().all(|field| object.contains_key(*field))
        && object
            .keys()
            .all(|field| required.contains(&field.as_str()) || optional.contains(&field.as_str()))
}

fn bounded_string(value: Option<&Value>, limit: usize) -> Result<&str, ApiError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(invalid_inventory_error)?;
    if value.is_empty() {
        return Err(invalid_inventory_error().into());
    }
    if value.len() > limit {
        return Err(inventory_limit_error().into());
    }
    Ok(value)
}

fn invalid_inventory_error() -> eutheto_types::ApiErrorDto {
    boundary_error(
        "license_inventory.invalid",
        "The bundled license inventory has an unsupported or invalid format.",
        None,
    )
}

fn inventory_limit_error() -> eutheto_types::ApiErrorDto {
    boundary_error(
        "license_inventory.resource_limit",
        "The bundled license inventory exceeds its resource limit.",
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_METADATA_BYTES, validate_package};
    use serde_json::{Value, json};

    fn package() -> Value {
        json!({
            "ecosystem": "cargo",
            "name": "example",
            "version": "1.0.0",
            "kind": "dependency",
            "licenseConcluded": "NOASSERTION",
            "source": "registry+https://example.invalid/index"
        })
    }

    #[test]
    fn metadata_limits_count_utf8_bytes_not_characters() {
        let mut value = package();
        value["name"] = Value::String("é".repeat(MAX_METADATA_BYTES / 2));
        assert!(validate_package(&value).is_ok());
        value["name"] = Value::String(format!("{}a", "é".repeat(MAX_METADATA_BYTES / 2)));
        assert!(matches!(
            validate_package(&value),
            Err(error) if error.code == "license_inventory.resource_limit"
        ));
    }

    #[test]
    fn rejects_null_metadata_and_unknown_checksum_shapes() {
        let mut value = package();
        value["checksum"] = json!({"algorithm": "SHA256", "value": "a".repeat(64)});
        assert!(validate_package(&value).is_ok());
        value["checksum"]["value"] = Value::String("a".repeat(128));
        assert!(validate_package(&value).is_err());
        value["checksum"] = json!({"algorithm": "SHA512", "value": "a".repeat(128)});
        assert!(validate_package(&value).is_ok());
        value["checksum"]["approved"] = Value::Bool(true);
        assert!(validate_package(&value).is_err());
        value["checksum"] = Value::Null;
        assert!(validate_package(&value).is_err());
        value = package();
        value["source"] = Value::Null;
        assert!(validate_package(&value).is_err());
    }
}
