//! Native diagnostic copy and private JSON publication actions.
//!
//! This module owns the application boundary for immutable diagnostic evidence.  Store remains
//! the transaction authority; this layer supplies only authenticated pack preparation and the
//! normal command journal payload.

use super::{
    AppCommandResult, EuthetoApp, SOLUTION_API_SCHEMA_VERSION, available_pack, command_store_error,
    domain_interruption, explanation_render_error, export_error, id_error, join_error,
    operation_interrupted, project_summary, solution_contract_error, solution_reference_error,
    store_error, validation_error,
};
use crate::counterfactual::CounterfactualPublicationHandle;
use eutheto_command::apply_command_with_registry;
use eutheto_domain_api::{DiagnosticCopyContext, DomainPackError};
use eutheto_domain_ir::{
    AssumptionGroupId, CounterfactualConditionV1, DiagnosticInfeasibilityProofV1,
    EvidenceRenderRequestV2, EvidenceRenderResultV1, ExplanationEvidencePayloadV2,
    ExplanationEvidenceV2, ExplanationRequestSubjectV1, ExplanationRequestV1,
    InfeasibilityEvidenceV1, OriginalInfeasibilityObservationV1, RunInfeasibilityEvidenceV1,
    RunManifest,
};
use eutheto_export::{PreparedPublication, encode_json_controlled, prepare_text_atomic_controlled};
use eutheto_store::{
    CommandWrite, JournalWrite, StoreDiagnosticCopyRequestV1, StoreError,
    StoredInfeasibilitySourceV1,
};
use eutheto_types::{
    ActorRef, AppError, CommandBatch, CommandEnvelope, CommandId, CommandSource, OperationControl,
    OperationId, ProjectListItemV1, RequestId, ResourceRef, Revision, ScenarioCommand, ScenarioId,
    SolveRunId,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Current native diagnostic-copy request schema.
pub const DIAGNOSTIC_COPY_API_SCHEMA_VERSION: u32 = 1;
/// Current native diagnostic-export request schema.
pub const DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION: u32 = 1;
/// Current private diagnostic JSON report schema.
pub const DIAGNOSTIC_REPORT_SCHEMA_VERSION: u32 = 2;
/// Maximum encoded diagnostic report size, including JSON escaping and envelope fields.
pub const MAX_DIAGNOSTIC_REPORT_BYTES: usize = 2 * 1024 * 1024;
const DIAGNOSTIC_REPORT_FORMAT: &str = "eutheto/infeasibility-report";

/// Closed, immutable authority references for one native reversible diagnostic copy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticCopyRequestV1 {
    pub schema_version: u32,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub source_revision: Revision,
    pub expected_library_revision: Revision,
    pub run_id: SolveRunId,
    pub run_manifest_checksum: String,
    pub evidence_checksum: String,
    pub group_id: AssumptionGroupId,
}

impl DiagnosticCopyRequestV1 {
    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != DIAGNOSTIC_COPY_API_SCHEMA_VERSION {
            return Err(validation_error(
                "diagnostic.copy_schema_unsupported",
                "/schemaVersion",
                "The diagnostic copy request schema is unsupported.",
            ));
        }
        validate_checksum(&self.run_manifest_checksum, "/runManifestChecksum")?;
        validate_checksum(&self.evidence_checksum, "/evidenceChecksum")?;
        Ok(())
    }
}

/// Lean identities returned after the copy transaction and normal command journal commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticCopyResultV1 {
    pub schema_version: u32,
    pub project: ProjectListItemV1,
    pub command_id: CommandId,
    pub library_revision: Revision,
}

/// Closed immutable authority references for one private diagnostic JSON export.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticExportRequestV1 {
    pub schema_version: u32,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub source_revision: Revision,
    pub run_id: SolveRunId,
    pub run_manifest_checksum: String,
    pub evidence_checksum: String,
}

impl DiagnosticExportRequestV1 {
    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION {
            return Err(validation_error(
                "diagnostic.export_schema_unsupported",
                "/schemaVersion",
                "The diagnostic export request schema is unsupported.",
            ));
        }
        validate_checksum(&self.run_manifest_checksum, "/runManifestChecksum")?;
        validate_checksum(&self.evidence_checksum, "/evidenceChecksum")?;
        Ok(())
    }
}

