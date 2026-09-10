//! Pure Workforce mutations. Persistence, revisions and production dispatch remain host-owned.

mod dispatch;
mod effect;
mod occurrences;
mod payload;
mod settings;

pub use payload::*;
pub(crate) use settings::reconcile_settings;

use crate::validation::{
    WorkforceSchemas,
    common::{Result, require},
    validate_document_with_schemas, validate_value_bounds,
};
use eutheto_domain_api::{
    DomainBatchCommand, DomainMutation, DomainPackError, MAX_DOMAIN_BATCH_COMMANDS,
    MAX_DOMAIN_MUTATION_RESULT_BYTES, bounded_json_size,
};
use eutheto_types::{
    CancellationToken, MAX_SCENARIO_DOCUMENT_BYTES, OperationControl, ScenarioDocument,
};
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
    apply_batch_controlled(
        document,
        batch,
        &OperationControl::Cancellation(cancellation.clone()),
    )
}

pub(crate) fn apply_batch_controlled(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
    control: &OperationControl,
) -> Result<DomainMutation> {
    apply_batch_inner(document, batch, Some(control))
}

fn apply_batch_inner(
    document: &ScenarioDocument,
    batch: &DomainBatchCommand,
    control: Option<&OperationControl>,
) -> Result<DomainMutation> {
    check_control(control)?;
    validate_batch(batch)?;
    let schemas = WorkforceSchemas::load()?;
    validate_document_with_schemas(document, &schemas, control)?;
    check_control(control)?;
    let mut working = document.clone();
    let mut results = Vec::with_capacity(batch.commands.len());
    let mut result_bytes = 2_usize;
    let mut changes = effect::Changes::new(batch.commands.len());
    let mut inverse = DomainBatchCommand {
        schema_version: batch.schema_version,
        pack_id: batch.pack_id.clone(),
        scenario_schema_version: batch.scenario_schema_version,
        label: batch.label.clone(),
        commands: Vec::with_capacity(batch.commands.len()),
    };
    let inverse_limit = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)
        .map_err(|_| DomainPackError::BatchInverseTooLarge)?;
    let mut inverse_bytes = bounded_json_size(&inverse, inverse_limit)
        .map_err(|_| DomainPackError::BatchInverseTooLarge)?;
    for (index, envelope) in batch.commands.iter().enumerate() {
        check_control(control)?;
        changes.begin_command(index)?;
        schemas.validate_payload(envelope)?;
        let effect = dispatch::apply_one(&mut working, envelope, &mut changes, control)?;
        validate_document_with_schemas(&working, &schemas, control)?;
        let separator = usize::from(!results.is_empty());
        let remaining = MAX_DOMAIN_MUTATION_RESULT_BYTES
            .checked_sub(result_bytes + separator)
            .ok_or(DomainPackError::MutationOutputLimit)?;
        result_bytes += separator
            + bounded_json_size(&effect.result, remaining)
                .map_err(|_| DomainPackError::MutationOutputLimit)?;
        results.push(effect.result);
        let remaining = inverse_limit
            .checked_sub(inverse_bytes + separator)
            .ok_or(DomainPackError::BatchInverseTooLarge)?;
        inverse_bytes += separator
            + bounded_json_size(&effect.inverse, remaining)
                .map_err(|_| DomainPackError::BatchInverseTooLarge)?;
        inverse.commands.push(effect.inverse);
    }
    inverse.commands.reverse();
    check_control(control)?;
    Ok(DomainMutation {
        document: working,
        results,
        changes: changes.into_records(),
        inverse,
    })
}

fn check_control(control: Option<&OperationControl>) -> Result {
    if let Some(control) = control {
        control.check()?;
    }
    Ok(())
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
