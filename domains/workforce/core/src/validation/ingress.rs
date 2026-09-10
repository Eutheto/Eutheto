use super::{
    common::{Result, invalid, record, require, weight},
    context::Context,
    schemas::WorkforceSchemas,
};
use crate::model::{LockState, WorkforceDomainV1, WorkforceEntity, planning_dates};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError, ValidatedContractSchema};
use eutheto_types::{
    OperationControl, PortableJsonLimits, SCENARIO_FORMAT_VERSION, ScenarioDocument,
    is_portable_namespace, validate_document_owned_uuid_uniqueness,
    validate_nonsecret_portable_json, validate_nonsecret_portable_json_bytes,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    io::{self, Write},
};

const LIMITS: PortableJsonLimits = PortableJsonLimits {
    max_depth: ContractJsonLimits::DEFAULT.max_depth,
    max_string_bytes: ContractJsonLimits::DEFAULT.max_string_bytes,
    max_collection_items: ContractJsonLimits::DEFAULT.max_collection_items,
};
const MAX_BYTES: usize = ContractJsonLimits::DEFAULT.max_serialized_bytes;

/// Decodes a bounded current Workforce document and validates its complete structure.
/// Editable drafts need no score policy; validity does not establish feasibility or solve support.
///
/// # Errors
/// Rejects unsafe or oversized JSON, unknown formats, malformed records and invalid references.
/// Parse diagnostics never echo submitted names, notes or unknown field values.
pub fn decode_document(bytes: &[u8]) -> Result<ScenarioDocument> {
    validate_bytes(bytes)?;
    let document: ScenarioDocument = serde_json::from_slice(bytes)
        .map_err(|_| invalid("document", "invalid or unsupported scenario envelope"))?;
    validate_contents(&document, &WorkforceSchemas::load()?, None)?;
    Ok(document)
}

/// Validates existing host state and decodes its four maps once into the typed model.
/// The returned model is operation-local, not a second persisted aggregate.
///
/// # Errors
/// Rejects oversized or unsafe state, unsupported versions, malformed records, duplicate
/// owned identities, unsafe scopes and unresolved or wrong-kind references.
pub fn validate_document(document: &ScenarioDocument) -> Result<WorkforceDomainV1> {
    validate_document_with_schemas(document, &WorkforceSchemas::load()?, None)
}

pub(crate) fn validate_document_controlled(
    document: &ScenarioDocument,
    control: Option<&OperationControl>,
) -> Result<WorkforceDomainV1> {
    control.map_or(Ok(()), OperationControl::check)?;
    validate_document_with_schemas(document, &WorkforceSchemas::load()?, control)
}

pub(crate) fn validate_document_with_schemas(
    document: &ScenarioDocument,
    schemas: &WorkforceSchemas,
    control: Option<&OperationControl>,
) -> Result<WorkforceDomainV1> {
    control.map_or(Ok(()), OperationControl::check)?;
    // Bound recursive Value depth before invoking a serializer on caller-built host state.
    for value in document
        .domain
        .entities
        .values()
        .chain(document.domain.rules.values())
        .chain(document.domain.preferences.values())
        .chain(document.domain.locked_assignments.values())
        .chain(document.extensions.values())
    {
        control.map_or(Ok(()), OperationControl::check)?;
        validate_value_bounds(value, "document")?;
    }
    let mut output = BoundedJsonWriter(Vec::new());
    serde_json::to_writer(&mut output, document)
        .map_err(|_| invalid("document", "serialized document exceeds its bound"))?;
    control.map_or(Ok(()), OperationControl::check)?;
    validate_bytes(&output.0)?;
    control.map_or(Ok(()), OperationControl::check)?;
    validate_contents(document, schemas, control)
}

pub(crate) fn validate_value_bounds(value: &Value, path: &str) -> Result {
    validate_nonsecret_portable_json(value, &LIMITS)
        .map_err(|_| invalid(path, "unsafe or over-budget record"))
}

fn validate_bytes(bytes: &[u8]) -> Result {
    require(
        bytes.len() <= MAX_BYTES,
        "document",
        "serialized document exceeds its byte bound",
    )?;
    validate_nonsecret_portable_json_bytes(bytes, &LIMITS)
        .map_err(|_| invalid("document", "unsafe, malformed or over-budget JSON"))
}

