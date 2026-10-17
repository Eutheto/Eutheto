//! Fresh result export authority; output encoders never replace original-domain verification.

use super::super::{
    EuthetoApp, OrdinaryPublicationHandle, export_error, operation_interrupted,
    recipient_report::{recipient_control, recipient_model, validate_recipient_bindings},
    reverify_accepted_solution, solution_contract_error, solution_verification_mismatch,
    store_error, validation_error,
};
use super::HeadlessService;
use eutheto_domain_api::bounded_json_size;
use eutheto_domain_ir::{NormalizedSolution, PortableAcceptedResult};
use eutheto_export::{
    EncodedRecipientHtml, PORTABLE_LIMITS, encode_json_controlled, encode_recipient_html,
};
use eutheto_store::StoredAcceptedResultV2;
use eutheto_types::{
    AppError, MAX_SCENARIO_DOCUMENT_BYTES, OperationControl, OperationInterruption, PersonId,
    RecipientOriginV1, RecipientReportOptionsV1, RecipientReportV1, Revision, Rfc3339Timestamp,
    ScenarioId, ScenarioSnapshotV1, SolutionId,
};
use eutheto_workforce::{
    assignments_csv::{AssignmentsCsvErrorCode, encode_assignments_csv},
    assignments_ics::{AssignmentsIcsErrorCode, encode_workforce_ics},
    export_source::WorkforceExportSource,
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    summary_csv::{SummaryCsvErrorCode, encode_workforce_summary_csv},
};

/// Bounded encoded bytes plus the solved revision and the scenario revision observed while the
/// result was read, independently verified, and encoded under the mutation boundary.
/// This presentation value does not grant authority to caller-modified bytes or claim that the
/// observed revision remains current after this method returns and a caller publishes the bytes.
pub struct StoredResultExport {
    pub bytes: Vec<u8>,
    pub scenario_revision: Revision,
    /// Revision observed as of the read/verification/encoding boundary.
    pub current_revision: Revision,
}

impl HeadlessService {
    /// Re-encodes the complete accepted artifact after fresh source-bound verification.
    /// External historical solver proof and timing remain unendorsed archival metadata.
    ///
    /// # Errors
    /// Rejects mismatched/unverified material, output limits and operation interruption.
    pub fn export_result_json(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResult,
        control: &OperationControl,
    ) -> Result<Vec<u8>, AppError> {
        bound_portable(portable, control)?;
        self.verify_result(snapshot, portable, control)?;
        encode_json(portable, control)
    }

    /// Exports only the selected Workforce schedule after verifying the complete source.
    /// False decisions and proof/evidence remain part of JSON, not this CSV round trip.
    ///
    /// # Errors
    /// Rejects unsupported packs, mismatched/unverified input, output limits and interruption.
    pub fn export_assignments_csv(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResult,
        control: &OperationControl,
    ) -> Result<Vec<u8>, AppError> {
        bound_portable(portable, control)?;
        self.verify_result(snapshot, portable, control)?;
        encode_csv(&portable.accepted_result.solution, control)
    }
    /// Exports source-derived person and shift summary rows after fresh verification.
    ///
    /// # Errors
    /// Rejects unsupported packs, mismatched/unverified input, output limits and interruption.
    pub fn export_result_summary_csv(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResult,
        control: &OperationControl,
    ) -> Result<Vec<u8>, AppError> {
        bound_portable(portable, control)?;
        self.verify_result(snapshot, portable, control)?;
        encode_summary_csv(
            &snapshot.document,
            WorkforceExportSource::Terminal(portable),
            snapshot.revision.value(),
            control,
        )
    }
    /// Exports a combined or per-person Workforce calendar only after fresh source verification.
    /// The source document, not candidate-provided timestamps or evidence, supplies event data.
    ///
    /// # Errors
    /// Rejects unsupported packs, mismatched/unverified input, stale source bindings, output
    /// limits, unsupported timestamp precision and operation interruption.
    pub fn export_result_ics(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResult,
        person_filter: Option<PersonId>,
        control: &OperationControl,
    ) -> Result<Vec<u8>, AppError> {
        bound_portable(portable, control)?;
        self.verify_result(snapshot, portable, control)?;
        encode_ics(
            &snapshot.document,
            WorkforceExportSource::Terminal(portable),
            person_filter,
            snapshot.revision.value(),
            self.clock.now(),
            control,
        )
    }

