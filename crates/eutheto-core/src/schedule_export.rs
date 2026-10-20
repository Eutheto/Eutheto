//! Complete accepted CSV/calendar artifacts retained through native review and publication.
use super::{
    AcceptedQuerySource, EuthetoApp, RecipientReportMetadataV1,
    headless::export::{encode_csv, encode_ics, encode_summary_csv},
    join_error, operation_interrupted,
    recipient_report::{
        recipient_control, recipient_metadata, recipient_source, validate_recipient_bindings,
    },
    result_publication::{CapturedPublication, admit},
    reverify_accepted_solution, reverify_checkpoint_solution, solution_contract_error,
    solution_reference_error, store_error, validation_error,
};
use eutheto_domain_api::{DomainPackRegistry, bounded_json_size};
use eutheto_export::{PORTABLE_LIMITS, prepare_text_atomic_controlled, sha256_hex};
use eutheto_types::{
    AppError, MAX_SCENARIO_DOCUMENT_BYTES, OperationControl, PersonId, RecipientSourceV1, Revision,
    Rfc3339Timestamp, ScenarioId, SolutionId,
};
use eutheto_workforce::export_source::WorkforceExportSource;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

pub const MAX_SCHEDULE_EXPORT_SAMPLE_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleExportFormatV1 {
    AssignmentsCsv,
    SummaryCsv,
    Ics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleExportRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
    pub expected_revision: Revision,
    pub format: ScheduleExportFormatV1,
    pub person_id: Option<PersonId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleExportPreviewV1 {
    pub schema_version: u32,
    pub source: RecipientSourceV1,
    pub library_revision: Revision,
    pub format: ScheduleExportFormatV1,
    pub person_id: Option<PersonId>,
    pub sha256: String,
    pub byte_count: u32,
    pub sample: String,

    pub sample_truncated: bool,
}
impl ScheduleExportPreviewV1 {
    /// Actual typed preview storage, excluding allocator overhead and transport-owned copies.
    #[must_use]
    pub fn retained_bytes(&self) -> Option<usize> {
        let Self {
            schema_version: _,
            source,
            library_revision: _,
            format: _,
            person_id: _,
            sha256,
            byte_count: _,
            sample,
            sample_truncated: _,
        } = self;
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
        let eutheto_types::RecipientSolverV1 {
            backend_id,
            recorded_version,
        } = solver;
        let bytes = [
            accepted_result_checksum,
            verification_report_checksum,
            source_document_hash,
            backend_id,
            sha256,
            sample,
        ]
        .into_iter()
        .try_fold(size_of::<Self>(), |total, value| {
            total.checked_add(value.capacity())
        })?;
        bytes.checked_add(recorded_version.as_ref().map_or(0, String::capacity))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleExportPublishedV1 {
    pub schema_version: u32,
    pub solution_id: SolutionId,
    pub source_revision: Revision,
    pub current_revision: Revision,
    pub format: ScheduleExportFormatV1,
    pub person_id: Option<PersonId>,
    pub sha256: String,
    pub byte_count: u32,
}

/// Native-owned complete encoded output; no preparation clock/token or source tree is retained.
pub struct CapturedScheduleExport {
    publication: CapturedPublication,
    format: ScheduleExportFormatV1,
    person_id: Option<PersonId>,
    bytes: Vec<u8>,
    sha256: String,
}

impl CapturedScheduleExport {
    #[must_use]
    pub fn metadata(&self) -> &RecipientReportMetadataV1 {
        &self.publication.metadata
    }
    #[must_use]
    pub fn format(&self) -> ScheduleExportFormatV1 {
        self.format
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// # Errors
    /// Refuses an established exact-source native alarm.
    pub fn admit(&self) -> Result<(), AppError> {
        self.publication.admit()
    }
    /// # Errors
    /// Refuses interruption, retained-capacity overflow or an established source alarm.
    pub fn retained_bytes(&self, control: &OperationControl) -> Result<usize, AppError> {
        self.admit()?;
        let artifact = self
            .bytes
            .capacity()
            .checked_add(self.sha256.capacity())
            .ok_or_else(solution_contract_error)?;
        self.publication
            .retained_bytes(control, size_of::<Self>(), artifact)
    }
    /// Publishes the retained artifact under its captured library/head lease.
    /// # Errors
    /// Refuses missing stale consent, changed revisions, alarms, interruption and unsafe output.
    pub async fn publish(
        self,
        destination: PathBuf,
        allow_stale: bool,
        control: OperationControl,
    ) -> Result<ScheduleExportPublishedV1, AppError> {
        self.admit()?;
        let metadata = &self.publication.metadata;
        let result = ScheduleExportPublishedV1 {
            schema_version: 1,
            solution_id: metadata.source.solution_id,
            source_revision: metadata.source.scenario_revision,
            current_revision: metadata.current_revision,
            format: self.format,
            person_id: self.person_id,
            sha256: self.sha256,
            byte_count: u32::try_from(self.bytes.len()).map_err(|_| solution_contract_error())?,
        };
        self.publication
            .publish(destination, allow_stale, control, move |path, control| {
                let text = std::str::from_utf8(&self.bytes).map_err(|_| {
                    eutheto_export::ExportError::InvalidModel(
                        "The captured export is not UTF-8.".to_owned(),
                    )
                })?;
                prepare_text_atomic_controlled(path, text, control)
            })
            .await?;
        Ok(result)
    }
}

impl EuthetoApp {
    /// Captures complete terminal/checkpoint CSV/calendar bytes and an explicitly partial sample.
    /// # Errors
    /// Refuses invalid scope, unverified sources, unsupported formats, limits and interruption.
    pub async fn prepare_schedule_export(
        &self,
        request: ScheduleExportRequestV1,
        control: OperationControl,
    ) -> Result<(CapturedScheduleExport, ScheduleExportPreviewV1), AppError> {
        if request.schema_version != 1 {
            return Err(solution_reference_error("/schemaVersion"));
        }
        if request.person_id.is_some() && request.format != ScheduleExportFormatV1::Ics {
            return Err(validation_error(
                "solution.export_person_invalid",
                "/personId",
                "A person filter is only valid for calendar export.",
            ));
        }
        let control = recipient_control(control, &self.monotonic_clock)?;
        let app = self.clone();
        tokio::spawn(async move { app.capture_schedule_export(request, control).await })
            .await
            .map_err(join_error)?
    }

    async fn capture_schedule_export(
        &self,
        request: ScheduleExportRequestV1,
        control: OperationControl,
    ) -> Result<(CapturedScheduleExport, ScheduleExportPreviewV1), AppError> {
        let _slot = super::manual::controlled_wait(
            Arc::clone(&self.accepted_query_slots).acquire_owned(),
            &control,
        )
        .await?
        .map_err(|_| solution_contract_error())?;
        let mut sources = Vec::with_capacity(1);
        let captured = async {
            let library_revision = self
                .store
                .library_metadata_snapshot()
                .await
                .map_err(store_error)?
                .revision;
            control.check().map_err(operation_interrupted)?;
            let mutation =
                super::manual::controlled_wait(self.scenario_lock(request.scenario_id), &control)
                    .await?;
            let guard = super::manual::controlled_wait(mutation.lock_owned(), &control).await?;
            let source = self
                .load_recipient_source(request.scenario_id, request.solution_id, &control)
                .await?;
            sources.push(self.recipient_source_custody(&source).await?);
            admit(&sources)?;
            control.check().map_err(operation_interrupted)?;
            let head = self.current_scenario_revision(request.scenario_id).await?;
            if head != request.expected_revision {
                return Err(solution_reference_error("/expectedRevision"));
            }
            drop(guard);
            control.check().map_err(operation_interrupted)?;
            let packs = Arc::clone(&self.pack_registry);
            let projection_control = control.clone();
            let created_at = self.clock.now();
            let built = tokio::task::spawn_blocking(move || {
                build_export(
                    &source,
                    &request,
                    &packs,
                    library_revision,
                    head,
                    created_at,
                    &projection_control,
                )
            })
            .await
            .map_err(join_error)??;
            control.check().map_err(operation_interrupted)?;
            let after = self
                .store
                .library_metadata_snapshot()
                .await
                .map_err(store_error)?
                .revision;
            if after != library_revision {
                return Err(solution_reference_error("/libraryRevision"));
            }
            control.check().map_err(operation_interrupted)?;
            Ok(built)
        }
        .await;
        admit(&sources)?;
        let (bytes, metadata, preview) = captured?;
        Ok((
            CapturedScheduleExport {
                publication: CapturedPublication {
                    store: Arc::clone(&self.store),
                    metadata,
                    sources,
                },
                format: preview.format,
                person_id: preview.person_id,
                sha256: preview.sha256.clone(),
                bytes,
            },
            preview,
        ))
    }
}

fn build_export(
    source: &AcceptedQuerySource,
    request: &ScheduleExportRequestV1,
    packs: &DomainPackRegistry,
    library_revision: Revision,
    head: Revision,
    created_at: Rfc3339Timestamp,
    control: &OperationControl,
) -> Result<(Vec<u8>, RecipientReportMetadataV1, ScheduleExportPreviewV1), AppError> {
    control.check().map_err(operation_interrupted)?;
    let (document, input, accepted, origin) = source.recipient_parts();
    let document_limit =
        usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES).map_err(|_| solution_contract_error())?;
    bounded_json_size(document, document_limit).map_err(|_| solution_contract_error())?;
    let limit =
        usize::try_from(PORTABLE_LIMITS.max_json_bytes).map_err(|_| solution_contract_error())?;
    validate_recipient_bindings(document, input, accepted, packs)?;
    let projection = match source {
        AcceptedQuerySource::Terminal(stored) => {
            bounded_json_size(&stored.portable, limit).map_err(|_| solution_contract_error())?;
            reverify_accepted_solution(stored, packs, control)?;
            WorkforceExportSource::Terminal(&stored.portable)
        }
        AcceptedQuerySource::Checkpoint(document, checkpoint) => {
            bounded_json_size(checkpoint, limit).map_err(|_| solution_contract_error())?;
            reverify_checkpoint_solution(document, checkpoint, packs, control)?;
            WorkforceExportSource::Checkpoint(checkpoint)
        }
    };
    let bytes = match request.format {
        ScheduleExportFormatV1::AssignmentsCsv => encode_csv(&accepted.solution, control)?,
        ScheduleExportFormatV1::SummaryCsv => {
            encode_summary_csv(document, projection, head.value(), control)?
        }
        ScheduleExportFormatV1::Ics => encode_ics(
            document,
            projection,
            request.person_id,
            head.value(),
            created_at,
            control,
        )?,
    };
    control.check().map_err(operation_interrupted)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| solution_contract_error())?;
    let mut sample_end = text.len().min(MAX_SCHEDULE_EXPORT_SAMPLE_BYTES);
    while !text.is_char_boundary(sample_end) {
        sample_end -= 1;
    }
    let header = recipient_source(input, accepted, origin, Some(head));
    let preview = ScheduleExportPreviewV1 {
        schema_version: 1,
        source: header.clone(),
        library_revision,
        format: request.format,
        person_id: request.person_id,
        sha256: sha256_hex(&bytes),
        byte_count: u32::try_from(bytes.len()).map_err(|_| solution_contract_error())?,
        sample: text[..sample_end].to_owned(),
        sample_truncated: sample_end != text.len(),
    };
    control.check().map_err(operation_interrupted)?;
    Ok((
        bytes,
        recipient_metadata(source, header, library_revision, head),
        preview,
    ))
}