fn validate_contents(
    document: &ScenarioDocument,
    schemas: &WorkforceSchemas,
    control: Option<&OperationControl>,
) -> Result<WorkforceDomainV1> {
    control.map_or(Ok(()), OperationControl::check)?;
    require(
        document.domain_pack.id.as_str() == "official.workforce",
        "domainPack.id",
        "expected the Workforce domain pack",
    )?;
    if document.domain_pack.schema_version != 1 {
        return Err(DomainPackError::UnsupportedVersion(
            document.domain_pack.schema_version,
        ));
    }
    if document.format_version != SCENARIO_FORMAT_VERSION {
        return Err(DomainPackError::UnsupportedVersion(document.format_version));
    }
    require(
        document.scenario_id.as_uuid().get_version_num() == 7,
        "scenarioId",
        "expected UUIDv7 identity",
    )?;
    planning_dates(&document.settings)?;
    require(
        document
            .extensions
            .keys()
            .all(|key| key.starts_with("nonsemantic.") && is_portable_namespace(key)),
        "extensions",
        "unknown semantic extension namespace",
    )?;
    validate_document_owned_uuid_uniqueness(document).map_err(|_| {
        invalid(
            "document",
            "owned identities must be unique across the complete document",
        )
    })?;
    let domain = WorkforceDomainV1 {
        entities: decode_map(
            &document.domain.entities,
            "entities",
            &schemas.entities,
            control,
        )?,
        rules: decode_map(&document.domain.rules, "rules", &schemas.rules, control)?,
        preferences: decode_map(
            &document.domain.preferences,
            "preferences",
            &schemas.preferences,
            control,
        )?,
        locked_assignments: decode_map(
            &document.domain.locked_assignments,
            "lockedAssignments",
            &schemas.locks,
            control,
        )?,
    };
    let context = Context::new(&domain, &document.settings, control)?;
    validate_records(&context)?;
    context.checkpoint()?;
    Ok(domain)
}

fn validate_records(context: &Context<'_>) -> Result {
    let domain = context.domain;
    let mut external_ids = BTreeSet::new();
    for (id, entity) in &domain.entities {
        context.checkpoint()?;
        record(context.entity(entity), "entities", id)?;
        if let WorkforceEntity::Person(person) = entity
            && let Some(external_id) = &person.external_id
        {
            record(
                require(
                    external_ids.insert(external_id),
                    "externalId",
                    "exact external identity is already in use",
                ),
                "entities",
                id,
            )?;
        }
    }
    // Population evaluation requires valid person sets regardless of entity UUID order.
    if let Some(policy) = context.score_policy {
        record(context.score_references(policy), "entities", policy.id)?;
    }
    for (id, rule) in &domain.rules {
        context.checkpoint()?;
        record(
            require(
                *id == rule.header().0,
                "id",
                "owned identity does not match its key",
            ),
            "rules",
            id,
        )?;
        record(context.rule(rule), "rules", id)?;
    }
    for (id, preference) in &domain.preferences {
        context.checkpoint()?;
        record(
            require(
                *id == preference.header().0,
                "id",
                "owned identity does not match its key",
            ),
            "preferences",
            id,
        )?;
        record(context.preference(preference), "preferences", id)?;
    }
    let mut locked_pairs = BTreeSet::new();
    for (id, lock) in &domain.locked_assignments {
        context.checkpoint()?;
        record(
            require(
                *id == lock.id,
                "id",
                "owned identity does not match its key",
            ),
            "lockedAssignments",
            id,
        )?;
        record(
            require(
                locked_pairs.insert((lock.person_id, lock.shift_id)),
                "shiftId",
                "duplicate assignment lock for the same person and shift",
            ),
            "lockedAssignments",
            id,
        )?;
        record(context.person(lock.person_id), "lockedAssignments", id)?;
        record(context.shift(lock.shift_id), "lockedAssignments", id)?;
        if let LockState::Soft { stability_weight } = lock.state {
            record(
                weight(stability_weight, "stabilityWeight"),
                "lockedAssignments",
                id,
            )?;
        }
    }
    Ok(())
}

fn decode_map<K: Copy + Ord + Display, T: DeserializeOwned>(
    values: &BTreeMap<K, Value>,
    map: &str,
    schema: &ValidatedContractSchema,
    control: Option<&OperationControl>,
) -> Result<BTreeMap<K, T>> {
    values
        .iter()
        .map(|(id, value)| {
            control.map_or(Ok(()), OperationControl::check)?;
            record(
                schema
                    .validate(value, ContractJsonLimits::DEFAULT)
                    .map_err(|_| invalid("record", "invalid Workforce record shape"))
                    .and_then(|()| {
                        T::deserialize(value)
                            .map_err(|_| invalid("record", "invalid Workforce record shape"))
                    }),
                map,
                id,
            )
            .map(|record| (*id, record))
        })
        .collect()
}

/// The only buffered representation here is bounded serialized input to the shared
/// streaming policy checker. Both length and requested capacity stay within the cap.
struct BoundedJsonWriter(Vec<u8>);

impl Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .0
            .len()
            .checked_add(bytes.len())
            .filter(|end| *end <= MAX_BYTES)
            .ok_or_else(|| io::Error::other("document byte limit exceeded"))?;
        if end > self.0.capacity() {
            let capacity = end.max(self.0.capacity().saturating_mul(2)).min(MAX_BYTES);
            self.0
                .try_reserve_exact(capacity - self.0.len())
                .map_err(|_| io::Error::other("document allocation failed"))?;
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
