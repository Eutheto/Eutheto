//! Pure scenario command application.
//!
//! This crate deliberately owns no persistence. Callers pass an immutable
//! document and revision and receive a replacement document plus journal-ready
//! metadata. The input is never modified, including on batch failure.
mod generated_official_test_pack_contract;
mod official_test_pack;

use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainMutation,
    DomainPack, DomainPackError, DomainPackRegistry, MAX_DOMAIN_MUTATION_CHANGE_BYTES,
    MAX_DOMAIN_MUTATION_CHANGES, MAX_DOMAIN_MUTATION_RESULT_BYTES, RegisteredCommand,
    bounded_json_size,
};
use eutheto_types::{
    AddEntity, AddRule, AssignmentId, CancellationToken, Change, ChangeKind, ChangeSet,
    CommandBatch, CommandEnvelope, CommandResult, DomainCommandEnvelope, EntityId, LockAssignment,
    MAX_SCENARIO_DOCUMENT_BYTES, PortableJsonLimits, Revision, RuleId, ScenarioCommand,
    ScenarioDocument, SetPreference, UnlockAssignment, UpdateEntity, UpdateRule, ValidationDelta,
    ValidationIssue, validate_nonsecret_portable_json,
};
/// Generated authoritative metadata for the synthetic conformance pack.
pub use generated_official_test_pack_contract::{
    OFFICIAL_TEST_COMMAND_IDS, OFFICIAL_TEST_PACK_CONTRACT_JSON, OFFICIAL_TEST_PACK_ID,
    OFFICIAL_TEST_PACK_VERSION,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Protects pure application from adversarially deep nested batches.
pub const MAX_BATCH_DEPTH: usize = 8;
/// Bounds total leaf commands in one atomic batch.
pub const MAX_BATCH_COMMANDS: usize = 1_000;
/// Maximum compact JSON bytes in the complete generic inverse, including nested batches.
pub const MAX_COMMAND_INVERSE_BYTES: usize = 64 * 1024 * 1024;

pub const CODE_BATCH_DEPTH_EXCEEDED: &str = "command.batch_depth_exceeded";
pub const CODE_BATCH_TOO_LARGE: &str = "command.batch_too_large";
pub const CODE_DUPLICATE_ENTITY: &str = "command.duplicate_entity";
pub const CODE_DUPLICATE_LOCK: &str = "command.duplicate_assignment_lock";
pub const CODE_DUPLICATE_RULE: &str = "command.duplicate_rule";
pub const CODE_EMPTY_BATCH: &str = "command.empty_batch";
pub const CODE_INVALID_RECORD_SHAPE: &str = "command.invalid_record_shape";
pub const CODE_MISSING_ENTITY: &str = "command.missing_entity";
pub const CODE_MISSING_LOCK: &str = "command.missing_assignment_lock";
pub const CODE_MISSING_PREFERENCE: &str = "command.missing_preference";
pub const CODE_MISSING_RULE: &str = "command.missing_rule";
pub const CODE_RECORD_ID_MISMATCH: &str = "command.record_id_mismatch";
pub const CODE_PROHIBITED_DATA: &str = "command.prohibited_data";

/// Successful, side-effect-free application of one envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedCommand {
    /// Complete replacement document.
    pub document: ScenarioDocument,
    /// Revision, changes, validation delta, and generated inverse.
    pub result: CommandResult,
    /// Deterministic journal/UI summary.
    pub summary: String,
    /// Stable command type metadata.
    pub command_type: String,
}

/// Stable failure returned by pure application.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CommandError {
    /// Cancellation observed before the pure mutation completed.
    #[error("command application was cancelled")]
    Cancelled,
    /// The command targets a different scenario.
    #[error(
        "command scenario {command_scenario_id} does not match document scenario {document_scenario_id}"
    )]
    ScenarioMismatch {
        command_scenario_id: String,
        document_scenario_id: String,
    },
    /// Optimistic revision check failed.
    #[error("revision conflict: expected {expected}, actual {actual}")]
    Conflict { expected: u64, actual: u64 },
    /// A stable structural or command validation rule failed.
    #[error("validation {code} at {path}: {message}")]
    Validation {
        code: &'static str,
        path: String,
        message: String,
    },
    /// No registered pack can apply the command.
    #[error("unsupported command {command_type} for domain pack {pack_id}")]
    Unsupported {
        pack_id: String,
        command_type: String,
    },
    /// A domain command payload did not match its declared command type.
    #[error("invalid payload for domain command {command_type}: {message}")]
    InvalidDomainPayload {
        command_type: String,
        message: String,
    },
    /// A coalesced domain batch failed without identifying an individual command.
    #[error("invalid domain batch payload at {path}: {message}")]
    InvalidDomainBatchPayload { path: String, message: String },
    /// The revision cannot be incremented.
    #[error("revision overflow at {revision}")]
    RevisionOverflow { revision: u64 },
}

impl CommandError {
    /// Machine-stable validation/dispatch code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "command.cancelled",
            Self::ScenarioMismatch { .. } => "command.scenario_mismatch",
            Self::Conflict { .. } => "command.revision_conflict",
            Self::Validation { code, .. } => code,
            Self::Unsupported { .. } => "command.unsupported",
            Self::InvalidDomainPayload { .. } | Self::InvalidDomainBatchPayload { .. } => {
                "command.invalid_domain_payload"
            }
            Self::RevisionOverflow { .. } => "command.revision_overflow",
        }
    }
}

/// Effect produced while applying one non-batch command to a private working copy.
#[derive(Clone, Debug, PartialEq)]
struct PackCommandEffect {
    changes: Vec<Change>,
    inverse: ScenarioCommand,
    summary: String,
    command_type: String,
}