/// Stable metadata retained alongside a staged private diagnostic publication.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticExportMetadataV1 {
    pub schema_version: u32,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    pub source_revision: Revision,
    /// Current native revision observed during preparation, distinct from the immutable source.
    pub observed_current_revision: Revision,
    pub run_id: SolveRunId,
    pub run_manifest_checksum: String,
    pub evidence_checksum: String,
}

/// Explicit local JSON whitelist.  It contains no source wrapper, scenario, asset, path, audit,
/// credential, device, database, or raw solver-log state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticReport {
    pub format: String,
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub source_revision: Revision,
    pub run_id: SolveRunId,
    pub run_input_checksum: String,
    pub source_document_hash: String,
    pub run_manifest_checksum: String,
    pub evidence_checksum: String,
    pub manifest: RunManifest,
    pub original: OriginalInfeasibilityObservationV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<CounterfactualConditionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<DiagnosticInfeasibilityProofV1>,
    pub infeasibility: InfeasibilityEvidenceV1,
    pub rendered: EvidenceRenderResultV1,
}

/// A validated, immutable diagnostic report captured before native destination selection.
///
/// The full source document is dropped after capture; only the bounded encoded whitelist,
/// metadata, operation control and live native publication handles remain.
pub struct CapturedDiagnosticExport {
    metadata: DiagnosticExportMetadataV1,
    report: String,
    control: OperationControl,
    ordinary: Option<super::OrdinaryPublicationHandle>,
    counterfactual: Option<CounterfactualPublicationHandle>,
}

impl CapturedDiagnosticExport {
    /// Returns identities and the exact revision observed while capturing this report.
    #[must_use]
    pub fn metadata(&self) -> &DiagnosticExportMetadataV1 {
        &self.metadata
    }

    /// Re-admits native alarm custody before honoring cancellation.
    ///
    /// # Errors
    ///
    /// Returns the retained native alarm or an operation interruption.
    pub fn admit(&self) -> Result<(), AppError> {
        admit_diagnostic_sources(self.ordinary.as_ref(), self.counterfactual.as_ref())?;
        self.control.check().map_err(operation_interrupted)
    }

    /// Stages the captured report at one native-only destination without re-reading its source.
    ///
    /// # Errors
    ///
    /// Refuses revoked custody, cancellation, unsafe paths, or staging failures.
    pub fn prepare_destination(
        self,
        destination: &Path,
    ) -> Result<PreparedDiagnosticExport, AppError> {
        self.admit()?;
        let prepared =
            match prepare_text_atomic_controlled(destination, &self.report, &self.control) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.admit()?;
                    return Err(export_error(&error));
                }
            };
        if let Err(error) = self.admit() {
            drop(prepared);
            return Err(error);
        }
        Ok(PreparedDiagnosticExport {
            metadata: self.metadata,
            control: self.control,
            prepared,
            ordinary: self.ordinary,
            counterfactual: self.counterfactual,
        })
    }
}

/// A report staged in a private no-clobber destination.  The destination path and publication
/// handle are intentionally not serializable or exposed as wire authority.
pub struct PreparedDiagnosticExport {
    metadata: DiagnosticExportMetadataV1,
    control: OperationControl,
    prepared: PreparedPublication,
    ordinary: Option<super::OrdinaryPublicationHandle>,
    counterfactual: Option<CounterfactualPublicationHandle>,
}

impl PreparedDiagnosticExport {
    /// Returns immutable source/run/evidence identities for the staged report.
    #[must_use]
    pub fn metadata(&self) -> &DiagnosticExportMetadataV1 {
        &self.metadata
    }

    /// Publishes once; a successful no-clobber commit wins over later cancellation or alarms.
    ///
    /// # Errors
    ///
    /// Returns the retained source alarm, an operation interruption, or a publication failure.
    pub fn publish_controlled(self) -> Result<DiagnosticExportMetadataV1, AppError> {
        admit_diagnostic_sources(self.ordinary.as_ref(), self.counterfactual.as_ref())?;
        let result = self
            .control
            .check()
            .map_err(operation_interrupted)
            .and_then(|()| {
                self.prepared
                    .publish_controlled(&self.control)
                    .map_err(|error| export_error(&error))
            });
        if result.is_err() {
            admit_diagnostic_sources(self.ordinary.as_ref(), self.counterfactual.as_ref())?;
        }
        result.map(|()| self.metadata)
    }
}

