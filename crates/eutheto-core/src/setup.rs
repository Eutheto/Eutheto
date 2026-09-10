//! Immutable setup capture and executable, nonpublishing command previews.

use super::{
    EuthetoApp, command_store_error, domain_interruption, ensure_supported_document, join_error,
    operation_interrupted, resource_limit_error, store_error, validation_error,
};
pub use eutheto_command::MAX_COMMAND_RESULT_BYTES;
use eutheto_command::{
    ReconciledCommandError, apply_reconciled_command_with_registry, preflight_setup_command,
};
use eutheto_domain_api::{
    ContractJsonLimits, DomainPackError, DomainPackRegistry, DomainView, DomainViewInput,
    SetupQueryDescriptor, SetupViewContext, bounded_json_size, validate_contract_value,
};
pub use eutheto_domain_api::{DomainSetupQueryV1, SetupContinuationV1, SetupQuerySource};
use eutheto_store::StoredProject;
use eutheto_types::{
    AppError, CancellationToken, OperationControl, Revision, ScenarioCommand, ScenarioDocument,
    ScenarioId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[path = "reviewed_generation.rs"]
mod reviewed_generation;
pub use reviewed_generation::*;

#[path = "readiness.rs"]
mod readiness;
pub(super) use readiness::ValidationReadiness;
pub use readiness::*;

const QUERY_BYTES: usize = 64 * 1024;
const FRAME_BYTES: usize = 64 * 1024;
const MAX_VIEW_BYTES: usize = 32 * 1024 * 1024;

/// A stored subject or an ordinary typed draft, never a client replacement document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SetupSourceV2 {
    Stored,
    CommandPreview { command: ScenarioCommand },
}

/// A view of the captured base revision, including when its subject is prospective.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioSetupViewResultV2 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub view: DomainView,
}

