//! Current Workforce portable data, without registration or application import authority.

use crate::{
    generated_workforce_pack_contract::{WORKFORCE_PACK_CONTRACT_JSON, WORKFORCE_PACK_ID},
    validation::{common::invalid, validate_document, validate_value_bounds},
};
use eutheto_domain_api::{
    ContractJsonLimits, DomainPackError, PortableImportContext, ValidatedContractSchema,
    bounded_json_size,
};
use eutheto_types::{
    AssignmentId, EntityId, PortableDomainDocument, RuleId, ScenarioDocument, ScenarioDomain,
    SemanticCapability,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const PORTABLE_VERSION: u32 = 1;
const PORTABLE_CAPABILITY: &str = "official.workforce.portable";
const EXTERNAL_STARTING_PLAN_CAPABILITY: &str = "official.workforce.external-starting-plan";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortablePayloadV1 {
    schema_version: u32,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    entities: BTreeMap<EntityId, Value>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    rules: BTreeMap<RuleId, Value>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    preferences: BTreeMap<RuleId, Value>,
    #[serde(deserialize_with = "eutheto_types::deserialize_unique_id_map")]
    locked_assignments: BTreeMap<AssignmentId, Value>,
    extensions: BTreeMap<String, Value>,
}

/// Exports validated internal Workforce v1 as portable v1, preserving raw records and extensions.
/// Reserved later-phase semantics remain data, not a declaration of solve support.
///
/// # Errors
/// Rejects invalid, unsafe, oversized or unsupported internal documents.
pub fn export_portable(
    document: &ScenarioDocument,
) -> Result<PortableDomainDocument, DomainPackError> {
    validate_document(document)?;
    let payload = json!({
        "schemaVersion": PORTABLE_VERSION,
        "entities": &document.domain.entities,
        "rules": &document.domain.rules,
        "preferences": &document.domain.preferences,
        "lockedAssignments": &document.domain.locked_assignments,
        "extensions": &document.extensions,
    });
    validate_payload(&payload)?;
    let portable = PortableDomainDocument {
        pack_id: document.domain_pack.id.clone(),
        schema_version: PORTABLE_VERSION,
        required_capabilities: capabilities(
            document
                .domain
                .entities
                .values()
                .any(is_external_starting_plan),
        ),
        payload,
    };
    bounded_json_size(&portable, ContractJsonLimits::DEFAULT.max_serialized_bytes)?;
    Ok(portable)
}

/// Imports portable v1 into an explicit host shell, replacing only its domain and extensions.
/// No historical Workforce migration exists; host identity, versions, metadata and settings stay exact.
///
/// # Errors
/// Rejects unknown versions/semantic requirements, unsafe or malformed payloads, duplicate identities,
/// invalid references and incompatibility with the supplied host settings or pack identity.
pub fn import_portable(
    document: &PortableDomainDocument,
    context: &PortableImportContext,
) -> Result<ScenarioDocument, DomainPackError> {
    if document.pack_id.as_str() != WORKFORCE_PACK_ID {
        return Err(invalid(
            "/portable/packId",
            "expected the Workforce domain pack",
        ));
    }
    if document.schema_version != PORTABLE_VERSION {
        return Err(DomainPackError::UnsupportedVersion(document.schema_version));
    }
    let has_external = document
        .payload
        .get("entities")
        .and_then(Value::as_object)
        .is_some_and(|entities| entities.values().any(is_external_starting_plan));
    if document.required_capabilities != capabilities(has_external) {
        return Err(invalid(
            "/portable",
            "unknown or missing semantic capability",
        ));
    }
    validate_payload(&document.payload)?;
    bounded_json_size(document, ContractJsonLimits::DEFAULT.max_serialized_bytes)?;
    let payload = PortablePayloadV1::deserialize(&document.payload)
        .map_err(|_| invalid("/portable", "invalid Workforce portable payload"))?;
    if payload.schema_version != document.schema_version {
        return Err(DomainPackError::UnsupportedVersion(payload.schema_version));
    }
    let shell = &context.scenario_shell;
    // Bound caller-owned host strings before retaining them; old domain data is not imported.
    bounded_json_size(
        &(&shell.metadata, &shell.settings),
        ContractJsonLimits::DEFAULT.max_serialized_bytes,
    )?;
    let result = ScenarioDocument {
        format: shell.format,
        format_version: shell.format_version,
        scenario_id: shell.scenario_id,
        domain_pack: shell.domain_pack.clone(),
        metadata: shell.metadata.clone(),
        settings: shell.settings.clone(),
        domain: ScenarioDomain {
            entities: payload.entities,
            rules: payload.rules,
            preferences: payload.preferences,
            locked_assignments: payload.locked_assignments,
        },
        extensions: payload.extensions,
    };
    validate_document(&result)?;
    Ok(result)
}

pub(crate) fn capabilities(external_starting_plan: bool) -> BTreeSet<SemanticCapability> {
    let mut capabilities = BTreeSet::from([SemanticCapability {
        id: PORTABLE_CAPABILITY.to_owned(),
        version: PORTABLE_VERSION,
    }]);
    if external_starting_plan {
        capabilities.insert(SemanticCapability {
            id: EXTERNAL_STARTING_PLAN_CAPABILITY.to_owned(),
            version: 1,
        });
    }
    capabilities
}

fn is_external_starting_plan(record: &Value) -> bool {
    record.get("kind").and_then(Value::as_str) == Some("externalStartingPlan")
}

fn validate_payload(payload: &Value) -> Result<(), DomainPackError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GeneratedPortableSchema {
        portable_schema: Value,
    }
    // Caller-built Values must be depth-bounded before the evaluator serializes or clones them.
    validate_value_bounds(payload, "/portable")?;
    let generated: GeneratedPortableSchema = serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON)
        .map_err(|_| {
            DomainPackError::CatalogMismatch("generated Workforce portable schema".to_owned())
        })?;
    ValidatedContractSchema::new(generated.portable_schema)?
        .validate(payload, ContractJsonLimits::DEFAULT)
        .map_err(|_| {
            invalid(
                "/portable",
                "unsafe, over-budget or invalid Workforce portable shape",
            )
        })
}