impl EuthetoApp {
    // Keep Store's CAS admission, domain-pack mutation and command journal in one callback so
    // source custody stays adjacent to the irreversible transaction boundary.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn create_diagnostic_copy(
        &self,
        request: DiagnosticCopyRequestV1,
        control: OperationControl,
    ) -> Result<AppCommandResult, AppError> {
        request.validate()?;
        let ordinary = self.ordinary_run_handle(request.scenario_id, request.run_id)?;
        if let Some(handle) = &ordinary {
            handle.validate_diagnostic_scope(
                request.scenario_id,
                request.source_revision,
                request.run_id,
            )?;
        }
        let counterfactual = self
            .counterfactual
            .diagnostic_run_handle(request.scenario_id, request.source_revision, request.run_id)
            .await;
        if ordinary.is_some() || counterfactual.is_some() {
            let references = validate_diagnostic_source_references(
                self,
                request.scenario_id,
                request.source_revision,
                request.run_id,
                &request.run_manifest_checksum,
                &request.evidence_checksum,
            )
            .await;
            let (input, evidence) = match references {
                Ok(references) => references,
                Err(error) => {
                    if !is_diagnostic_reference_refusal(&error) {
                        admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                    }
                    return Err(error);
                }
            };
            if let Some(handle) = &ordinary
                && let Err(error) = handle.validate_input(&input)
            {
                handle.admit()?;
                return Err(error);
            }
            if let Some(handle) = &counterfactual
                && !handle.matches_input(&input)
            {
                handle.admit()?;
                return Err(solution_contract_error());
            }
            validate_diagnostic_copy_group(evidence.as_ref(), &request.group_id)?;
        }
        if let Err(reason) = control.check() {
            admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
            return Err(operation_interrupted(reason));
        }
        if ordinary.is_some() || counterfactual.is_some() {
            let actual_library_revision = match self.store.library_metadata_snapshot().await {
                Ok(snapshot) => snapshot.revision,
                Err(error) => {
                    admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                    return Err(store_error(error));
                }
            };
            if let Err(reason) = control.check() {
                admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                return Err(operation_interrupted(reason));
            }
            if actual_library_revision != request.expected_library_revision {
                return Err(store_error(StoreError::LibraryConflict {
                    expected: request.expected_library_revision,
                    actual: actual_library_revision,
                }));
            }
        }
        admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
        let source_admitted = Arc::new(AtomicBool::new(false));
        let callback_admitted = Arc::clone(&source_admitted);
        let callback_ordinary = ordinary.clone();
        let callback_counterfactual = counterfactual.clone();
        let command_id = CommandId::new(self.ids.as_ref()).map_err(id_error)?;
        let created_at = self.clock.now();
        let registry = Arc::clone(&self.pack_registry);
        let ids = Arc::clone(&self.ids);
        let normalize_control = control.clone();
        let callback_control = control.clone();
        let callback_command_id = command_id;
        let callback_created_at = created_at;
        let source_revision = request.source_revision;
        let outcome = self
            .store
            .create_diagnostic_copy(
                StoreDiagnosticCopyRequestV1 {
                    run_id: request.run_id,
                    scenario_id: request.scenario_id,
                    source_revision: request.source_revision,
                    run_manifest_checksum: request.run_manifest_checksum,
                    evidence_checksum: request.evidence_checksum,
                    group_id: request.group_id,
                    expected_library_revision: request.expected_library_revision,
                    request_id: request.request_id,
                },
                ids.clone(),
                created_at,
                control,
                move |original, copied, mapping| {
                    super::normalize_workforce_manual_copy(
                        original,
                        copied,
                        mapping,
                        mapping,
                        Some(&normalize_control),
                    )
                },
                move |source, destination, id_remap, group, condition| {
                    // Store has admitted the exact manifest, evidence, group and library CAS.
                    callback_admitted.store(true, Ordering::Release);
                    let admit = || {
                        admit_diagnostic_sources(
                            callback_ordinary.as_ref(),
                            callback_counterfactual.as_ref(),
                        )
                        .map_err(|_| StoreError::CommandApplication {
                            code: "diagnostic.source_not_publishable".to_owned(),
                            message: "The native diagnostic source cannot be published.".to_owned(),
                            field_path: None,
                        })
                    };
                    admit()?;
                    callback_control
                        .check()
                        .map_err(|_| StoreError::OperationCancelled)?;
                    let pack = registry
                        .require(&destination.domain_pack.id)
                        .map_err(pack_store_error)?;
                    let context = DiagnosticCopyContext {
                        source_revision: source_revision.value(),
                        destination,
                        id_remap,
                        group,
                        condition,
                        ids: ids.as_ref(),
                        control: &callback_control,
                    };
                    let batch = pack
                        .prepare_diagnostic_copy_changes(source, &context)
                        .map_err(pack_store_error)?;
                    let command = ScenarioCommand::ApplyBatch(CommandBatch {
                        label: batch.label,
                        commands: batch
                            .commands
                            .into_iter()
                            .map(ScenarioCommand::ApplyDomainCommand)
                            .collect(),
                    });
                    let envelope = CommandEnvelope {
                        command_id: callback_command_id,
                        scenario_id: destination.scenario_id,
                        expected_revision: Revision::INITIAL,
                        actor: ActorRef {
                            actor_id: None,
                            display_name: "Diagnostic copy".to_owned(),
                        },
                        source: CommandSource::System,
                        command,
                    };
                    let mut applied = apply_command_with_registry(
                        destination,
                        Revision::INITIAL,
                        &envelope,
                        &registry,
                        callback_control.cancellation(),
                    )
                    .map_err(|error| command_store_error(&error, Some(&envelope.command)))?;
                    applied.document.metadata.updated_at = callback_created_at;
                    callback_control
                        .check()
                        .map_err(|_| StoreError::OperationCancelled)?;
                    let command_json =
                        serde_json::to_value(&envelope.command).map_err(StoreError::Json)?;
                    let result = applied.result;
                    let inverse = result
                        .inverse
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(StoreError::Json)?;
                    admit()?;
                    Ok(CommandWrite {
                        document: applied.document,
                        journal: JournalWrite {
                            command_type: applied.command_type,
                            command: command_json,
                            command_id: envelope.command_id,
                            inverse,
                            actor: envelope.actor,
                            source: envelope.source,
                            summary: applied.summary,
                            created_at: callback_created_at,
                        },
                        output: result,
                    })
                },
            )
            .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                if source_admitted.load(Ordering::Acquire)
                    || !matches!(
                        &error,
                        StoreError::Conflict { .. } | StoreError::LibraryConflict { .. }
                    )
                {
                    admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                }
                return Err(store_error(error));
            }
        };
        let result = DiagnosticCopyResultV1 {
            schema_version: DIAGNOSTIC_COPY_API_SCHEMA_VERSION,
            project: project_summary(outcome.project.summary),
            command_id,
            library_revision: outcome.library_revision,
        };
        self.publish_scenario_events(
            result.project.scenario_id,
            request.request_id,
            &outcome.command.output,
        );
        Ok(AppCommandResult::DiagnosticCopy(result))
    }

    /// Captures, validates and renders one complete bounded diagnostic report.
    ///
    /// The returned capture is tied to this immutable source and retains only its encoded report,
    /// metadata and native publication handles; destination selection happens afterward.
    ///
    /// # Errors
    ///
    /// Refuses invalid references, revoked native custody, interrupted work, or an
    /// unavailable or oversized native diagnostic source/report.
    // Pin the source-owning privacy/admission pipeline once, without enlarging each caller.
    #[allow(clippy::too_many_lines)]
    pub async fn prepare_diagnostic_export(
        &self,
        request: DiagnosticExportRequestV1,
        control: OperationControl,
    ) -> Result<CapturedDiagnosticExport, AppError> {
        Box::pin(async move {
            request.validate()?;
            let ordinary = self.ordinary_run_handle(request.scenario_id, request.run_id)?;
            if let Some(handle) = &ordinary {
                handle.validate_diagnostic_scope(
                    request.scenario_id,
                    request.source_revision,
                    request.run_id,
                )?;
            }
            let counterfactual = self
                .counterfactual
                .diagnostic_run_handle(request.scenario_id, request.source_revision, request.run_id)
                .await;
            if ordinary.is_some() || counterfactual.is_some() {
                let references = validate_diagnostic_source_references(
                    self,
                    request.scenario_id,
                    request.source_revision,
                    request.run_id,
                    &request.run_manifest_checksum,
                    &request.evidence_checksum,
                )
                .await;
                let (input, _) = match references {
                    Ok(references) => references,
                    Err(error) => {
                        if !is_diagnostic_reference_refusal(&error) {
                            admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                        }
                        return Err(error);
                    }
                };
                if let Some(handle) = &ordinary
                    && let Err(error) = handle.validate_input(&input)
                {
                    handle.admit()?;
                    return Err(error);
                }
                if let Some(handle) = &counterfactual
                    && !handle.matches_input(&input)
                {
                    handle.admit()?;
                    return Err(solution_contract_error());
                }
                admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
            }
            if let Err(reason) = control.check() {
                admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                return Err(operation_interrupted(reason));
            }
            let mut source_admitted = false;
            let result = Box::pin(async {
                let explain_request = super::SolutionExplainRequestV1 {
                    schema_version: SOLUTION_API_SCHEMA_VERSION,
                    scenario_id: request.scenario_id,
                    request: ExplanationRequestV1::new(
                        ExplanationRequestSubjectV1::Infeasibility {
                            solve_run_id: request.run_id,
                            run_manifest_checksum: request.run_manifest_checksum.clone(),
                            conflict_id: None,
                        },
                    )
                    .map_err(|_| solution_contract_error())?,
                };
                let captured = match self
                    .capture_explanation(
                        &explain_request,
                        request.source_revision,
                        &control,
                        std::slice::from_ref(&ordinary),
                    )
                    .await
                {
                    Ok(captured) => captured,
                    Err(error) => {
                        if !is_diagnostic_reference_refusal(&error) {
                            admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                        }
                        return Err(error);
                    }
                };
                let super::CapturedExplanation::Infeasibility(source) = captured else {
                    return Err(solution_contract_error());
                };
                if let Err(error) = validate_captured_source(&request, &source) {
                    if !is_diagnostic_reference_refusal(&error) {
                        admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                    }
                    return Err(error);
                }
                if let Some(handle) = &ordinary
                    && let Err(error) = handle.validate_input(&source.input)
                {
                    handle.admit()?;
                    return Err(error);
                }
                if let Some(handle) = &counterfactual
                    && !handle.matches_input(&source.input)
                {
                    handle.admit()?;
                    return Err(solution_contract_error());
                }
                source_admitted = true;
                admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                crate::native_observations::validate_fingerprint(&source.input, &source.manifest)
                    .map_err(|_| solution_contract_error())?;
                let document = source.document;
                let objective_context = source.input.objective_context;
                let evidence = source
                    .evidence
                    .as_ref()
                    .ok_or_else(|| super::explanation_unavailable("infeasibility"))?;
                let render_request = EvidenceRenderRequestV2::new(
                    ExplanationEvidenceV2::new(ExplanationEvidencePayloadV2::Infeasibility {
                        infeasibility: evidence.infeasibility.clone(),
                        condition: evidence.condition.clone(),
                    })
                    .map_err(|_| solution_contract_error())?,
                )
                .map_err(|_| solution_contract_error())?;
                let registry = Arc::clone(&self.pack_registry);
                let render_control = control.clone();
                let rendered = tokio::task::spawn_blocking(move || {
                    render_control.check().map_err(operation_interrupted)?;
                    let pack = available_pack(&document, &registry)
                        .ok_or_else(|| super::unsupported_project_pack(&document.domain_pack))?;
                    let rendered = pack
                        .render_evidence(
                            &document,
                            &render_request,
                            objective_context.as_ref(),
                            &render_control,
                        )
                        .map_err(|error| {
                            domain_interruption(&error)
                                .unwrap_or_else(|| explanation_render_error(&error))
                        })?;
                    rendered.validate().map_err(|_| solution_contract_error())?;
                    if rendered.kind != render_request.kind
                        || rendered.evidence_checksum != render_request.evidence.checksum
                    {
                        return Err(solution_contract_error());
                    }
                    render_control.check().map_err(operation_interrupted)?;
                    Ok::<_, AppError>(rendered)
                })
                .await
                .map_err(join_error)??;
                let encoded = {
                    let evidence = source
                        .evidence
                        .ok_or_else(|| super::explanation_unavailable("infeasibility"))?;
                    let report = DiagnosticReport {
                        format: DIAGNOSTIC_REPORT_FORMAT.to_owned(),
                        schema_version: DIAGNOSTIC_REPORT_SCHEMA_VERSION,
                        scenario_id: request.scenario_id,
                        source_revision: request.source_revision,
                        run_id: request.run_id,
                        run_input_checksum: source.input.checksum,
                        source_document_hash: evidence.source_document_hash.clone(),
                        run_manifest_checksum: source.manifest.checksum.clone(),
                        evidence_checksum: evidence.checksum.clone(),
                        manifest: source.manifest,
                        original: evidence.original,
                        condition: evidence.condition,
                        diagnostic: evidence.diagnostic,
                        infeasibility: evidence.infeasibility,
                        rendered,
                    };
                    encode_json_controlled(&report, &control)
                        .map_err(|error| export_error(&error))?
                };
                control.check().map_err(operation_interrupted)?;
                if encoded.len() > MAX_DIAGNOSTIC_REPORT_BYTES {
                    return Err(validation_error(
                        "diagnostic.export_too_large",
                        "/report",
                        "The complete diagnostic report exceeds the local JSON size limit.",
                    ));
                }
                let observed_current_revision =
                    self.current_scenario_revision(request.scenario_id).await?;
                control.check().map_err(operation_interrupted)?;
                let report = String::from_utf8(encoded).map_err(|_| solution_contract_error())?;
                admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
                Ok(CapturedDiagnosticExport {
                    metadata: DiagnosticExportMetadataV1 {
                        schema_version: DIAGNOSTIC_EXPORT_API_SCHEMA_VERSION,
                        operation_id: request.operation_id,
                        request_id: request.request_id,
                        scenario_id: request.scenario_id,
                        source_revision: request.source_revision,
                        observed_current_revision,
                        run_id: request.run_id,
                        run_manifest_checksum: request.run_manifest_checksum,
                        evidence_checksum: request.evidence_checksum,
                    },
                    report,
                    control,
                    ordinary: ordinary.clone(),
                    counterfactual: counterfactual.clone(),
                })
            })
            .await;
            if source_admitted {
                admit_diagnostic_sources(ordinary.as_ref(), counterfactual.as_ref())?;
            }
            result
        })
        .await
    }
}

