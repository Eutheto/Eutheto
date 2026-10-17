//! Server-derived generation preparation, followed by one short optimistic publication.

use super::{
    CancelOnDrop, EuthetoApp, SetupSourceV2, check_cancelled, join_error, reconciliation_error,
    setup_domain_error, setup_invalid, store_error, validation_error,
};
use eutheto_command::{
    AppliedCommand, apply_reconciled_command_with_registry, preflight_setup_command,
};
use eutheto_domain_api::{DomainPackError, DomainPackRegistry};
use eutheto_store::{CommandWrite, JournalWrite, RedoBranchPolicy, StoreError, StoredProject};
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandId, CommandResult, CommandSource, OperationId,
    RequestId, Revision, ScenarioDocument, ScenarioId,
};
use eutheto_workforce::temporal::{TemporalError, TemporalIssueKind, preview_generation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Approval of a reviewed prospective hash, not permission to supply reconciliation commands.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkforceGenerationApplyRequestV1 {
    pub request_id: RequestId,
    pub schema_version: u32,
    pub operation_id: OperationId,
    pub command_id: CommandId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub actor: ActorRef,
    pub source: SetupSourceV2,
    pub prospective_hash: [u8; 32],
    pub truncate_redo: bool,
}

struct PreparedGeneration {
    original: ScenarioDocument,
    applied: AppliedCommand,
    forward_json: Value,
    inverse_json: Option<Value>,
    protected: Option<super::super::manual::ProtectedEffectWrite>,
}

impl EuthetoApp {
    /// Checks reviewed-mutation ingress before native heavy admission, without scenario I/O.
    ///
    /// # Errors
    /// Rejects cancellation, unsupported versions, invalid actors and unbounded commands.
    pub fn preflight_reviewed_generation(
        &self,
        request: &WorkforceGenerationApplyRequestV1,
        cancellation: &CancellationToken,
    ) -> Result<(), AppError> {
        self.check_cancelled()?;
        check_cancelled(cancellation)?;
        if request.schema_version != 1 {
            return Err(setup_invalid("/schemaVersion"));
        }
        if !super::super::valid_command_actor(&request.actor) {
            return Err(setup_invalid("/actor"));
        }
        if let SetupSourceV2::CommandPreview { command } = &request.source {
            preflight_setup_command(command, "/source/command", cancellation)
                .map_err(|error| store_error(super::command_store_error(&error, Some(command))))?;
        }
        check_cancelled(cancellation)
    }

    /// Rebuilds reviewed generation outside locks, then commits its exact prepared command once.
    /// Native callers must claim the exact operation purpose/context and retain heavy admission.
    /// The invoke result is authoritative: cancellation after commit cannot replace success.
    ///
    /// # Errors
    /// Rejects stale/replaced sources, changed hashes, invalid drafts, empty stored reconciliation,
    /// interrupted work and ordinary replay/journal limits without a partial mutation.
    #[allow(clippy::too_many_lines)]
    pub async fn apply_reviewed_generation(
        &self,
        request: WorkforceGenerationApplyRequestV1,
        cancellation: CancellationToken,
    ) -> Result<CommandResult, AppError> {
        let _cancel_on_drop = CancelOnDrop::new(cancellation.clone());
        self.preflight_reviewed_generation(&request, &cancellation)?;
        let captured = self
            .capture_setup_project(
                request.scenario_id,
                Some(request.expected_revision),
                &cancellation,
            )
            .await?;
        let (project, custody_stamp) = self
            .store
            .capture_scenario_custody(
                request.scenario_id,
                request.expected_revision,
                captured.document,
                eutheto_types::OperationControl::Cancellation(cancellation.clone()),
                None,
            )
            .await
            .map_err(store_error)?;
        let registry = Arc::clone(&self.pack_registry);
        let preparation_registry = Arc::clone(&registry);
        let preparation_token = cancellation.clone();
        let (request, prepared) = tokio::task::spawn_blocking(move || {
            let prepared =
                prepare_generation(project, &request, &preparation_registry, &preparation_token)?;
            Ok::<_, AppError>((request, prepared))
        })
        .await
        .map_err(join_error)??;
        check_cancelled(&cancellation)?;
        let mutation = self.scenario_lock(request.scenario_id).await;
        let _guard = mutation.lock().await;
        check_cancelled(&cancellation)?;
        let applied_at = self.clock.now();
        let result = self
            .store
            .execute_command(
                request.scenario_id,
                request.expected_revision,
                if request.truncate_redo {
                    RedoBranchPolicy::Truncate
                } else {
                    RedoBranchPolicy::Reject
                },
                custody_stamp,
                eutheto_types::OperationControl::Cancellation(cancellation),
                move |current| {
                    // Library replacement can change source authority without taking this scenario lock.
                    let PreparedGeneration {
                        original,
                        mut applied,
                        forward_json,
                        inverse_json,
                        protected,
                    } = prepared;
                    if current != &original {
                        return Err(StoreError::CommandApplication {
                            code: "scenario.setup_source_changed".to_owned(),
                            message: "The captured scenario was replaced; review generation again."
                                .to_owned(),
                            field_path: None,
                        });
                    }
                    applied.document.metadata.updated_at = applied_at;
                    let mut output = applied.result;
                    let (command_type, command, inverse, actor, summary) =
                        if let Some(protected) = protected {
                            output
                                .change_set
                                .changes
                                .extend(protected.change_set.changes);
                            output.validation_delta = super::super::validation_delta(
                                &super::super::validate(&original, &registry),
                                &super::super::validate(&applied.document, &registry),
                            );
                            output.inverse = None;
                            (
                                eutheto_store::NATIVE_PRIVATE_HISTORY_COMMAND_TYPE.to_owned(),
                                protected.command,
                                protected.inverse,
                                super::super::private_actor(request.actor),
                                "Updated private Workforce selection".to_owned(),
                            )
                        } else {
                            (
                                applied.command_type,
                                forward_json,
                                inverse_json,
                                request.actor,
                                applied.summary,
                            )
                        };
                    Ok(CommandWrite {
                        document: applied.document,
                        journal: JournalWrite {
                            command_type,
                            command,
                            command_id: request.command_id,
                            inverse,
                            actor,
                            source: CommandSource::Desktop,
                            summary,
                            created_at: applied_at,
                        },
                        output,
                    })
                },
            )
            .await
            .map_err(store_error)?;
        let mut output = result.output;
        output.new_revision = result.new_revision;
        self.publish_scenario_events(request.scenario_id, request.request_id, &output);
        Ok(output)
    }
}

fn prepare_generation(
    project: StoredProject,
    request: &WorkforceGenerationApplyRequestV1,
    registry: &DomainPackRegistry,
    cancellation: &CancellationToken,
) -> Result<PreparedGeneration, AppError> {
    if project.document.domain_pack.id.as_str() != "official.workforce" {
        return Err(setup_invalid("/scenarioId"));
    }
    let draft = match &request.source {
        SetupSourceV2::Stored => None,
        SetupSourceV2::CommandPreview { command } => Some(command),
    };
    let mut command = apply_reconciled_command_with_registry(
        &project.document,
        request.expected_revision,
        draft,
        registry,
        cancellation,
        |prospective, _| {
            let preview = preview_generation(&project.document, prospective, cancellation)
                .map_err(temporal_error)?;
            Ok(preview.reconciliation)
        },
    )
    .map_err(|error| reconciliation_error(error, draft))?
    .ok_or_else(|| {
        validation_error(
            "workforce.generation_no_changes",
            "/source",
            "There is no reviewed generation change to apply.",
        )
    })?;
    check_cancelled(cancellation)?;
    // Serialization is preparation work, never replay or large JSON conversion inside the write tx.
    let forward_json = serde_json::to_value(&command.effective_command)
        .map_err(StoreError::Json)
        .map_err(store_error)?;
    let inverse_json = command
        .applied
        .result
        .inverse
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(StoreError::Json)
        .map_err(store_error)?;
    check_cancelled(cancellation)?;
    let control = eutheto_types::OperationControl::Cancellation(cancellation.clone());
    let protected = super::super::manual::finalize_base_change(
        &project.document,
        &mut command.applied.document,
        &forward_json,
        inverse_json.as_ref(),
        Some(&control),
    )
    .map_err(store_error)?;
    let finalized = preview_generation(&project.document, &command.applied.document, cancellation)
        .map_err(temporal_error)
        .map_err(setup_domain_error)?;
    if finalized.prospective_hash != request.prospective_hash {
        return Err(validation_error(
            "workforce.generation_review_changed",
            "/prospectiveHash",
            "The prospective scenario differs from the reviewed generation; review it again.",
        ));
    }
    Ok(PreparedGeneration {
        original: project.document,
        applied: command.applied,
        forward_json,
        inverse_json,
        protected,
    })
}

fn temporal_error(error: TemporalError) -> DomainPackError {
    match error {
        TemporalError::InvalidDocument(error) => error,
        TemporalError::Cancelled => DomainPackError::Cancelled,
        TemporalError::Issue(issue) => match issue.kind {
            TemporalIssueKind::OccurrenceLimit
            | TemporalIssueKind::OutputLimit
            | TemporalIssueKind::CalendarLimit => DomainPackError::ResourceLimitExceeded,
            _ => DomainPackError::InvalidPayload {
                path: "/generation".to_owned(),
                message: "Workforce temporal generation requires a valid reviewed scenario."
                    .to_owned(),
            },
        },
    }
}
