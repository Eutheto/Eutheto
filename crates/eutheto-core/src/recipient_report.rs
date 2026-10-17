//! Immutable recipient preparation and revision-leased publication; window custody stays native.
use super::result_publication::{CapturedPublication, SourceCustody, admit};
use super::{
    AcceptedQuerySource, EuthetoApp, available_pack, domain_interruption, export_error, join_error,
    operation_interrupted, same_accepted_original, solution_contract_error,
    solution_reference_error, solution_verification_failed, solution_verification_mismatch,
    store_error, unsupported_project_pack, validation_error,
};
use eutheto_domain_api::DomainPackRegistry;
use eutheto_domain_ir::{AcceptedResult, RunBaselineSelection, RunInput};
use eutheto_export::{EncodedRecipientHtml, encode_recipient_html, recipient_renderer};
use eutheto_types::{
    AppError, DurationMillis, MonotonicClock, OperationControl, ParentSolveBudget,
    RecipientContentV1, RecipientLabelV1, RecipientOriginV1, RecipientOriginalAdmissionV1,
    RecipientProfileV1, RecipientReportFormat, RecipientReportOptionsV1, RecipientReportV1,
    RecipientSolverV1, RecipientSourceV1, RecipientVerificationV1, ResourceRef, Revision,
    Rfc3339Timestamp, ScenarioDocument, ScenarioId, SolutionId, SolveRunId,
};
use eutheto_workforce::{
    assignment_rules::{WorkforceAdmissionBudget, workforce_baseline_reference},
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::WorkforceEntity,
    recipient_report::{
        RecipientAcceptedSource, RecipientManualSource, build_workforce_recipient_report,
    },
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

const PREPARATION_MILLISECONDS: u64 = 30_000;

pub(super) fn recipient_control(
    control: OperationControl,
    clock: &Arc<dyn MonotonicClock>,
) -> Result<OperationControl, AppError> {
    match control {
        OperationControl::Cancellation(cancellation) => Ok(OperationControl::Solve(
            ParentSolveBudget::new(
                DurationMillis::new(PREPARATION_MILLISECONDS)
                    .map_err(|_| solution_contract_error())?,
                Arc::clone(clock),
                cancellation,
            )
            .map_err(|_| solution_contract_error())?
            .phase_view(),
        )),
        OperationControl::Solve(view) => {
            if view.snapshot().remaining_milliseconds.value() > PREPARATION_MILLISECONDS {
                return Err(solution_reference_error("/operation/budget"));
            }
            Ok(OperationControl::Solve(view))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipientReportRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub solution_id: SolutionId,
    pub expected_revision: Revision,
    pub options: RecipientReportOptionsV1,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipientReportMetadataV1 {
    pub schema_version: u32,
    pub source: RecipientSourceV1,
    pub library_revision: Revision,
    pub current_revision: Revision,
    pub run_id: SolveRunId,
    pub run_input_checksum: String,
    pub run_manifest_checksum: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipientHtmlPublishedV1 {
    pub schema_version: u32,
    pub solution_id: SolutionId,
    pub source_revision: Revision,
    pub current_revision: Revision,
    pub model_sha256: String,
    pub html_sha256: String,
}

/// Owned approved bytes, never reconstructed from a preview submitted by a client.
/// This object deliberately retains no preparation token, deadline or scenario document.
pub struct CapturedRecipientReport {
    publication: CapturedPublication,
    encoded: EncodedRecipientHtml,
}

impl CapturedRecipientReport {
    #[must_use]
    pub fn metadata(&self) -> &RecipientReportMetadataV1 {
        &self.publication.metadata
    }

    #[must_use]
    pub fn canonical_model_bytes(&self) -> &[u8] {
        self.encoded.canonical_model_bytes()
    }

    #[must_use]
    pub fn html_bytes(&self) -> &[u8] {
        self.encoded.as_bytes()
    }

    #[must_use]
    pub fn model_sha256(&self) -> &str {
        self.encoded.model_sha256()
    }

    #[must_use]
    pub fn html_sha256(&self) -> &str {
        self.encoded.html_sha256()
    }

    /// Checks the retained live source without extending its preparation operation.
    ///
    /// # Errors
    /// Returns an established native source alarm.
    pub fn admit(&self) -> Result<(), AppError> {
        self.publication.admit()
    }

    /// Charges owned buffers/metadata and pinned source state, including bounded late growth.
    /// Shared source allocations are conservatively charged per handle. Independently
    /// application-owned Store/clock/root cancellation and allocator overhead/RSS are excluded.
    ///
    /// # Errors
    /// Refuses interrupted/overflowing accounting or a native source alarm.
    pub fn retained_bytes(&self, control: &OperationControl) -> Result<usize, AppError> {
        self.publication.retained_bytes(
            control,
            size_of::<Self>(),
            self.encoded.retained_bytes() - size_of::<EncodedRecipientHtml>(),
        )
    }

    /// Stages the approved HTML and commits under the captured library/head lease.
    /// Interactive callers supply a fresh create control after human review; finite CLI callers
    /// retain their preparation parent budget through publication.
    ///
    /// # Errors
    /// Refuses missing stale consent, changed revisions, native alarms, interruption or unsafe output.
    pub async fn publish_html(
        self,
        destination: PathBuf,
        allow_stale: bool,
        control: OperationControl,
    ) -> Result<RecipientHtmlPublishedV1, AppError> {
        self.admit()?;
        let metadata = &self.publication.metadata;
        let result = RecipientHtmlPublishedV1 {
            schema_version: 1,
            solution_id: metadata.source.solution_id,
            source_revision: metadata.source.scenario_revision,
            current_revision: metadata.current_revision,
            model_sha256: self.encoded.model_sha256().to_owned(),
            html_sha256: self.encoded.html_sha256().to_owned(),
        };
        self.publication
            .publish(destination, allow_stale, control, move |path, control| {
                self.encoded.prepare_destination(path, control)
            })
            .await?;
        Ok(result)
    }
}

impl EuthetoApp {
    /// Captures one exact accepted source and its privacy-filtered presentation.
    /// An inherited solve control must have at most30 seconds remaining; cancellation-only
    /// callers receive that bound before any source/admission wait.
    ///
    /// # Errors
    /// Refuses invalid/stale capture context, failed native/source verification or bounded work.
    pub async fn prepare_recipient_report(
        &self,
        request: RecipientReportRequestV1,
        control: OperationControl,
    ) -> Result<(CapturedRecipientReport, RecipientReportV1), AppError> {
        if request.schema_version != 1 {
            return Err(solution_reference_error("/schemaVersion"));
        }
        let control = recipient_control(control, &self.monotonic_clock)?;
        let app = self.clone();
        tokio::spawn(async move { app.capture_recipient_report(request, control).await })
            .await
            .map_err(join_error)?
    }

    // Keep owned settlement and its exact-source alarm error funnel in one scope.
    #[allow(clippy::too_many_lines)]
    async fn capture_recipient_report(
        &self,
        request: RecipientReportRequestV1,
        control: OperationControl,
    ) -> Result<(CapturedRecipientReport, RecipientReportV1), AppError> {
        let _slot = super::manual::controlled_wait(
            Arc::clone(&self.accepted_query_slots).acquire_owned(),
            &control,
        )
        .await?
        .map_err(|_| solution_contract_error())?;
        let mut sources = Vec::with_capacity(2);
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
            let mutation_guard =
                super::manual::controlled_wait(mutation.lock_owned(), &control).await?;
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
            control.check().map_err(operation_interrupted)?;
            let original = if request.options.include_changes {
                if let Some(baseline) = source
                    .recipient_parts()
                    .1
                    .objective_context
                    .as_ref()
                    .and_then(|value| value.baseline.as_ref())
                {
                    match self
                        .load_solution(
                            request.scenario_id,
                            baseline.original.accepted_result.solution_id,
                        )
                        .await
                    {
                        Ok(original) => {
                            if !same_accepted_original(&baseline.original, &original.portable) {
                                return Err(solution_verification_mismatch());
                            }
                            let original = AcceptedQuerySource::Terminal(original);
                            sources.push(self.recipient_source_custody(&original).await?);
                            admit(&sources)?;
                            Some(original)
                        }
                        Err(AppError::NotFound(ResourceRef::Solution(_))) => None,
                        Err(error) => return Err(error),
                    }
                } else {
                    None
                }
            } else {
                None
            };
            control.check().map_err(operation_interrupted)?;
            let manual = if request.options.include_changes {
                self.recipient_manual_source(&source, head, &control)
                    .await?
            } else {
                None
            };
            drop(mutation_guard);
            control.check().map_err(operation_interrupted)?;
            let packs = Arc::clone(&self.pack_registry);
            let projection_control = control.clone();
            let generated_at = self.clock.now();
            // Move, never clone, the complete source trees; one owner retains the query slot.
            let (report, encoded, metadata) = tokio::task::spawn_blocking(move || {
                build_report(
                    &source,
                    original.as_ref(),
                    manual.as_ref(),
                    &packs,
                    request.options,
                    library_revision,
                    head,
                    generated_at,
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
            Ok((report, encoded, metadata))
        }
        .await;
        admit(&sources)?;
        let (report, encoded, metadata) = captured?;
        Ok((
            CapturedRecipientReport {
                publication: CapturedPublication {
                    store: Arc::clone(&self.store),
                    metadata,
                    sources,
                },
                encoded,
            },
            report,
        ))
    }

    pub(super) async fn load_recipient_source(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
        control: &OperationControl,
    ) -> Result<AcceptedQuerySource, AppError> {
        match self.load_solution(scenario_id, solution_id).await {
            Ok(stored) => Ok(AcceptedQuerySource::Terminal(stored)),
            Err(AppError::NotFound(ResourceRef::Solution(_))) => {
                control.check().map_err(operation_interrupted)?;
                let (document, checkpoint) = self
                    .store
                    .load_verified_checkpoint_for_solution(solution_id)
                    .await
                    .map_err(store_error)?
                    .ok_or(AppError::NotFound(ResourceRef::Solution(solution_id)))?;
                if document.scenario_id != scenario_id
                    || checkpoint.run_input.scenario_id != scenario_id
                {
                    return Err(solution_reference_error("/scenarioId"));
                }
                Ok(AcceptedQuerySource::Checkpoint(document, checkpoint))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn recipient_source_custody(
        &self,
        source: &AcceptedQuerySource,
    ) -> Result<SourceCustody, AppError> {
        let (_, input, _, _) = source.recipient_parts();
        let ordinary = self.ordinary_run_handle(input.scenario_id, input.run_id)?;
        if let Some(handle) = &ordinary {
            handle.validate_input(input)?;
        }
        let counterfactual = self
            .counterfactual
            .diagnostic_run_handle(
                input.scenario_id,
                Revision::new(input.scenario_revision),
                input.run_id,
            )
            .await;
        if counterfactual
            .as_ref()
            .is_some_and(|handle| !handle.matches_input(input))
        {
            return Err(solution_reference_error("/source"));
        }
        Ok(SourceCustody {
            ordinary,
            counterfactual,
        })
    }

    async fn recipient_manual_source(
        &self,
        source: &AcceptedQuerySource,
        head: Revision,
        control: &OperationControl,
    ) -> Result<Option<(ScenarioDocument, Revision)>, AppError> {
        let (document, input, _, _) = source.recipient_parts();
        let Some(RunBaselineSelection::ManualSelection { entity, .. }) = input
            .objective_context
            .as_ref()
            .and_then(|value| value.baseline.as_ref())
            .map(|baseline| &baseline.selection)
        else {
            return Ok(None);
        };
        let mut budget = WorkforceAdmissionBudget::new(Some(control));
        budget
            .steps(
                u64::try_from(document.domain.entities.len())
                    .map_err(|_| solution_contract_error())?,
            )
            .map_err(super::manual::manual_integrity_error)?;
        let (id, value) = super::manual::find_manual_value(document)
            .map_err(store_error)?
            .ok_or_else(solution_verification_mismatch)?;
        if &workforce_baseline_reference(*id, true).map_err(|_| solution_verification_mismatch())?
            != entity
        {
            return Err(solution_verification_mismatch());
        }
        budget
            .reserve_json(value, 1)
            .map_err(super::manual::manual_integrity_error)?;
        let WorkforceEntity::ManualSelection(manual) =
            serde_json::from_value(value.clone()).map_err(|_| solution_verification_mismatch())?
        else {
            return Err(solution_verification_mismatch());
        };
        let receipt = self
            .store
            .lookup_private_history(
                document.scenario_id,
                head,
                manual.admission_command_id,
                control.clone(),
                None,
            )
            .await
            .map_err(store_error)?
            .entry;
        let Some(receipt) = receipt else {
            return Ok(None);
        };
        let original = super::manual::authenticate_owned_manual_receipt(
            receipt,
            *id,
            value,
            &manual,
            document.scenario_id,
            &mut budget,
        )?;
        Ok(Some((original, manual.admission_source_revision)))
    }
}

impl AcceptedQuerySource {
    pub(super) fn recipient_parts(
        &self,
    ) -> (
        &ScenarioDocument,
        &RunInput,
        &AcceptedResult,
        RecipientOriginV1,
    ) {
        match self {
            Self::Terminal(stored) => (
                &stored.document,
                &stored.portable.run_input,
                &stored.portable.accepted_result,
                RecipientOriginV1::StoredTerminal,
            ),
            Self::Checkpoint(document, checkpoint) => (
                document,
                &checkpoint.run_input,
                &checkpoint.accepted_result,
                RecipientOriginV1::StoredCheckpoint,
            ),
        }
    }
}

fn validate_source(
    source: &AcceptedQuerySource,
    packs: &DomainPackRegistry,
    control: &OperationControl,
) -> Result<(), AppError> {
    control.check().map_err(operation_interrupted)?;
    match source {
        AcceptedQuerySource::Terminal(stored) => {
            stored
                .portable
                .validate()
                .map_err(|_| solution_verification_mismatch())?;
            crate::native_observations::validate_fingerprint(
                &stored.portable.run_input,
                &stored.portable.run_manifest,
            )
            .map_err(|_| solution_verification_mismatch())?;
        }
        AcceptedQuerySource::Checkpoint(_, checkpoint) => checkpoint
            .validate()
            .map_err(|_| solution_verification_mismatch())?,
    }
    let (document, input, accepted, _) = source.recipient_parts();
    validate_recipient_bindings(document, input, accepted, packs)
}

pub(super) fn validate_recipient_bindings(
    document: &ScenarioDocument,
    input: &RunInput,
    accepted: &AcceptedResult,
    packs: &DomainPackRegistry,
) -> Result<(), AppError> {
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID
        || available_pack(document, packs).is_none()
    {
        return Err(unsupported_project_pack(&document.domain_pack));
    }
    if input.pack_id != document.domain_pack.id
        || input.pack_schema_version != document.domain_pack.schema_version
        || input.scenario_id != document.scenario_id
        || accepted.solution.scenario_id != document.scenario_id
        || input.scenario_revision != accepted.solution.scenario_revision
        || input.snapshot_document_hash != accepted.verification.document_hash
        || input.model_hash != accepted.verification.planning_model_hash
    {
        return Err(solution_verification_mismatch());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    source: &AcceptedQuerySource,
    original: Option<&AcceptedQuerySource>,
    manual: Option<&(ScenarioDocument, Revision)>,
    packs: &DomainPackRegistry,
    options: RecipientReportOptionsV1,
    library_revision: Revision,
    head: Revision,
    generated_at: Rfc3339Timestamp,
    control: &OperationControl,
) -> Result<
    (
        RecipientReportV1,
        EncodedRecipientHtml,
        RecipientReportMetadataV1,
    ),
    AppError,
> {
    validate_source(source, packs, control)?;
    if let Some(original) = original {
        validate_source(original, packs, control)?;
    }
    let (document, input, accepted, origin) = source.recipient_parts();
    let original_source = original.map(|source| {
        let (document, input, accepted, _) = source.recipient_parts();
        RecipientAcceptedSource {
            document,
            accepted,
            objective_context: input.objective_context.as_ref(),
        }
    });
    let manual_source = manual.map(|(document, revision)| RecipientManualSource {
        document,
        revision: *revision,
    });
    let mut report = recipient_model(
        document,
        input,
        accepted,
        origin,
        Some(head),
        original_source,
        manual_source,
        options,
        generated_at,
        control,
    )?;
    if original.is_some() {
        let RecipientContentV1::Workforce(content) = &mut report.content;
        for comparison in content.changes.iter_mut().flatten() {
            if let Some(original) = &mut comparison.original {
                original.admission = RecipientOriginalAdmissionV1::FreshlyReadmittedNative;
            }
        }
    }
    let metadata = recipient_metadata(source, report.source.clone(), library_revision, head);
    let encoded = encode_recipient_html(&report, control).map_err(|error| export_error(&error))?;
    Ok((report, encoded, metadata))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recipient_model(
    document: &ScenarioDocument,
    input: &RunInput,
    accepted: &AcceptedResult,
    origin: RecipientOriginV1,
    observed_current_revision: Option<Revision>,
    original: Option<RecipientAcceptedSource<'_>>,
    manual: Option<RecipientManualSource<'_>>,
    options: RecipientReportOptionsV1,
    generated_at: Rfc3339Timestamp,
    control: &OperationControl,
) -> Result<RecipientReportV1, AppError> {
    // Reject before copying a host-authored field; the encoder also bounds escaped framing.
    if options.include_authored_labels
        && document.metadata.title.len() > eutheto_types::MAX_RECIPIENT_REPORT_BYTES
    {
        return Err(validation_error(
            "recipient.report_too_large",
            "/title",
            "The report exceeds the supported size limit.",
        ));
    }
    let content = build_workforce_recipient_report(
        document,
        accepted,
        input.objective_context.as_ref(),
        original,
        manual,
        options,
        control,
    )
    .map_err(|error| domain_interruption(&error).unwrap_or_else(solution_verification_failed))?;
    let source = recipient_source(input, accepted, origin, observed_current_revision);
    Ok(RecipientReportV1 {
        format: RecipientReportFormat::EuthetoShareResult,
        schema_version: 1,
        profile: RecipientProfileV1::ParticipantFriendly,
        generated_at,
        application_version: env!("CARGO_PKG_VERSION").to_owned(),
        renderer: recipient_renderer(),
        title: if options.include_authored_labels {
            RecipientLabelV1::Authored(document.metadata.title.clone())
        } else {
            RecipientLabelV1::Generated("Shared plan".to_owned())
        },
        plan_version: format!("Plan from revision {}", input.scenario_revision),
        locale: document.settings.locale.clone(),
        time_zone: document.settings.time_zone.clone(),
        horizon: document.settings.horizon,
        source,
        privacy: options,
        content: RecipientContentV1::Workforce(content),
    })
}

pub(super) fn recipient_source(
    input: &RunInput,
    accepted: &AcceptedResult,
    origin: RecipientOriginV1,
    observed_current_revision: Option<Revision>,
) -> RecipientSourceV1 {
    RecipientSourceV1 {
        scenario_id: input.scenario_id,
        scenario_revision: Revision::new(input.scenario_revision),
        solution_id: accepted.solution.solution_id,
        result_schema_version: accepted.schema_version,
        accepted_result_checksum: accepted.checksum.clone(),
        verification_report_checksum: accepted.verification.checksum.clone(),
        source_document_hash: accepted.verification.document_hash.clone(),
        origin,
        observed_current_revision,
        verification: RecipientVerificationV1::VerifiedFeasible,
        solver: RecipientSolverV1 {
            backend_id: input.backend_id.to_string(),
            recorded_version: Some(input.backend_version.clone()),
        },
    }
}

pub(super) fn recipient_metadata(
    source: &AcceptedQuerySource,
    header: RecipientSourceV1,
    library_revision: Revision,
    head: Revision,
) -> RecipientReportMetadataV1 {
    let input = source.recipient_parts().1;
    RecipientReportMetadataV1 {
        schema_version: 1,
        source: header,
        library_revision,
        current_revision: head,
        run_id: input.run_id,
        run_input_checksum: input.checksum.clone(),
        run_manifest_checksum: match source {
            AcceptedQuerySource::Terminal(stored) => {
                Some(stored.portable.run_manifest.checksum.clone())
            }
            AcceptedQuerySource::Checkpoint(_, _) => None,
        },
    }
}