fn admit_diagnostic_sources(
    ordinary: Option<&super::OrdinaryPublicationHandle>,
    counterfactual: Option<&CounterfactualPublicationHandle>,
) -> Result<(), AppError> {
    if let Some(handle) = ordinary {
        handle.admit()?;
    }
    if let Some(handle) = counterfactual {
        handle.admit()?;
    }
    Ok(())
}

fn is_diagnostic_reference_refusal(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(report)
            if report.issues.iter().any(|issue| issue.code == "solution.reference_mismatch")
    )
}

async fn validate_diagnostic_source_references(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    source_revision: Revision,
    run_id: SolveRunId,
    run_manifest_checksum: &str,
    evidence_checksum: &str,
) -> Result<
    (
        eutheto_domain_ir::RunInput,
        Option<RunInfeasibilityEvidenceV1>,
    ),
    AppError,
> {
    let run = app
        .store
        .load_solve_run(scenario_id, run_id)
        .await
        .map_err(store_error)?
        .ok_or(AppError::NotFound(ResourceRef::SolveRun(run_id)))?;
    if let Some(manifest) = &run.manifest {
        crate::native_observations::validate_fingerprint(&run.input, manifest)
            .map_err(|_| solution_contract_error())?;
    }
    if run.input.run_id != run_id || run.input.scenario_id != scenario_id {
        return Err(solution_reference_error("/request"));
    }
    if Revision::try_new(run.input.scenario_revision).ok() != Some(source_revision) {
        return Err(solution_reference_error("/sourceRevision"));
    }
    if run
        .manifest
        .as_ref()
        .is_some_and(|manifest| manifest.checksum != run_manifest_checksum)
    {
        return Err(solution_reference_error("/request"));
    }
    let evidence = app
        .store
        .load_run_infeasibility_evidence(run_id)
        .await
        .map_err(store_error)?;
    if evidence.as_ref().is_some_and(|evidence| {
        evidence.run_id != run_id
            || evidence.scenario_id != scenario_id
            || evidence.scenario_revision != source_revision.value()
            || evidence.run_input_checksum != run.input.checksum
            || evidence.run_manifest_checksum != run_manifest_checksum
            || evidence.checksum != evidence_checksum
    }) {
        return Err(solution_reference_error("/evidenceChecksum"));
    }
    Ok((run.input, evidence))
}