    /// Builds the same privacy-filtered report from a freshly verified immutable file source.
    /// Recorded changes remain descriptive; no native history admission or current head is inferred.
    ///
    /// # Errors
    /// Rejects unsupported packs, malformed or mismatched source bindings, failed original-domain
    /// verification, report limits and interruption under the inherited report budget.
    pub fn export_recipient_html(
        &self,
        snapshot: &ScenarioSnapshotV1,
        portable: &PortableAcceptedResult,
        options: RecipientReportOptionsV1,
        control: &OperationControl,
    ) -> Result<(EncodedRecipientHtml, RecipientReportV1), AppError> {
        let control = recipient_control(control.clone(), &self.monotonic_clock)?;
        bound_portable(portable, &control)?;
        let limit =
            usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES).map_err(|_| solution_contract_error())?;
        bounded_json_size(snapshot, limit).map_err(|_| solution_contract_error())?;
        eutheto_export::validate_scenario_snapshot(snapshot)
            .map_err(|_| solution_contract_error())?;
        control.check().map_err(operation_interrupted)?;
        portable
            .validate()
            .map_err(|_| solution_verification_mismatch())?;
        crate::native_observations::validate_fingerprint(
            &portable.run_input,
            &portable.run_manifest,
        )
        .map_err(|_| solution_verification_mismatch())?;
        let input = &portable.run_input;
        if snapshot.revision.value() != input.scenario_revision {
            return Err(solution_verification_mismatch());
        }
        validate_recipient_bindings(
            &snapshot.document,
            input,
            &portable.accepted_result,
            &self.packs,
        )?;
        // The shared domain builder revalidates the complete original rules, score and report
        // while projecting the full schedule; calling verify_result first would do that twice.
        let report = recipient_model(
            &snapshot.document,
            input,
            &portable.accepted_result,
            RecipientOriginV1::VerifiedFile,
            None,
            None,
            None,
            options,
            self.clock.now(),
            &control,
        )?;
        let encoded =
            encode_recipient_html(&report, &control).map_err(|error| export_error(&error))?;
        Ok((encoded, report))
    }
}

impl EuthetoApp {
    /// Exports the full portable result from its exact retained solved revision.
    ///
    /// # Errors
    /// Rejects missing, unverified or mismatched stored material, limits and interruption.
    pub async fn export_solution_json(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
    ) -> Result<StoredResultExport, AppError> {
        let control = OperationControl::Cancellation(self.cancellation.clone());
        self.export_verified(scenario_id, solution_id, &control, |stored, _, control| {
            encode_json(&stored.portable, control)
        })
        .await
    }

    /// Exports the selected Workforce schedule from its exact retained solved revision.
    ///
    /// # Errors
    /// Rejects unsupported packs, missing/unverified material, limits and interruption.
    pub async fn export_solution_assignments_csv(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
    ) -> Result<StoredResultExport, AppError> {
        let control = OperationControl::Cancellation(self.cancellation.clone());
        self.export_verified(scenario_id, solution_id, &control, |stored, _, control| {
            encode_csv(&stored.portable.accepted_result.solution, control)
        })
        .await
    }
    /// Exports source-derived person and shift summary rows from the exact retained solved
    /// revision.
    ///
    /// # Errors
    /// Rejects unsupported packs, missing/unverified material, limits and interruption.
    pub async fn export_solution_summary_csv(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
    ) -> Result<StoredResultExport, AppError> {
        let control = OperationControl::Cancellation(self.cancellation.clone());
        self.export_verified(
            scenario_id,
            solution_id,
            &control,
            |stored, current, control| {
                encode_summary_csv(
                    &stored.document,
                    WorkforceExportSource::Terminal(&stored.portable),
                    current.value(),
                    control,
                )
            },
        )
        .await
    }

    /// Exports a combined or per-person Workforce calendar from its exact retained solved
    /// revision.
    ///
    /// # Errors
    /// Rejects unsupported packs, missing/unverified material, limits, timestamp precision and
    /// interruption.
    pub async fn export_solution_ics(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
        person_filter: Option<PersonId>,
    ) -> Result<StoredResultExport, AppError> {
        let control = OperationControl::Cancellation(self.cancellation.clone());
        let clock = &self.clock;
        self.export_verified(
            scenario_id,
            solution_id,
            &control,
            |stored, current, control| {
                encode_ics(
                    &stored.document,
                    WorkforceExportSource::Terminal(&stored.portable),
                    person_filter,
                    current.value(),
                    clock.now(),
                    control,
                )
            },
        )
        .await
    }

    async fn export_verified<F>(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
        control: &OperationControl,
        encode: F,
    ) -> Result<StoredResultExport, AppError>
    where
        F: FnOnce(
            &StoredAcceptedResultV2,
            Revision,
            &OperationControl,
        ) -> Result<Vec<u8>, AppError>,
    {
        let ordinary = self.ordinary_solution_handle(scenario_id, solution_id)?;
        let result = async {
            control.check().map_err(operation_interrupted)?;
            let mutation = self.scenario_lock(scenario_id).await;
            // Keep the mutation boundary through source capture, independent verification, and
            // encoding. Publication happens in the CLI after this method returns, so returned
            // revisions are explicitly observations as-of this read/verification boundary, not
            // promises that the scenario is still current at publication time.
            let _guard = mutation.lock().await;
            let stored = self.load_solution(scenario_id, solution_id).await?;
            if let Some(handle) = &ordinary {
                handle.admit_input(&stored.portable.run_input)?;
            }
            let current_revision = self
                .store
                .get_project(scenario_id)
                .await
                .map_err(store_error)?
                .summary
                .revision;
            bound_portable(&stored.portable, control)?;
            reverify_accepted_solution(&stored, &self.pack_registry, control)?;
            let bytes = encode(&stored, current_revision, control)?;
            exported(&stored, current_revision, bytes)
        }
        .await;
        ordinary
            .as_ref()
            .map_or(Ok(()), OrdinaryPublicationHandle::admit)?;
        result
    }
}

