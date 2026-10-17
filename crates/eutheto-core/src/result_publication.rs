//! Shared native source custody and revision-leased publication for captured result artifacts.
use super::{
    OrdinaryPublicationHandle, export_error, join_error, operation_interrupted,
    recipient_report::RecipientReportMetadataV1, store_error, validation_error,
};
use crate::counterfactual::CounterfactualPublicationHandle;
use eutheto_export::{ExportError, PreparedPublication};
use eutheto_store::SqliteScenarioStore;
use eutheto_types::{AppError, OperationControl, RecipientSolverV1, RecipientSourceV1};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(super) struct SourceCustody {
    pub ordinary: Option<OrdinaryPublicationHandle>,
    pub counterfactual: Option<CounterfactualPublicationHandle>,
}

pub(super) fn admit(sources: &[SourceCustody]) -> Result<(), AppError> {
    for source in sources {
        if let Some(handle) = &source.ordinary {
            handle.admit()?;
        }
        if let Some(handle) = &source.counterfactual {
            handle.admit()?;
        }
    }
    Ok(())
}

pub(super) struct CapturedPublication {
    pub store: Arc<SqliteScenarioStore>,
    pub metadata: RecipientReportMetadataV1,
    pub sources: Vec<SourceCustody>,
}

impl CapturedPublication {
    pub fn admit(&self) -> Result<(), AppError> {
        admit(&self.sources)
    }

    // Inline size belongs to the outer capture. Artifact buffers are charged alongside
    // metadata and actual retained source capacities, never only Arc pointer sizes.
    pub fn retained_bytes(
        &self,
        control: &OperationControl,
        inline_bytes: usize,
        artifact_bytes: usize,
    ) -> Result<usize, AppError> {
        self.admit()?;
        let result = (|| {
            let mut bytes = super::recipient_memory::SourceBytes::new(control)?;
            bytes.add(inline_bytes)?;
            bytes.add(artifact_bytes)?;
            let RecipientReportMetadataV1 {
                schema_version: _,
                source,
                library_revision: _,
                current_revision: _,
                run_id: _,
                run_input_checksum,
                run_manifest_checksum,
            } = &self.metadata;
            let RecipientSourceV1 {
                scenario_id: _,
                scenario_revision: _,
                solution_id: _,
                result_schema_version: _,
                accepted_result_checksum,
                verification_report_checksum,
                source_document_hash,
                origin: _,
                observed_current_revision: _,
                verification: _,
                solver,
            } = source;
            let RecipientSolverV1 {
                backend_id,
                recorded_version,
            } = solver;
            bytes.strings([
                run_input_checksum,
                accepted_result_checksum,
                verification_report_checksum,
                source_document_hash,
                backend_id,
            ])?;
            for value in [run_manifest_checksum, recorded_version]
                .into_iter()
                .flatten()
            {
                bytes.strings([value])?;
            }
            bytes.vector(&self.sources, self.sources.capacity())?;
            for source in &self.sources {
                if let Some(handle) = &source.ordinary {
                    bytes.add(
                        handle.recipient_retained_bytes(control)?
                            - size_of::<OrdinaryPublicationHandle>(),
                    )?;
                }
                if let Some(handle) = &source.counterfactual {
                    bytes.add(
                        handle.recipient_retained_bytes(control)?
                            - size_of::<CounterfactualPublicationHandle>(),
                    )?;
                }
            }
            Ok(bytes.finish())
        })();
        self.admit()?;
        result
    }

    pub async fn publish<F>(
        self,
        destination: PathBuf,
        allow_stale: bool,
        control: OperationControl,
        stage: F,
    ) -> Result<(), AppError>
    where
        F: FnOnce(&Path, &OperationControl) -> Result<PreparedPublication, ExportError>
            + Send
            + 'static,
    {
        // The task owns bytes (inside stage) and handles even if its observer is dropped.
        tokio::spawn(async move {
            self.admit()?;
            let approval = (|| {
                if !allow_stale
                    && self.metadata.source.scenario_revision != self.metadata.current_revision
                {
                    return Err(validation_error(
                        "recipient.stale_consent_required",
                        "/allowStale",
                        "Review and explicitly approve this captured older plan.",
                    ));
                }
                control.check().map_err(operation_interrupted)
            })();
            self.admit()?;
            approval?;
            let sources = self.sources;
            let stage_control = control.clone();
            let staged = tokio::task::spawn_blocking(move || stage(&destination, &stage_control))
                .await
                .map_err(join_error)
                .and_then(|value| value.map_err(|error| export_error(&error)));
            admit(&sources)?;
            let staged = staged?;
            let final_sources = sources.clone();
            let publication = self
                .store
                .with_publication_revision_lease(
                    self.metadata.library_revision,
                    Some((
                        self.metadata.source.scenario_id,
                        self.metadata.current_revision,
                    )),
                    move || {
                        admit(&final_sources)?;
                        let outcome = staged
                            .publish_controlled(&control)
                            .map_err(|error| export_error(&error));
                        if outcome.is_err() {
                            admit(&final_sources)?;
                        }
                        outcome
                    },
                )
                .await
                .map_err(store_error)
                .and_then(std::convert::identity);
            if publication.is_err() {
                admit(&sources)?;
            }
            // Never revoke a confirmed commit with a late cancellation/alarm/cleanup check.
            publication
        })
        .await
        .map_err(join_error)?
    }
}