fn validate_diagnostic_copy_group(
    evidence: Option<&RunInfeasibilityEvidenceV1>,
    group_id: &AssumptionGroupId,
) -> Result<(), AppError> {
    if evidence.is_some_and(|evidence| {
        !matches!(
            &evidence.infeasibility,
            InfeasibilityEvidenceV1::Conflict { groups, .. }
                if groups.iter().any(|group| &group.group_id == group_id)
        )
    }) {
        return Err(solution_reference_error("/groupId"));
    }
    Ok(())
}

fn validate_captured_source(
    request: &DiagnosticExportRequestV1,
    source: &StoredInfeasibilitySourceV1,
) -> Result<(), AppError> {
    if source.input.run_id != request.run_id
        || source.input.scenario_id != request.scenario_id
        || source.input.checksum.is_empty()
        || Revision::try_new(source.input.scenario_revision).ok() != Some(request.source_revision)
        || source.manifest.run_id != request.run_id
        || source.manifest.run_input_checksum != source.input.checksum
        || source.manifest.checksum != request.run_manifest_checksum
    {
        return Err(solution_reference_error("/request"));
    }
    let Some(evidence) = source.evidence.as_ref() else {
        return Err(super::explanation_unavailable("infeasibility"));
    };
    if evidence.run_id != request.run_id
        || evidence.scenario_id != request.scenario_id
        || evidence.scenario_revision != request.source_revision.value()
        || evidence.run_input_checksum != source.input.checksum
        || evidence.run_manifest_checksum != request.run_manifest_checksum
        || evidence.checksum != request.evidence_checksum
        || evidence.condition != source.condition
    {
        return Err(solution_reference_error("/evidenceChecksum"));
    }
    Ok(())
}

fn validate_checksum(value: &str, path: &str) -> Result<(), AppError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(validation_error(
            "diagnostic.reference_invalid",
            path,
            "The diagnostic authority checksum is invalid.",
        ));
    }
    Ok(())
}

pub(super) fn pack_store_error(error: DomainPackError) -> StoreError {
    match error {
        DomainPackError::Cancelled | DomainPackError::BudgetExpired => {
            StoreError::OperationCancelled
        }
        DomainPackError::ResourceLimitExceeded => StoreError::CommandApplication {
            code: "command.resource_limit".to_owned(),
            message: "The diagnostic command exceeded its resource limit.".to_owned(),
            field_path: None,
        },
        DomainPackError::UnsupportedDomainCapability(capability) => {
            StoreError::CommandApplication {
                code: "command.unsupported".to_owned(),
                message: format!("The domain pack does not support {capability:?}."),
                field_path: None,
            }
        }
        DomainPackError::InvalidPayload { path, message } => StoreError::CommandApplication {
            code: "command.invalid_domain_payload".to_owned(),
            message,
            field_path: Some(path),
        },
        other => StoreError::CommandApplication {
            code: "command.invalid_domain_payload".to_owned(),
            message: other.to_string(),
            field_path: None,
        },
    }
}