fn exported(
    stored: &StoredAcceptedResultV2,
    current_revision: Revision,
    bytes: Vec<u8>,
) -> Result<StoredResultExport, AppError> {
    let scenario_revision = Revision::try_new(stored.portable.scenario_revision)
        .map_err(|_| solution_contract_error())?;
    Ok(StoredResultExport {
        bytes,
        scenario_revision,
        current_revision,
    })
}

fn bound_portable(
    portable: &PortableAcceptedResult,
    control: &OperationControl,
) -> Result<(), AppError> {
    control.check().map_err(operation_interrupted)?;
    let limit =
        usize::try_from(PORTABLE_LIMITS.max_json_bytes).map_err(|_| solution_contract_error())?;
    bounded_json_size(portable, limit).map_err(|_| solution_contract_error())?;
    control.check().map_err(operation_interrupted)
}

fn encode_json(
    portable: &PortableAcceptedResult,
    control: &OperationControl,
) -> Result<Vec<u8>, AppError> {
    let encoded = encode_json_controlled(portable, control);
    control.check().map_err(operation_interrupted)?;
    encoded.map_err(|error| export_error(&error))
}

pub(crate) fn encode_csv(
    solution: &NormalizedSolution,
    control: &OperationControl,
) -> Result<Vec<u8>, AppError> {
    if solution.pack_id.as_str() != "official.workforce" {
        return Err(AppError::Unsupported(eutheto_types::UnsupportedFeature {
            code: "solution.csv_pack_unavailable".to_owned(),
            capability: "Assignment CSV for this domain pack".to_owned(),
        }));
    }
    encode_assignments_csv(solution, control).map_err(|error| match error.code {
        AssignmentsCsvErrorCode::Cancelled => {
            operation_interrupted(OperationInterruption::Cancelled)
        }
        AssignmentsCsvErrorCode::DeadlineExceeded => {
            operation_interrupted(OperationInterruption::DeadlineExceeded)
        }
        _ => validation_error(
            "solution.csv_export_invalid",
            "/result",
            "The accepted schedule cannot be encoded within the assignment CSV contract.",
        ),
    })
}
pub(crate) fn encode_summary_csv(
    document: &eutheto_types::ScenarioDocument,
    source: WorkforceExportSource<'_>,
    current_revision: u64,
    control: &OperationControl,
) -> Result<Vec<u8>, AppError> {
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
        return Err(AppError::Unsupported(eutheto_types::UnsupportedFeature {
            code: "solution.summary_csv_pack_unavailable".to_owned(),
            capability: "Workforce people and shift summary CSV export".to_owned(),
        }));
    }
    encode_workforce_summary_csv(document, source, current_revision, control).map_err(|error| {
        match error.code {
            SummaryCsvErrorCode::Cancelled => {
                operation_interrupted(OperationInterruption::Cancelled)
            }
            SummaryCsvErrorCode::DeadlineExceeded => {
                operation_interrupted(OperationInterruption::DeadlineExceeded)
            }
            SummaryCsvErrorCode::UnsupportedPack => {
                AppError::Unsupported(eutheto_types::UnsupportedFeature {
                    code: "solution.summary_csv_pack_unavailable".to_owned(),
                    capability: "Workforce people and shift summary CSV export".to_owned(),
                })
            }
            _ => validation_error(
                "solution.summary_csv_export_invalid",
                "/result",
                "The accepted Workforce result cannot be encoded within the summary CSV contract.",
            ),
        }
    })
}
pub(crate) fn encode_ics(
    document: &eutheto_types::ScenarioDocument,
    source: WorkforceExportSource<'_>,
    person_filter: Option<PersonId>,
    current_revision: u64,
    created_at: Rfc3339Timestamp,
    control: &OperationControl,
) -> Result<Vec<u8>, AppError> {
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
        return Err(AppError::Unsupported(eutheto_types::UnsupportedFeature {
            code: "solution.ics_pack_unavailable".to_owned(),
            capability: "Workforce iCalendar export for this domain pack".to_owned(),
        }));
    }
    encode_workforce_ics(
        document,
        source,
        person_filter,
        current_revision,
        created_at,
        control,
    )
    .map_err(|error| match error.code {
        AssignmentsIcsErrorCode::Cancelled => {
            operation_interrupted(OperationInterruption::Cancelled)
        }
        AssignmentsIcsErrorCode::DeadlineExceeded => {
            operation_interrupted(OperationInterruption::DeadlineExceeded)
        }
        AssignmentsIcsErrorCode::UnsupportedPack => {
            AppError::Unsupported(eutheto_types::UnsupportedFeature {
                code: "solution.ics_pack_unavailable".to_owned(),
                capability: "Workforce iCalendar export for this domain pack".to_owned(),
            })
        }
        _ => validation_error(
            "solution.ics_export_invalid",
            "/result",
            "The accepted Workforce schedule cannot be encoded as iCalendar.",
        ),
    })
}
