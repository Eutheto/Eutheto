//! Scenario-owned UUID definitions shared by validation, copy, and portable boundaries.

use crate::{ScenarioDocument, ScenarioSnapshotV1};
use serde_json::Value;
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::fmt;
use uuid::Uuid;

/// An identity defined more than once within one scenario revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedIdentityError {
    /// The UUID shared by the conflicting definitions.
    pub duplicate_uuid: Uuid,
}

impl fmt::Display for OwnedIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "owned identity {} is defined more than once in one scenario revision",
            self.duplicate_uuid
        )
    }
}

impl std::error::Error for OwnedIdentityError {}

/// Recognizes a UUID-keyed definition whose child `id` names the same UUID.
/// Accepted UUID spellings compare by identity, not by their original text.
#[must_use]
pub fn self_declared_uuid(key: &str, value: &Value) -> Option<Uuid> {
    let identity = Uuid::parse_str(key).ok()?;
    let declared = Uuid::parse_str(value.get("id")?.as_str()?).ok()?;
    (identity == declared).then_some(identity)
}

/// Collects every identity owned by a scenario revision.
///
/// Includes the scenario ID, all four typed domain-map keys, and recursively
/// self-declared UUID-keyed objects whose `id` equals their containing key in
/// domain records, semantic extensions, and both nonsemantic extension layers.
#[must_use]
pub fn collect_scenario_owned_uuids(scenario: &ScenarioSnapshotV1) -> BTreeSet<Uuid> {
    collect_document_identities(
        &scenario.document,
        scenario
            .semantic_extensions
            .values()
            .chain(scenario.document.extensions.values())
            .chain(scenario.extensions.values()),
    )
}

/// Collects the scenario ID, domain-map keys, and recursively self-declared
/// identities in domain records and document extensions.
#[must_use]
pub fn collect_document_owned_uuids(document: &ScenarioDocument) -> BTreeSet<Uuid> {
    collect_document_identities(document, document.extensions.values())
}

/// Collects recursively self-declared UUID-keyed objects whose `id` equals
/// their containing key.
#[must_use]
pub fn collect_self_declared_uuids(value: &Value) -> BTreeSet<Uuid> {
    let mut identities = BTreeSet::new();
    visit_self_declared_uuids(value, &mut |identity| {
        identities.insert(identity);
        Ok::<(), Infallible>(())
    })
    .unwrap_or_else(|never| match never {});
    identities
}

/// Rejects duplicate conceptual identity definitions within one scenario revision.
///
/// # Errors
///
/// Returns the duplicate UUID across the root, domain-map keys, and recursively
/// self-declared identities, including all snapshot extension layers.
pub fn validate_scenario_owned_uuid_uniqueness(
    scenario: &ScenarioSnapshotV1,
) -> Result<(), OwnedIdentityError> {
    validate_document_identities(
        &scenario.document,
        scenario
            .semantic_extensions
            .values()
            .chain(scenario.document.extensions.values())
            .chain(scenario.extensions.values()),
    )
}

/// Rejects duplicate conceptual identity definitions within one document.
///
/// # Errors
///
/// Returns the duplicate UUID across the root, domain-map keys, and recursively
/// self-declared identities in domain records and document extensions.
pub fn validate_document_owned_uuid_uniqueness(
    document: &ScenarioDocument,
) -> Result<(), OwnedIdentityError> {
    validate_document_identities(document, document.extensions.values())
}

fn collect_document_identities<'a>(
    document: &'a ScenarioDocument,
    extensions: impl Iterator<Item = &'a Value>,
) -> BTreeSet<Uuid> {
    let mut identities = BTreeSet::new();
    visit_document_identities(document, extensions, &mut |identity| {
        identities.insert(identity);
        Ok::<(), Infallible>(())
    })
    .unwrap_or_else(|never| match never {});
    identities
}

fn validate_document_identities<'a>(
    document: &'a ScenarioDocument,
    extensions: impl Iterator<Item = &'a Value>,
) -> Result<(), OwnedIdentityError> {
    let mut seen = BTreeSet::new();
    visit_document_identities(document, extensions, &mut |identity| {
        if seen.insert(identity) {
            Ok(())
        } else {
            Err(OwnedIdentityError {
                duplicate_uuid: identity,
            })
        }
    })
}

fn visit_document_identities<'a, E>(
    document: &'a ScenarioDocument,
    extensions: impl Iterator<Item = &'a Value>,
    visit: &mut impl FnMut(Uuid) -> Result<(), E>,
) -> Result<(), E> {
    visit(document.scenario_id.as_uuid())?;
    for identity in document
        .domain
        .entities
        .keys()
        .map(|id| id.as_uuid())
        .chain(document.domain.rules.keys().map(|id| id.as_uuid()))
        .chain(document.domain.preferences.keys().map(|id| id.as_uuid()))
        .chain(
            document
                .domain
                .locked_assignments
                .keys()
                .map(|id| id.as_uuid()),
        )
    {
        visit(identity)?;
    }
    for value in document
        .domain
        .entities
        .values()
        .chain(document.domain.rules.values())
        .chain(document.domain.preferences.values())
        .chain(document.domain.locked_assignments.values())
        .chain(extensions)
    {
        visit_self_declared_uuids(value, visit)?;
    }
    Ok(())
}

fn visit_self_declared_uuids<E>(
    value: &Value,
    visit: &mut impl FnMut(Uuid) -> Result<(), E>,
) -> Result<(), E> {
    match value {
        Value::Array(values) => {
            for value in values {
                visit_self_declared_uuids(value, visit)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if let Some(identity) = self_declared_uuid(key, value) {
                    visit(identity)?;
                }
                visit_self_declared_uuids(value, visit)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}