pub(super) struct CancelOnDrop(pub(super) CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl EuthetoApp {
    /// Creates an independently cancellable setup operation that also observes application shutdown.
    #[must_use]
    pub fn setup_cancellation(&self) -> CancellationToken {
        self.cancellation.child()
    }

    /// Signals application-root cancellation without claiming that in-flight work has settled.
    pub fn request_shutdown(&self) {
        self.cancellation.cancel();
    }

    /// Validates a native prepare purpose against the same registered setup-source policy.
    ///
    /// # Errors
    /// Rejects unknown view IDs or a source not supported by the registered query.
    pub fn preflight_setup_source(
        &self,
        view_id: &str,
        source: SetupQuerySource,
    ) -> Result<(), AppError> {
        self.setup_query_descriptor(view_id)?
            .validate_subject(source, false)
            .map_err(|error| setup_domain_error(&error))
    }

    fn setup_query_descriptor(&self, view_id: &str) -> Result<&SetupQueryDescriptor, AppError> {
        self.pack_registry
            .descriptors()
            .find_map(|pack| {
                self.pack_registry
                    .catalog(&pack.id)?
                    .setup_queries
                    .iter()
                    .find(|descriptor| descriptor.id == view_id)
            })
            .ok_or_else(|| setup_invalid("/query/viewId"))
    }

    /// Validates query/command ingress without reading or cloning scenario state.
    /// Native admission calls this before reserving a heavy snapshot permit.
    ///
    /// # Errors
    /// Rejects unknown setup queries, invalid versions, malformed data and input resource limits.
    pub fn preflight_setup_view(
        &self,
        source: &SetupSourceV2,
        query: &DomainSetupQueryV1,
        cancellation: &CancellationToken,
    ) -> Result<(), AppError> {
        check_cancelled(cancellation)?;
        if query.schema_version != 1 {
            return Err(setup_invalid("/query/schemaVersion"));
        }
        let descriptor = self.setup_query_descriptor(&query.view_id)?;
        descriptor
            .validate_subject(
                match source {
                    SetupSourceV2::Stored => SetupQuerySource::Stored,
                    SetupSourceV2::CommandPreview { .. } => SetupQuerySource::CommandPreview,
                },
                query.continuation.is_some(),
            )
            .map_err(|error| setup_domain_error(&error))?;
        descriptor
            .validate_parameters(
                &query.parameters,
                ContractJsonLimits {
                    max_serialized_bytes: QUERY_BYTES,
                    ..ContractJsonLimits::DEFAULT
                },
            )
            .map_err(|error| setup_domain_error(&error))?;
        if let Some(cursor) = &query.continuation {
            if cursor.schema_version != 1 {
                return Err(setup_invalid("/query/continuation/schemaVersion"));
            }
            validate_contract_value(
                &serde_json::json!({}),
                &cursor.position,
                ContractJsonLimits {
                    max_depth: 4,
                    max_serialized_bytes: 512,
                    ..ContractJsonLimits::DEFAULT
                },
            )
            .map_err(|error| setup_domain_error(&error))?;
        }
        bounded_json_size(query, QUERY_BYTES).map_err(|error| setup_domain_error(&error))?;
        if let SetupSourceV2::CommandPreview { command } = source {
            preflight_setup_command(command, "/source/command", cancellation)
                .map_err(|error| store_error(command_store_error(&error)))?;
        }
        check_cancelled(cancellation)
    }

    /// Builds a registered setup view after immutable capture, off the asynchronous executor.
    /// The caller must hold its native heavy admission permit for the complete invocation.
    /// No document, history entry, validation lifecycle or event is published by this read.
    ///
    /// # Errors
    /// Rejects stale state, invalid drafts/views, reconciliation replay limits and cancellation.
    pub async fn setup_view(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        source: SetupSourceV2,
        query: DomainSetupQueryV1,
        cancellation: CancellationToken,
    ) -> Result<ScenarioSetupViewResultV2, AppError> {
        let _cancel_on_drop = CancelOnDrop(cancellation.clone());
        self.preflight_setup_view(&source, &query, &cancellation)?;
        let project = self
            .capture_setup_project(scenario_id, Some(expected_revision), &cancellation)
            .await?;
        let registry = Arc::clone(&self.pack_registry);
        tokio::task::spawn_blocking(move || {
            build_setup_view(&project, &source, &query, &registry, &cancellation)
        })
        .await
        .map_err(join_error)?
    }

    async fn capture_setup_project(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Option<Revision>,
        cancellation: &CancellationToken,
    ) -> Result<StoredProject, AppError> {
        check_cancelled(cancellation)?;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        check_cancelled(cancellation)?;
        let project = self
            .store
            .get_project(scenario_id)
            .await
            .map_err(store_error)?;
        if let Some(expected_revision) = expected_revision
            && project.summary.revision != expected_revision
        {
            return Err(AppError::Conflict {
                expected_revision,
                actual_revision: project.summary.revision,
            });
        }
        ensure_supported_document(&project.document, &self.pack_registry).map_err(store_error)?;
        check_cancelled(cancellation)?;
        Ok(project)
    }
}

fn build_setup_view(
    project: &StoredProject,
    source: &SetupSourceV2,
    query: &DomainSetupQueryV1,
    registry: &DomainPackRegistry,
    cancellation: &CancellationToken,
) -> Result<ScenarioSetupViewResultV2, AppError> {
    let control = OperationControl::Cancellation(cancellation.clone());
    control.check().map_err(operation_interrupted)?;
    let document = &project.document;
    let pack = registry
        .require(&document.domain_pack.id)
        .map_err(|error| setup_domain_error(&error))?;
    let context = SetupViewContext {
        revision: project.summary.revision,
        query_fingerprint: query_fingerprint(document, project.summary.revision, source, query)?,
    };
    let view = match source {
        SetupSourceV2::Stored => {
            let output = pack
                .build_view(
                    DomainViewInput::StoredSetup {
                        document,
                        query,
                        context,
                    },
                    &control,
                )
                .map_err(|error| setup_domain_error(&error))?;
            if let Some(reconciliation) = output.reconciliation {
                apply_reconciled_command_with_registry(
                    document,
                    context.revision,
                    None,
                    registry,
                    cancellation,
                    |_, _| Ok(Some(reconciliation)),
                )
                .map_err(reconciliation_error)?;
            }
            output.view
        }
        SetupSourceV2::CommandPreview { command } => {
            let mut retained_view = None;
            apply_reconciled_command_with_registry(
                document,
                context.revision,
                Some(command),
                registry,
                cancellation,
                |prospective, changes| {
                    let output = pack.build_view(
                        DomainViewInput::CommandPreviewSetup {
                            original: document,
                            prospective,
                            command,
                            changes,
                            query,
                            context,
                        },
                        &control,
                    )?;
                    retained_view = Some(output.view);
                    Ok(output.reconciliation)
                },
            )
            .map_err(reconciliation_error)?;
            retained_view.ok_or_else(|| setup_invalid("/view"))?
        }
    };
    control.check().map_err(operation_interrupted)?;
    let data_bytes = bounded_json_size(&view.data, MAX_VIEW_BYTES)
        .map_err(|error| setup_domain_error(&error))?;
    let result = ScenarioSetupViewResultV2 {
        schema_version: 2,
        scenario_id: document.scenario_id,
        revision: context.revision,
        view,
    };
    bounded_json_size(
        &result,
        data_bytes
            .checked_add(FRAME_BYTES)
            .ok_or_else(resource_limit_error)?,
    )
    .map_err(|error| setup_domain_error(&error))?;
    control.check().map_err(operation_interrupted)?;
    Ok(result)
}

fn query_fingerprint(
    document: &ScenarioDocument,
    revision: Revision,
    source: &SetupSourceV2,
    query: &DomainSetupQueryV1,
) -> Result<[u8; 32], AppError> {
    // Fixed fields and ordered JSON maps; continuation is deliberately not part of the preimage.
    let mut hash = blake3::Hasher::new();
    hash.update(b"eutheto/setup-query/v1\0");
    serde_json::to_writer(
        &mut hash,
        &(
            2_u32,
            document.scenario_id,
            revision,
            &document.domain_pack,
            query.schema_version,
            &query.view_id,
            &query.parameters,
            source,
        ),
    )
    .map_err(|_| setup_invalid("/query"))?;
    Ok(*hash.finalize().as_bytes())
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), AppError> {
    OperationControl::Cancellation(cancellation.clone())
        .check()
        .map_err(operation_interrupted)
}

fn reconciliation_error(error: ReconciledCommandError) -> AppError {
    match error {
        ReconciledCommandError::Command(error) => store_error(command_store_error(&error)),
        ReconciledCommandError::Reconciliation(error) => setup_domain_error(&error),
    }
}

fn setup_domain_error(error: &DomainPackError) -> AppError {
    domain_interruption(error).unwrap_or_else(|| setup_invalid("/query"))
}

fn setup_invalid(path: &str) -> AppError {
    validation_error(
        "scenario.setup_invalid",
        path,
        "The setup request is not valid for the captured scenario and registered view.",
    )
}
