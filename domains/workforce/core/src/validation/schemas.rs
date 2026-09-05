use super::common::{Result, invalid};
use crate::generated_workforce_pack_contract::WORKFORCE_PACK_CONTRACT_JSON;
use eutheto_domain_api::{ContractJsonLimits, DomainPackError, ValidatedContractSchema};
use eutheto_types::DomainCommandEnvelope;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One operation owns these immutable schemas and reuses them across all records and prefixes.
pub(crate) struct WorkforceSchemas {
    pub entities: ValidatedContractSchema,
    pub rules: ValidatedContractSchema,
    pub preferences: ValidatedContractSchema,
    pub locks: ValidatedContractSchema,
    commands: BTreeMap<String, ValidatedContractSchema>,
}

// Read only the implemented schema products from the trusted generated metadata document.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedSchemas {
    internal_schema: Value,
    commands: Vec<GeneratedCommandSchema>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedCommandSchema {
    id: String,
    payload_schema: Value,
}

impl WorkforceSchemas {
    pub fn load() -> Result<Self> {
        let mut generated: GeneratedSchemas =
            serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON).map_err(|_| generated_error())?;
        let mut commands = BTreeMap::new();
        for command in generated.commands {
            let schema = ValidatedContractSchema::new(command.payload_schema)
                .map_err(|_| generated_error())?;
            if commands.insert(command.id, schema).is_some() {
                return Err(generated_error());
            }
        }
        Ok(Self {
            entities: record_schema(&mut generated.internal_schema, "entities")?,
            rules: record_schema(&mut generated.internal_schema, "rules")?,
            preferences: record_schema(&mut generated.internal_schema, "preferences")?,
            locks: record_schema(&mut generated.internal_schema, "lockedAssignments")?,
            commands,
        })
    }

    pub fn validate_payload(&self, envelope: &DomainCommandEnvelope) -> Result {
        self.commands
            .get(&envelope.command_type)
            .ok_or_else(|| DomainPackError::UnknownCommand(envelope.command_type.clone()))?
            .validate(&envelope.payload, ContractJsonLimits::DEFAULT)
            .map_err(|_| invalid("/payload", "invalid Workforce command JSON shape"))
    }
}

fn record_schema(internal: &mut Value, map: &str) -> Result<ValidatedContractSchema> {
    let schema = internal
        .get_mut("properties")
        .and_then(|properties| properties.get_mut(map))
        .and_then(|schema| schema.get_mut("additionalProperties"))
        .map(Value::take)
        .ok_or_else(generated_error)?;
    ValidatedContractSchema::new(schema).map_err(|_| generated_error())
}

fn generated_error() -> DomainPackError {
    DomainPackError::CatalogMismatch("generated Workforce schemas".to_owned())
}