struct ApplyContext<'a> {
    pack: &'a dyn DomainPack,
    registry: &'a DomainPackRegistry,
    cancellation: &'a CancellationToken,
    leaf_count: usize,
    change_count: usize,
    change_bytes: usize,
    result_bytes: usize,
    inverse_bytes: usize,
}

impl ApplyContext<'_> {
    fn check_cancelled(&self) -> Result<(), CommandError> {
        if self.cancellation.is_cancelled() {
            Err(CommandError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn charge_leaves(&mut self, count: usize) -> Result<(), CommandError> {
        self.leaf_count = self.leaf_count.saturating_add(count);
        if self.leaf_count > MAX_BATCH_COMMANDS {
            return Err(validation_error(
                CODE_BATCH_TOO_LARGE,
                "/command/commands",
                format!("batch may contain at most {MAX_BATCH_COMMANDS} commands"),
            ));
        }
        Ok(())
    }

    fn charge_inverse<T: Serialize>(&mut self, value: &T) -> Result<(), CommandError> {
        let remaining = MAX_COMMAND_INVERSE_BYTES.saturating_sub(self.inverse_bytes);
        self.inverse_bytes +=
            bounded_json_size(value, remaining).map_err(|error| domain_pack_error(&error))?;
        Ok(())
    }

    fn charge_output(
        &mut self,
        change_count: usize,
        change_bytes: usize,
        result_bytes: usize,
    ) -> Result<(), CommandError> {
        if change_count != 0 {
            self.change_bytes = self.change_bytes.saturating_add(
                change_bytes.saturating_sub(2) + usize::from(self.change_count != 0),
            );
        }
        if result_bytes > 2 {
            self.result_bytes = self
                .result_bytes
                .saturating_add(result_bytes - 2 + usize::from(self.result_bytes > 2));
        }
        self.change_count = self.change_count.saturating_add(change_count);
        if self.change_count > MAX_DOMAIN_MUTATION_CHANGES
            || self.change_bytes > MAX_DOMAIN_MUTATION_CHANGE_BYTES
            || self.result_bytes > MAX_DOMAIN_MUTATION_RESULT_BYTES
        {
            return Err(domain_pack_error(&DomainPackError::MutationOutputLimit));
        }
        self.check_cancelled()
    }
}

/// Synthetic Phase-02 conformance pack. It is never a production domain or authority.
#[derive(Clone, Copy, Debug, Default)]
pub struct OfficialTestPack;

/// Builds the validated deterministic compiled-in pack registry.
///
/// # Errors
/// Returns a descriptor/catalog contract error if generated static metadata has drifted.
pub fn official_registry() -> Result<DomainPackRegistry, DomainPackError> {
    DomainPackRegistry::builder()
        .register(OfficialTestPack)
        .build()
}

/// Apply using the validated compiled-in Phase-02 pack registry.
///
/// The function performs no I/O, does not mutate `document`, increments the
/// revision exactly once (including for a batch), and returns no document on
/// failure.
///
/// # Errors
///
/// Propagates typed command precondition, pack-dispatch, validation, and
/// revision-overflow failures.
pub fn apply_command(
    document: &ScenarioDocument,
    current_revision: Revision,
    envelope: &CommandEnvelope,
) -> Result<AppliedCommand, CommandError> {
    let registry = official_registry().map_err(|error| domain_pack_error(&error))?;
    apply_command_with_registry(
        document,
        current_revision,
        envelope,
        &registry,
        &CancellationToken::new(),
    )
}

/// Apply using an already validated Phase-02 compiled-in pack registry.
///
/// Application services should retain one registry and pass it here rather
/// than rebuilding static metadata for every command.
///
/// # Errors
///
/// Returns the same typed failures as [`apply_command`].
pub fn apply_command_with_registry(
    document: &ScenarioDocument,
    current_revision: Revision,
    envelope: &CommandEnvelope,
    registry: &DomainPackRegistry,
    cancellation: &CancellationToken,
) -> Result<AppliedCommand, CommandError> {
    if cancellation.is_cancelled() {
        return Err(CommandError::Cancelled);
    }
    validate_safe_serialized(&envelope.command, "/command")?;
    if envelope.scenario_id != document.scenario_id {
        return Err(CommandError::ScenarioMismatch {
            command_scenario_id: envelope.scenario_id.to_string(),
            document_scenario_id: document.scenario_id.to_string(),
        });
    }
    if envelope.expected_revision != current_revision {
        return Err(CommandError::Conflict {
            expected: envelope.expected_revision.value(),
            actual: current_revision.value(),
        });
    }

    let pack =
        registry
            .require(&document.domain_pack.id)
            .map_err(|_| CommandError::Unsupported {
                pack_id: document.domain_pack.id.to_string(),
                command_type: command_type(&envelope.command),
            })?;
    let supports_schema = registry.descriptors().any(|descriptor| {
        descriptor.id == document.domain_pack.id
            && descriptor
                .scenario_versions
                .supports(document.domain_pack.schema_version)
    });
    if !supports_schema {
        return Err(CommandError::Unsupported {
            pack_id: document.domain_pack.id.to_string(),
            command_type: command_type(&envelope.command),
        });
    }

    validate_document_shape(document)?;
    let before_issues = pack.validate_fast(document).issues;
    let mut working = document.clone();
    let mut context = ApplyContext {
        pack,
        registry,
        cancellation,
        leaf_count: 0,
        change_count: 0,
        change_bytes: 2,
        result_bytes: 2,
        inverse_bytes: 0,
    };
    let effect = apply_nested(&mut working, &envelope.command, &mut context, 0)?;
    context.check_cancelled()?;
    bounded_json_size(&effect.inverse, MAX_COMMAND_INVERSE_BYTES)
        .map_err(|error| domain_pack_error(&error))?;
    context.check_cancelled()?;
    validate_safe_serialized(&effect.inverse, "/inverse")?;
    context.check_cancelled()?;
    validate_document_shape(&working)?;
    let after_issues = pack.validate_fast(&working).issues;
    let validation_delta = validation_delta(&before_issues, &after_issues);
    context.check_cancelled()?;
    let new_revision =
        current_revision
            .checked_next()
            .map_err(|_| CommandError::RevisionOverflow {
                revision: current_revision.value(),
            })?;

    Ok(AppliedCommand {
        document: working,
        result: CommandResult {
            new_revision,
            change_set: ChangeSet {
                changes: effect.changes,
            },
            validation_delta,
            inverse: Some(effect.inverse),
        },
        summary: effect.summary,
        command_type: effect.command_type,
    })
}

fn apply_nested(
    document: &mut ScenarioDocument,
    command: &ScenarioCommand,
    context: &mut ApplyContext<'_>,
    depth: usize,
) -> Result<PackCommandEffect, CommandError> {
    context.check_cancelled()?;
    if let ScenarioCommand::ApplyBatch(batch) = command {
        return apply_batch(document, batch, context, depth);
    }
    context.charge_leaves(1)?;
    match command {
        ScenarioCommand::ApplyDomainCommand(envelope) => {
            let mut run = apply_domain_run(document, std::iter::once(envelope), context)?;
            let inverse = run
                .inverses
                .pop()
                .ok_or_else(|| mutation_error("missing inverse"))?;
            Ok(PackCommandEffect {
                changes: run.changes,
                inverse,
                summary: format!("Apply {} domain command", envelope.command_type),
                command_type: format!("domain.{}", envelope.command_type),
            })
        }
        _ if document.domain_pack.id.as_str() == OFFICIAL_TEST_PACK_ID => {
            let effect = apply_official_test_leaf(document, command)?;
            let bytes = bounded_json_size(&effect.changes, MAX_DOMAIN_MUTATION_CHANGE_BYTES)
                .map_err(|error| domain_pack_error(&error))?;
            context.charge_output(effect.changes.len(), bytes, 2)?;
            context.charge_inverse(&effect.inverse)?;
            Ok(effect)
        }
        _ => Err(CommandError::Unsupported {
            pack_id: document.domain_pack.id.to_string(),
            command_type: command_type(command),
        }),
    }
}

fn apply_batch(
    document: &mut ScenarioDocument,
    batch: &CommandBatch,
    context: &mut ApplyContext<'_>,
    depth: usize,
) -> Result<PackCommandEffect, CommandError> {
    if batch.commands.is_empty() {
        return Err(validation_error(
            CODE_EMPTY_BATCH,
            "/command/commands",
            "a batch must contain at least one command",
        ));
    }
    if depth >= MAX_BATCH_DEPTH {
        return Err(validation_error(
            CODE_BATCH_DEPTH_EXCEEDED,
            "/command/commands",
            format!("batch nesting may not exceed {MAX_BATCH_DEPTH}"),
        ));
    }

    // Charge each existing batch frame once, not its growing inverse prefix.
    context.charge_inverse(&ScenarioCommand::ApplyBatch(CommandBatch {
        label: batch.label.clone(),
        commands: Vec::new(),
    }))?;
    context.inverse_bytes = context
        .inverse_bytes
        .saturating_add(batch.commands.len().saturating_sub(1));
    if context.inverse_bytes > MAX_COMMAND_INVERSE_BYTES {
        return Err(domain_pack_error(&DomainPackError::MutationOutputLimit));
    }
    let mut changes = Vec::new();
    let mut inverses = Vec::with_capacity(batch.commands.len());
    let mut children = batch.commands.as_slice();
    while let Some(child) = children.first() {
        context.check_cancelled()?;
        if matches!(child, ScenarioCommand::ApplyDomainCommand(_)) {
            let length = children
                .iter()
                .take_while(|child| matches!(child, ScenarioCommand::ApplyDomainCommand(_)))
                .count();
            context.charge_leaves(length)?;
            let run = apply_domain_run(
                document,
                children[..length].iter().filter_map(|child| match child {
                    ScenarioCommand::ApplyDomainCommand(envelope) => Some(envelope),
                    _ => None,
                }),
                context,
            )?;
            changes.extend(run.changes);
            inverses.extend(run.inverses);
            children = &children[length..];
        } else {
            let effect = apply_nested(document, child, context, depth + 1)?;
            changes.extend(effect.changes);
            inverses.push(effect.inverse);
            children = &children[1..];
        }
    }
    inverses.reverse();
    let count = batch.commands.len();
    let summary = match &batch.label {
        Some(label) => format!("{label} ({count} commands)"),
        None => format!("Apply batch ({count} commands)"),
    };
    Ok(PackCommandEffect {
        changes,
        inverse: ScenarioCommand::ApplyBatch(CommandBatch {
            label: batch.label.clone(),
            commands: inverses,
        }),
        summary,
        command_type: "apply_batch".to_owned(),
    })
}

fn apply_official_test_leaf(
    document: &mut ScenarioDocument,
    command: &ScenarioCommand,
) -> Result<PackCommandEffect, CommandError> {
    match command {
        ScenarioCommand::AddEntity(value) => add_entity(document, value),
        ScenarioCommand::UpdateEntity(value) => update_entity(document, value),
        ScenarioCommand::RemoveEntity(value) => remove_entity(document, value),
        ScenarioCommand::AddRule(value) => add_rule(document, value),
        ScenarioCommand::UpdateRule(value) => update_rule(document, value),
        ScenarioCommand::RemoveRule(value) => remove_rule(document, value),
        ScenarioCommand::SetPreference(value) => set_preference(document, value),
        ScenarioCommand::LockAssignment(value) => lock_assignment(document, value),
        ScenarioCommand::UnlockAssignment(value) => unlock_assignment(document, value),
        ScenarioCommand::ApplyDomainCommand(_) | ScenarioCommand::ApplyBatch(_) => {
            Err(validation_error(
                CODE_INVALID_RECORD_SHAPE,
                "/command",
                "the command engine, not the legacy leaf applicator, must dispatch this command",
            ))
        }
    }
}

fn add_entity(
    document: &mut ScenarioDocument,
    command: &AddEntity,
) -> Result<PackCommandEffect, CommandError> {
    validate_record(&command.value, &command.entity_id, "/command/value")?;
    if document.domain.entities.contains_key(&command.entity_id) {
        return Err(validation_error(
            CODE_DUPLICATE_ENTITY,
            entity_path(&command.entity_id),
            "entity already exists",
        ));
    }
    document
        .domain
        .entities
        .insert(command.entity_id, command.value.clone());
    Ok(effect(
        ChangeKind::Added,
        entity_path(&command.entity_id),
        None,
        Some(command.value.clone()),
        ScenarioCommand::RemoveEntity(eutheto_types::RemoveEntity {
            entity_id: command.entity_id,
        }),
        format!("Add entity {}", command.entity_id),
        "add_entity",
    ))
}

fn update_entity(
    document: &mut ScenarioDocument,
    command: &UpdateEntity,
) -> Result<PackCommandEffect, CommandError> {
    validate_record(&command.value, &command.entity_id, "/command/value")?;
    let Some(previous) = document
        .domain
        .entities
        .insert(command.entity_id, command.value.clone())
    else {
        return Err(validation_error(
            CODE_MISSING_ENTITY,
            entity_path(&command.entity_id),
            "entity does not exist",
        ));
    };
    Ok(effect(
        ChangeKind::Updated,
        entity_path(&command.entity_id),
        Some(previous.clone()),
        Some(command.value.clone()),
        ScenarioCommand::UpdateEntity(UpdateEntity {
            entity_id: command.entity_id,
            value: previous,
        }),
        format!("Update entity {}", command.entity_id),
        "update_entity",
    ))
}

fn remove_entity(
    document: &mut ScenarioDocument,
    command: &eutheto_types::RemoveEntity,
) -> Result<PackCommandEffect, CommandError> {
    let Some(previous) = document.domain.entities.remove(&command.entity_id) else {
        return Err(validation_error(
            CODE_MISSING_ENTITY,
            entity_path(&command.entity_id),
            "entity does not exist",
        ));
    };
    Ok(effect(
        ChangeKind::Removed,
        entity_path(&command.entity_id),
        Some(previous.clone()),
        None,
        ScenarioCommand::AddEntity(AddEntity {
            entity_id: command.entity_id,
            value: previous,
        }),
        format!("Remove entity {}", command.entity_id),
        "remove_entity",
    ))
}

fn add_rule(
    document: &mut ScenarioDocument,
    command: &AddRule,
) -> Result<PackCommandEffect, CommandError> {
    validate_record(&command.value, &command.rule_id, "/command/value")?;
    if document.domain.rules.contains_key(&command.rule_id) {
        return Err(validation_error(
            CODE_DUPLICATE_RULE,
            rule_path(&command.rule_id),
            "rule already exists",
        ));
    }
    document
        .domain
        .rules
        .insert(command.rule_id, command.value.clone());
    Ok(effect(
        ChangeKind::Added,
        rule_path(&command.rule_id),
        None,
        Some(command.value.clone()),
        ScenarioCommand::RemoveRule(eutheto_types::RemoveRule {
            rule_id: command.rule_id,
        }),
        format!("Add rule {}", command.rule_id),
        "add_rule",
    ))
}

fn update_rule(
    document: &mut ScenarioDocument,
    command: &UpdateRule,
) -> Result<PackCommandEffect, CommandError> {
    validate_record(&command.value, &command.rule_id, "/command/value")?;
    let Some(previous) = document
        .domain
        .rules
        .insert(command.rule_id, command.value.clone())
    else {
        return Err(validation_error(
            CODE_MISSING_RULE,
            rule_path(&command.rule_id),
            "rule does not exist",
        ));
    };
    Ok(effect(
        ChangeKind::Updated,
        rule_path(&command.rule_id),
        Some(previous.clone()),
        Some(command.value.clone()),
        ScenarioCommand::UpdateRule(UpdateRule {
            rule_id: command.rule_id,
            value: previous,
        }),
        format!("Update rule {}", command.rule_id),
        "update_rule",
    ))
}

fn remove_rule(
    document: &mut ScenarioDocument,
    command: &eutheto_types::RemoveRule,
) -> Result<PackCommandEffect, CommandError> {
    let Some(previous) = document.domain.rules.remove(&command.rule_id) else {
        return Err(validation_error(
            CODE_MISSING_RULE,
            rule_path(&command.rule_id),
            "rule does not exist",
        ));
    };
    Ok(effect(
        ChangeKind::Removed,
        rule_path(&command.rule_id),
        Some(previous.clone()),
        None,
        ScenarioCommand::AddRule(AddRule {
            rule_id: command.rule_id,
            value: previous,
        }),
        format!("Remove rule {}", command.rule_id),
        "remove_rule",
    ))
}

fn set_preference(
    document: &mut ScenarioDocument,
    command: &SetPreference,
) -> Result<PackCommandEffect, CommandError> {
    let path = preference_path(&command.preference_id);
    let previous = match &command.value {
        Some(value) => {
            validate_record(value, &command.preference_id, "/command/value")?;
            document
                .domain
                .preferences
                .insert(command.preference_id, value.clone())
        }
        None => document.domain.preferences.remove(&command.preference_id),
    };
    if command.value.is_none() && previous.is_none() {
        return Err(validation_error(
            CODE_MISSING_PREFERENCE,
            path,
            "preference does not exist",
        ));
    }
    let kind = match (&previous, &command.value) {
        (None, Some(_)) => ChangeKind::Added,
        (Some(_), Some(_)) => ChangeKind::Updated,
        _ => ChangeKind::Removed,
    };
    let action = if command.value.is_some() {
        "Set"
    } else {
        "Clear"
    };
    Ok(effect(
        kind,
        preference_path(&command.preference_id),
        previous.clone(),
        command.value.clone(),
        ScenarioCommand::SetPreference(SetPreference {
            preference_id: command.preference_id,
            value: previous,
        }),
        format!("{action} preference {}", command.preference_id),
        "set_preference",
    ))
}

fn lock_assignment(
    document: &mut ScenarioDocument,
    command: &LockAssignment,
) -> Result<PackCommandEffect, CommandError> {
    validate_record(&command.value, &command.assignment_id, "/command/value")?;
    if document
        .domain
        .locked_assignments
        .contains_key(&command.assignment_id)
    {
        return Err(validation_error(
            CODE_DUPLICATE_LOCK,
            lock_path(&command.assignment_id),
            "assignment is already locked",
        ));
    }
    document
        .domain
        .locked_assignments
        .insert(command.assignment_id, command.value.clone());
    Ok(effect(
        ChangeKind::Locked,
        lock_path(&command.assignment_id),
        None,
        Some(command.value.clone()),
        ScenarioCommand::UnlockAssignment(UnlockAssignment {
            assignment_id: command.assignment_id,
        }),
        format!("Lock assignment {}", command.assignment_id),
        "lock_assignment",
    ))
}

fn unlock_assignment(
    document: &mut ScenarioDocument,
    command: &UnlockAssignment,
) -> Result<PackCommandEffect, CommandError> {
    let Some(previous) = document
        .domain
        .locked_assignments
        .remove(&command.assignment_id)
    else {
        return Err(validation_error(
            CODE_MISSING_LOCK,
            lock_path(&command.assignment_id),
            "assignment is not locked",
        ));
    };
    Ok(effect(
        ChangeKind::Unlocked,
        lock_path(&command.assignment_id),
        Some(previous.clone()),
        None,
        ScenarioCommand::LockAssignment(LockAssignment {
            assignment_id: command.assignment_id,
            value: previous,
        }),
        format!("Unlock assignment {}", command.assignment_id),
        "unlock_assignment",
    ))
}

struct DomainRunEffect {
    changes: Vec<Change>,
    /// Input correspondence order; the containing generic batch reverses its children.
    inverses: Vec<ScenarioCommand>,
}

fn apply_domain_run<'a>(
    document: &mut ScenarioDocument,
    envelopes: impl Iterator<Item = &'a DomainCommandEnvelope>,
    context: &mut ApplyContext<'_>,
) -> Result<DomainRunEffect, CommandError> {
    let mut batch = DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: document.domain_pack.id.clone(),
        scenario_schema_version: document.domain_pack.schema_version,
        label: None,
        commands: Vec::new(),
    };
    let limit = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)
        .map_err(|_| mutation_error("scenario byte limit is unavailable"))?;
    let framing = bounded_json_size(&batch, limit).map_err(|error| domain_pack_error(&error))?;
    // Aggregate items are nodes minus the root, so the shared node ceiling also
    // enforces the item ceiling. Count the serialized header once, including [].
    let frame_nodes = json_node_count(
        &serde_json::to_value(&batch)
            .map_err(|_| mutation_error("domain batch frame cannot be serialized"))?,
    );
    let node_limit = ContractJsonLimits::DEFAULT.max_collection_items;
    let mut nodes = frame_nodes;
    let mut bytes = framing;
    let mut registered = Vec::new();
    let mut effect = DomainRunEffect {
        changes: Vec::new(),
        inverses: Vec::new(),
    };
    for envelope in envelopes {
        context.check_cancelled()?;
        let input = (|| {
            let command = context
                .registry
                .command(&batch.pack_id, &envelope.command_type)
                .map_err(|error| domain_command_error(document, envelope, error))?;
            command
                .validate_payload(&envelope.payload)
                .map_err(|error| domain_command_error(document, envelope, error))?;
            // Measure every envelope once; only fixed framing and commas are added later.
            let size = bounded_json_size(envelope, limit.saturating_sub(framing))
                .map_err(|error| domain_command_error(document, envelope, error))?;
            // An envelope adds an object and commandType string around its payload.
            // Schema validation already bounds depth; this traversal visits each node once.
            let envelope_nodes = 2 + json_node_count(&envelope.payload);
            if frame_nodes + envelope_nodes > node_limit {
                return Err(domain_command_error(
                    document,
                    envelope,
                    DomainPackError::InvalidPayload {
                        path: "/commands".to_owned(),
                        message: "domain command exceeds the batch JSON node limit".to_owned(),
                    },
                ));
            }
            Ok::<_, CommandError>((command, size, envelope_nodes))
        })();
        let (command, size, envelope_nodes) = match input {
            Ok(input) => input,
            Err(error) => {
                // A later schema/identity failure must not mask an earlier semantic failure.
                if !batch.commands.is_empty() {
                    apply_domain_chunk(document, batch, registered, context, &mut effect)?;
                }
                context.check_cancelled()?;
                return Err(error);
            }
        };
        let separator = usize::from(!batch.commands.is_empty());
        if bytes + separator + size > limit || nodes + envelope_nodes > node_limit {
            let next = DomainBatchCommand {
                schema_version: batch.schema_version,
                pack_id: batch.pack_id.clone(),
                scenario_schema_version: batch.scenario_schema_version,
                label: None,
                commands: Vec::new(),
            };
            let chunk = std::mem::replace(&mut batch, next);
            apply_domain_chunk(
                document,
                chunk,
                std::mem::take(&mut registered),
                context,
                &mut effect,
            )?;
            bytes = framing;
            nodes = frame_nodes;
        }
        bytes += usize::from(!batch.commands.is_empty()) + size;
        nodes += envelope_nodes;
        batch.commands.push(envelope.clone());
        registered.push(command);
    }
    if !batch.commands.is_empty() {
        apply_domain_chunk(document, batch, registered, context, &mut effect)?;
    }
    Ok(effect)
}

