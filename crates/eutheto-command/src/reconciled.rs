use super::{
    AppliedCommand, ApplyContext, CommandError, MAX_BATCH_COMMANDS, PackCommandEffect,
    apply_nested, batch_effect, finalize_application, preflight_settings_leaves,
    validate_safe_serialized,
};
use eutheto_domain_api::{
    DomainBatchCommand, DomainPackError, DomainPackRegistry, bounded_json_size,
};
use eutheto_types::{
    CancellationToken, Change, CommandBatch, Revision, ScenarioCommand, ScenarioDocument,
};
use thiserror::Error;

/// Journal-ready ordinary command and its already-applied, private replacement document.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedReconciledCommand {
    pub effective_command: ScenarioCommand,
    pub applied: AppliedCommand,
}

/// Preserves the distinction between command failure and the reconciliation authority's error.
#[derive(Debug, Error)]
pub enum ReconciledCommandError {
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error(transparent)]
    Reconciliation(#[from] DomainPackError),
}

/// Applies a draft once, then derives and applies optional reconciliation to the same working copy.
///
/// The callback observes the exact prospective document and ordered draft changes. Neither its
/// view nor this preparation may be published until this function succeeds: combined command,
/// output, and reverse-ordered inverse limits are checked before returning. With neither a draft
/// nor reconciliation, returns `None` without manufacturing an empty batch or advancing a revision.
/// Persistence, actor/source metadata, and optimistic commit checks remain the caller's authority.
///
/// # Errors
///
/// Returns existing command failures or the original error from the reconciliation authority.
pub fn apply_reconciled_command_with_registry(
    document: &ScenarioDocument,
    current_revision: Revision,
    draft: Option<&ScenarioCommand>,
    registry: &DomainPackRegistry,
    cancellation: &CancellationToken,
    reconcile: impl FnOnce(
        &ScenarioDocument,
        &[Change],
    ) -> Result<Option<DomainBatchCommand>, DomainPackError>,
) -> Result<Option<PreparedReconciledCommand>, ReconciledCommandError> {
    if let Some(command) = draft {
        preflight_setup_command(command, "/command", cancellation)?;
    }
    let mut context = ApplyContext::new(document, registry, cancellation, draft)?;
    context.check_cancelled()?;
    let prepared_draft = if let Some(command) = draft {
        let before_issues = context.pack.validate_fast(document).issues;
        let mut working = document.clone();
        let effect = apply_nested(&mut working, command, &mut context, 0)?;
        Some(DraftApplication {
            command,
            working,
            before_issues,
            effect,
        })
    } else {
        None
    };
    context.check_cancelled()?;
    let (prospective, changes) = match &prepared_draft {
        Some(draft) => (&draft.working, draft.effect.changes.as_slice()),
        None => (document, &[][..]),
    };
    let reconciliation = reconcile(prospective, changes)?;
    context.check_cancelled()?;
    let reconciliation = reconciliation
        .map(|batch| reconciliation_command(document, batch, cancellation))
        .transpose()?;
    let combined = prepared_draft.is_some() && reconciliation.is_some();
    let (working, before_issues, effective_command, effect) = match (prepared_draft, reconciliation)
    {
        (None, None) => return Ok(None),
        (Some(draft), None) => (
            draft.working,
            draft.before_issues,
            draft.command.clone(),
            draft.effect,
        ),
        (None, Some(generated)) => {
            let before_issues = context.pack.validate_fast(document).issues;
            let mut working = document.clone();
            let effect = apply_nested(&mut working, &generated, &mut context, 0)?;
            (working, before_issues, generated, effect)
        }
        (Some(mut draft), Some(generated)) => {
            let mut remaining_leaves = MAX_BATCH_COMMANDS;
            preflight_settings_leaves(draft.command, 1, &mut remaining_leaves, cancellation)?;
            preflight_settings_leaves(&generated, 1, &mut remaining_leaves, cancellation)?;
            context.charge_batch_frame(None, 2)?;
            let effect = apply_nested(&mut draft.working, &generated, &mut context, 1)?;
            (
                draft.working,
                draft.before_issues,
                ScenarioCommand::ApplyBatch(CommandBatch {
                    label: None,
                    commands: vec![draft.command.clone(), generated],
                }),
                combine_effects(draft.effect, effect),
            )
        }
    };
    if combined {
        preflight_setup_command(&effective_command, "/command", cancellation)?;
    }
    let applied =
        finalize_application(working, current_revision, effect, &before_issues, &context)?;
    Ok(Some(PreparedReconciledCommand {
        effective_command,
        applied,
    }))
}

/// Checks a setup draft's ordinary replay shape, privacy bounds and compact input ceiling.
/// Hosts use this before capturing a heavy immutable scenario.
///
/// # Errors
/// Rejects cancelled, malformed, over-deep or oversized commands without applying them.
pub fn preflight_setup_command(
    command: &ScenarioCommand,
    path: &str,
    cancellation: &CancellationToken,
) -> Result<(), CommandError> {
    let mut remaining_leaves = MAX_BATCH_COMMANDS;
    preflight_settings_leaves(command, 0, &mut remaining_leaves, cancellation)?;
    let limit = usize::try_from(eutheto_types::MAX_SCENARIO_DOCUMENT_BYTES)
        .map_err(|_| CommandError::ResourceLimitExceeded)?;
    bounded_json_size(command, limit).map_err(|_| CommandError::ResourceLimitExceeded)?;
    validate_safe_serialized(command, path)
}

struct DraftApplication<'a> {
    command: &'a ScenarioCommand,
    working: ScenarioDocument,
    before_issues: Vec<eutheto_types::ValidationIssue>,
    effect: PackCommandEffect,
}

fn reconciliation_command(
    document: &ScenarioDocument,
    batch: DomainBatchCommand,
    cancellation: &CancellationToken,
) -> Result<ScenarioCommand, ReconciledCommandError> {
    batch.validate_bounds()?;
    if batch.pack_id != document.domain_pack.id
        || batch.scenario_schema_version != document.domain_pack.schema_version
    {
        return Err(DomainPackError::InvalidPayload {
            path: "/reconciliation".to_owned(),
            message: "reconciliation identity must match its source document".to_owned(),
        }
        .into());
    }
    let command = ScenarioCommand::ApplyBatch(CommandBatch {
        label: batch.label,
        commands: batch
            .commands
            .into_iter()
            .map(ScenarioCommand::ApplyDomainCommand)
            .collect(),
    });
    preflight_setup_command(&command, "/reconciliation", cancellation)?;
    Ok(command)
}

fn combine_effects(
    mut draft: PackCommandEffect,
    generated: PackCommandEffect,
) -> PackCommandEffect {
    draft.changes.extend(generated.changes);
    batch_effect(
        None,
        2,
        draft.changes,
        vec![draft.inverse, generated.inverse],
    )
}
