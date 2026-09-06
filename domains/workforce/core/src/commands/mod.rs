//! Pure Workforce mutations. Persistence, revisions and production dispatch remain host-owned.

mod dispatch;
mod effect;
mod occurrences;
mod payload;

pub use payload::*;

use crate::validation::{
    WorkforceSchemas,
    common::{Result, require},
    validate_document_with_schemas, validate_value_bounds,
};
use eutheto_domain_api::{
    DomainBatchCommand, DomainMutation, DomainPackError, MAX_DOMAIN_BATCH_COMMANDS,
};
use eutheto_types::{CancellationToken, ScenarioDocument};
use serde_json::Value;

/// Applies typed record operations atomically to an owned working document.
/// Every prefix must remain structurally valid, including references and document bounds.
/// Host fields and untouched JSON records are unchanged; ordinary inverses preserve exact
/// original record values, including valid noncanonical UUID or timestamp spellings.
///
/// # Errors
///
/// Rejects invalid envelopes/payloads, identity or kind changes, dangling references,
/// oversized state, or an inverse that cannot be replayed within the command byte bound.
/// Failure never mutates the caller's document. This function performs no persistence or solving.
pub fn apply_batch(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
) -> Result<DomainMutation> {
    apply_batch_inner(document, batch, None)
}

pub(crate) fn apply_batch_cancellable(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
    cancellation: &CancellationToken,
) -> Result<DomainMutation> {
    apply_batch_inner(document, batch, Some(cancellation))
}

fn apply_batch_inner(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
    cancellation: Option<&CancellationToken>,
) -> Result<DomainMutation> {
    check_cancellation(cancellation)?;
    validate_batch(batch)?;
    let schemas = WorkforceSchemas::load()?;
    validate_document_with_schemas(document, &schemas)?;
    check_cancellation(cancellation)?;
    let mut working = document.clone();
    let mut results = Vec::with_capacity(batch.commands.len());
    let mut changes = effect::Changes::new(batch.commands.len());
    let mut inverse = DomainBatchCommand {
        schema_version: batch.schema_version,
        pack_id: batch.pack_id.clone(),
        scenario_schema_version: batch.scenario_schema_version,
        label: batch.label.clone(),
        commands: Vec::with_capacity(batch.commands.len()),
    };
    for envelope in &batch.commands {
        check_cancellation(cancellation)?;
        schemas.validate_payload(envelope)?;
        let effect = dispatch::apply_one(&mut working, envelope, &mut changes, cancellation)?;
        validate_document_with_schemas(&working, &schemas)?;
        results.push(effect.result);
        inverse.commands.push(effect.inverse);
    }
    inverse.commands.reverse();
    // Before-records come from the bounded original document or an earlier bounded input
    // record. The constructed inverse is bounded by their sum; require replayability too.
    inverse.validate_bounds()?;
    check_cancellation(cancellation)?;
    Ok(DomainMutation {
        document: working,
        results,
        changes: changes.into_records(),
        inverse,
    })
}

fn check_cancellation(cancellation: Option<&CancellationToken>) -> Result {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(DomainPackError::Cancelled)
    } else {
        Ok(())
    }
}

fn validate_batch(batch: &DomainBatchCommand) -> Result {
    // Bound the pre-serialization depth/safety pass itself before generic envelope validation.
    require(
        batch.commands.len() <= MAX_DOMAIN_BATCH_COMMANDS,
        "/commands",
        "too many Workforce commands",
    )?;
    for envelope in &batch.commands {
        validate_value_bounds(&envelope.payload, "/commands/payload")?;
    }
    batch.validate_bounds()?;
    if batch.pack_id.as_str() != "official.workforce" {
        return Err(DomainPackError::PackUnavailable(batch.pack_id.to_string()));
    }
    if batch.scenario_schema_version != 1 {
        return Err(DomainPackError::UnsupportedVersion(
            batch.scenario_schema_version,
        ));
    }
    if let Some(label) = &batch.label {
        validate_value_bounds(&Value::String(label.clone()), "/label")?;
    }
    Ok(())
}