fn json_node_count(value: &Value) -> usize {
    1 + match value {
        Value::Array(values) => values.iter().map(json_node_count).sum::<usize>(),
        Value::Object(values) => values.values().map(json_node_count).sum::<usize>(),
        _ => 0,
    }
}

fn apply_domain_chunk(
    document: &mut ScenarioDocument,
    mut batch: DomainBatchCommand,
    mut registered: Vec<RegisteredCommand<'_>>,
    context: &mut ApplyContext<'_>,
    effect: &mut DomainRunEffect,
) -> Result<(), CommandError> {
    context.check_cancelled()?;
    let mutation = context
        .pack
        .apply_batch(document, &batch, context.cancellation);
    context.check_cancelled()?;
    let mutation = match mutation {
        Ok(mutation) => mutation,
        Err(DomainPackError::BatchInverseTooLarge) if batch.commands.len() > 1 => {
            // Pure packing retry only. split_off moves the owned payloads; each descent
            // halves a <=1000-leaf chunk, bounding recursion to ten levels.
            let midpoint = batch.commands.len() / 2;
            let right = DomainBatchCommand {
                schema_version: batch.schema_version,
                pack_id: batch.pack_id.clone(),
                scenario_schema_version: batch.scenario_schema_version,
                label: None,
                commands: batch.commands.split_off(midpoint),
            };
            let right_registered = registered.split_off(midpoint);
            apply_domain_chunk(document, batch, registered, context, effect)?;
            return apply_domain_chunk(document, right, right_registered, context, effect);
        }
        Err(error) if batch.commands.len() == 1 => {
            return Err(domain_command_error(document, &batch.commands[0], error));
        }
        // The pack error does not identify an input leaf. Never blame the first one.
        Err(DomainPackError::InvalidPayload { path, message }) => {
            return Err(CommandError::InvalidDomainBatchPayload { path, message });
        }
        Err(error) => return Err(domain_pack_error(&error)),
    };
    let (change_bytes, result_bytes) =
        validate_domain_mutation(document, &batch, &mutation, &registered, context)?;
    let changes = mutation
        .changes
        .into_iter()
        .map(|change| {
            context.check_cancelled()?;
            domain_change(change.value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let translated_bytes = bounded_json_size(&changes, MAX_DOMAIN_MUTATION_CHANGE_BYTES)
        .map_err(|error| domain_pack_error(&error))?;
    context.charge_output(
        changes.len(),
        change_bytes.max(translated_bytes),
        result_bytes,
    )?;
    for inverse in mutation.inverse.commands.into_iter().rev() {
        context.check_cancelled()?;
        let inverse = ScenarioCommand::ApplyDomainCommand(inverse);
        context.charge_inverse(&inverse)?;
        effect.inverses.push(inverse);
    }
    context.check_cancelled()?;
    effect.changes.extend(changes);
    *document = mutation.document;
    Ok(())
}

fn domain_command_error(
    document: &ScenarioDocument,
    envelope: &DomainCommandEnvelope,
    error: DomainPackError,
) -> CommandError {
    match error {
        DomainPackError::PackUnavailable(_) | DomainPackError::UnknownCommand(_) => {
            CommandError::Unsupported {
                pack_id: document.domain_pack.id.to_string(),
                command_type: envelope.command_type.clone(),
            }
        }
        DomainPackError::InvalidPayload { message, .. } => CommandError::InvalidDomainPayload {
            command_type: envelope.command_type.clone(),
            message,
        },
        other => domain_pack_error(&other),
    }
}

fn validate_domain_mutation(
    original: &ScenarioDocument,
    batch: &DomainBatchCommand,
    mutation: &DomainMutation,
    commands: &[RegisteredCommand<'_>],
    context: &ApplyContext<'_>,
) -> Result<(usize, usize), CommandError> {
    context.check_cancelled()?;
    if mutation.results.len() != batch.commands.len()
        || mutation.inverse.commands.len() != batch.commands.len()
        || mutation.changes.len() > MAX_DOMAIN_MUTATION_CHANGES
    {
        return Err(mutation_error(
            "domain result, change or inverse count is invalid",
        ));
    }
    let result_bytes = bounded_json_size(&mutation.results, MAX_DOMAIN_MUTATION_RESULT_BYTES)
        .map_err(|error| domain_pack_error(&error))?;
    let change_bytes = bounded_json_size(&mutation.changes, MAX_DOMAIN_MUTATION_CHANGE_BYTES)
        .map_err(|error| domain_pack_error(&error))?;
    let changed = &mutation.document;
    if changed.format != original.format
        || changed.format_version != original.format_version
        || changed.scenario_id != original.scenario_id
        || changed.domain_pack != original.domain_pack
        || changed.metadata != original.metadata
        || changed.settings != original.settings
        || changed.extensions != original.extensions
    {
        return Err(mutation_error("domain mutation changed host-owned fields"));
    }
    let document_limit = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)
        .map_err(|_| mutation_error("scenario byte limit is unavailable"))?;
    bounded_json_size(changed, document_limit).map_err(|error| domain_pack_error(&error))?;
    validate_document_shape(changed)?;
    if mutation.inverse.pack_id != batch.pack_id
        || mutation.inverse.scenario_schema_version != batch.scenario_schema_version
    {
        return Err(mutation_error(
            "domain inverse identity does not match its input",
        ));
    }
    mutation
        .inverse
        .validate_bounds()
        .map_err(|error| domain_pack_error(&error))?;
    for (command, result) in commands.iter().zip(&mutation.results) {
        context.check_cancelled()?;
        command
            .validate_result(result)
            .map_err(|error| domain_pack_error(&error))?;
    }
    let mut previous = None;
    for change in &mutation.changes {
        context.check_cancelled()?;
        if previous.is_some_and(|index| index > change.command_index) {
            return Err(mutation_error("domain changes are not in input order"));
        }
        let index = usize::try_from(change.command_index)
            .map_err(|_| mutation_error("domain change index is invalid"))?;
        let command = commands
            .get(index)
            .ok_or_else(|| mutation_error("domain change index is outside its input"))?;
        command
            .validate_change(&change.value)
            .map_err(|error| domain_pack_error(&error))?;
        previous = Some(change.command_index);
    }
    for inverse in &mutation.inverse.commands {
        context.check_cancelled()?;
        context
            .registry
            .command(&batch.pack_id, &inverse.command_type)
            .and_then(|command| command.validate_payload(&inverse.payload))
            .map_err(|error| domain_pack_error(&error))?;
    }
    context.check_cancelled()?;
    Ok((change_bytes, result_bytes))
}

fn mutation_error(message: &str) -> CommandError {
    validation_error(CODE_INVALID_RECORD_SHAPE, "/domainMutation", message)
}

fn domain_change(value: Value) -> Result<Change, CommandError> {
    let Value::Object(mut object) = value else {
        return Err(validation_error(
            CODE_INVALID_RECORD_SHAPE,
            "/domainChange",
            "domain-pack change must be an object",
        ));
    };
    let path = match object.remove("path") {
        Some(Value::String(path)) if path.starts_with('/') => path,
        _ => {
            return Err(validation_error(
                CODE_INVALID_RECORD_SHAPE,
                "/domainChange/path",
                "domain-pack change path must be absolute",
            ));
        }
    };
    let before = object.remove("before").filter(|value| !value.is_null());
    let after = object.remove("after").filter(|value| !value.is_null());
    let kind = match (&before, &after) {
        (None, Some(_)) => ChangeKind::Added,
        (Some(_), None) => ChangeKind::Removed,
        _ => ChangeKind::Updated,
    };
    Ok(Change {
        kind,
        path,
        before,
        after,
    })
}

fn domain_pack_error(error: &DomainPackError) -> CommandError {
    if matches!(error, DomainPackError::Cancelled) {
        return CommandError::Cancelled;
    }
    validation_error(
        CODE_INVALID_RECORD_SHAPE,
        "/domainPack",
        format!("domain-pack contract rejected the operation: {error}"),
    )
}

fn effect(
    kind: ChangeKind,
    path: String,
    before: Option<Value>,
    after: Option<Value>,
    inverse: ScenarioCommand,
    summary: String,
    command_type: &str,
) -> PackCommandEffect {
    PackCommandEffect {
        changes: vec![Change {
            kind,
            path,
            before,
            after,
        }],
        inverse,
        summary,
        command_type: command_type.to_owned(),
    }
}
/// Validate generic structural invariants before pack-owned typed validation.
///
/// Import staging and command application share this narrow private-data check
/// so malformed records cannot reach persistence or a pack decoder.
///
/// # Errors
///
/// Returns [`CommandError::Validation`] when a domain record is not an object
/// or an embedded record identity differs from its map key.
pub fn validate_document_shape(document: &ScenarioDocument) -> Result<(), CommandError> {
    validate_map(&document.domain.entities, "/domain/entities")?;
    validate_map(&document.domain.rules, "/domain/rules")?;
    validate_map(&document.domain.preferences, "/domain/preferences")?;
    validate_map(
        &document.domain.locked_assignments,
        "/domain/lockedAssignments",
    )?;
    validate_safe_serialized(document, "/document")?;
    Ok(())
}

const COMMAND_JSON_LIMITS: PortableJsonLimits = PortableJsonLimits {
    max_depth: 128,
    max_string_bytes: 1024 * 1024,
    max_collection_items: 1_000_000,
};

fn validate_safe_serialized<T: Serialize>(value: &T, path: &str) -> Result<(), CommandError> {
    let serialized = serde_json::to_value(value).map_err(|error| {
        validation_error(
            CODE_PROHIBITED_DATA,
            path,
            format!("value cannot be checked before application: {error}"),
        )
    })?;
    validate_nonsecret_portable_json(&serialized, &COMMAND_JSON_LIMITS)
        .map_err(|error| validation_error(CODE_PROHIBITED_DATA, path, error.to_string()))
}

fn validate_map<K>(values: &BTreeMap<K, Value>, base: &str) -> Result<(), CommandError>
where
    K: Ord + std::fmt::Display,
{
    for (map_id, value) in values {
        let Some(object) = value.as_object() else {
            return Err(validation_error(
                CODE_INVALID_RECORD_SHAPE,
                format!("{base}/{map_id}"),
                "record must be a JSON object",
            ));
        };
        if let Some(id_value) = object.get("id") {
            let expected_id = map_id.to_string();
            if id_value.as_str() != Some(expected_id.as_str()) {
                return Err(validation_error(
                    CODE_RECORD_ID_MISMATCH,
                    format!("{base}/{map_id}/id"),
                    format!("record id must match map key {expected_id}"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_record<I: std::fmt::Display>(
    value: &Value,
    expected_id: &I,
    path: &str,
) -> Result<(), CommandError> {
    let Some(object) = value.as_object() else {
        return Err(validation_error(
            CODE_INVALID_RECORD_SHAPE,
            path,
            "record must be a JSON object",
        ));
    };
    if let Some(id_value) = object.get("id") {
        let Some(id) = id_value.as_str() else {
            return Err(validation_error(
                CODE_RECORD_ID_MISMATCH,
                format!("{path}/id"),
                "record id must be a string when present",
            ));
        };
        let expected_id = expected_id.to_string();
        if id != expected_id.as_str() {
            return Err(validation_error(
                CODE_RECORD_ID_MISMATCH,
                format!("{path}/id"),
                format!("record id {id} does not match map key {expected_id}"),
            ));
        }
    }
    validate_safe_serialized(value, path)?;
    Ok(())
}

fn validation_delta(before: &[ValidationIssue], after: &[ValidationIssue]) -> ValidationDelta {
    let before_by_key: BTreeMap<String, &ValidationIssue> = before
        .iter()
        .map(|issue| (validation_issue_key(issue), issue))
        .collect();
    let after_by_key: BTreeMap<String, &ValidationIssue> = after
        .iter()
        .map(|issue| (validation_issue_key(issue), issue))
        .collect();
    let added = after_by_key
        .iter()
        .filter(|(key, _)| !before_by_key.contains_key(*key))
        .map(|(_, issue)| (*issue).clone())
        .collect();
    let resolved = before
        .iter()
        .map(|issue| issue.code.as_str())
        .filter(|code| !after.iter().any(|issue| issue.code == *code))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect();
    ValidationDelta { added, resolved }
}

fn validation_issue_key(issue: &ValidationIssue) -> String {
    match &issue.field_path {
        Some(path) => format!("{}:{path}", issue.code),
        None => issue.code.clone(),
    }
}

fn validation_error(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> CommandError {
    CommandError::Validation {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn entity_path(id: &EntityId) -> String {
    format!("/domain/entities/{id}")
}

fn rule_path(id: &RuleId) -> String {
    format!("/domain/rules/{id}")
}

fn preference_path(id: &RuleId) -> String {
    format!("/domain/preferences/{id}")
}

fn lock_path(id: &AssignmentId) -> String {
    format!("/domain/lockedAssignments/{id}")
}

/// Return stable command type metadata without applying the command.
#[must_use]
pub fn command_type(command: &ScenarioCommand) -> String {
    match command {
        ScenarioCommand::AddEntity(_) => "add_entity".to_owned(),
        ScenarioCommand::UpdateEntity(_) => "update_entity".to_owned(),
        ScenarioCommand::RemoveEntity(_) => "remove_entity".to_owned(),
        ScenarioCommand::AddRule(_) => "add_rule".to_owned(),
        ScenarioCommand::UpdateRule(_) => "update_rule".to_owned(),
        ScenarioCommand::RemoveRule(_) => "remove_rule".to_owned(),
        ScenarioCommand::SetPreference(_) => "set_preference".to_owned(),
        ScenarioCommand::LockAssignment(_) => "lock_assignment".to_owned(),
        ScenarioCommand::UnlockAssignment(_) => "unlock_assignment".to_owned(),
        ScenarioCommand::ApplyDomainCommand(value) => format!("domain.{}", value.command_type),
        ScenarioCommand::ApplyBatch(_) => "apply_batch".to_owned(),
    }
}

/// Return a deterministic human-readable summary without applying the command.
#[must_use]
pub fn human_summary(command: &ScenarioCommand) -> String {
    match command {
        ScenarioCommand::AddEntity(value) => format!("Add entity {}", value.entity_id),
        ScenarioCommand::UpdateEntity(value) => format!("Update entity {}", value.entity_id),
        ScenarioCommand::RemoveEntity(value) => format!("Remove entity {}", value.entity_id),
        ScenarioCommand::AddRule(value) => format!("Add rule {}", value.rule_id),
        ScenarioCommand::UpdateRule(value) => format!("Update rule {}", value.rule_id),
        ScenarioCommand::RemoveRule(value) => format!("Remove rule {}", value.rule_id),
        ScenarioCommand::SetPreference(value) => {
            let action = if value.value.is_some() {
                "Set"
            } else {
                "Clear"
            };
            format!("{action} preference {}", value.preference_id)
        }
        ScenarioCommand::LockAssignment(value) => {
            format!("Lock assignment {}", value.assignment_id)
        }
        ScenarioCommand::UnlockAssignment(value) => {
            format!("Unlock assignment {}", value.assignment_id)
        }
        ScenarioCommand::ApplyDomainCommand(value) => {
            format!("Apply {} domain command", value.command_type)
        }
        ScenarioCommand::ApplyBatch(value) => {
            let count = value.commands.len();
            match &value.label {
                Some(label) => format!("{label} ({count} commands)"),
                None => format!("Apply batch ({count} commands)"),
            }
        }
    }
}
