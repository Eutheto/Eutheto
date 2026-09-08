//! Fresh result export authority; output encoders never replace original-domain verification.

use super::super::{
    EuthetoApp, export_error, operation_interrupted, reverify_accepted_solution,
    solution_contract_error, store_error, validation_error,
};
use super::HeadlessService;
use eutheto_domain_api::bounded_json_size;
use eutheto_domain_ir::PortableAcceptedResultV2;
use eutheto_export::{PORTABLE_LIMITS, encode_json_controlled};
use eutheto_store::StoredAcceptedResultV2;
use eutheto_types::{
    AppError, OperationControl, OperationInterruption, Revision, ScenarioId, ScenarioSnapshotV1,
    SolutionId,
};
use eutheto_workforce::assignments_csv::{AssignmentsCsvErrorCode, encode_assignments_csv};

/// Bounded encoded bytes plus the revisions observed during stored result capture.
/// This presentation value does not grant authority to caller-modified bytes.
pub struct StoredResultExport {
    pub bytes: Vec<u8>,
    pub scenario_revision: Revision,
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
        portable: &PortableAcceptedResultV2,
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
        portable: &PortableAcceptedResultV2,
        control: &OperationControl,
    ) -> Result<Vec<u8>, AppError> {
        bound_portable(portable, control)?;
        self.verify_result(snapshot, portable, control)?;
        encode_csv(portable, control)
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
        let (stored, current_revision) = self
            .load_verified_export(scenario_id, solution_id, &control)
            .await?;
        let bytes = encode_json(&stored.portable, &control)?;
        exported(&stored, current_revision, bytes)
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
        let (stored, current_revision) = self
            .load_verified_export(scenario_id, solution_id, &control)
            .await?;
        let bytes = encode_csv(&stored.portable, &control)?;
        exported(&stored, current_revision, bytes)
    }

    async fn load_verified_export(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
        control: &OperationControl,
    ) -> Result<(StoredAcceptedResultV2, Revision), AppError> {
        control.check().map_err(operation_interrupted)?;
        let mutation = self.scenario_lock(scenario_id).await;
        let _guard = mutation.lock().await;
        let stored = self.load_solution(scenario_id, solution_id).await?;
        let current_revision = self
            .store
            .get_project(scenario_id)
            .await
            .map_err(store_error)?
            .summary
            .revision;
        bound_portable(&stored.portable, control)?;
        reverify_accepted_solution(&stored, &self.pack_registry, control)?;
        Ok((stored, current_revision))
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
    portable: &PortableAcceptedResultV2,
    control: &OperationControl,
) -> Result<(), AppError> {
    control.check().map_err(operation_interrupted)?;
    let limit =
        usize::try_from(PORTABLE_LIMITS.max_json_bytes).map_err(|_| solution_contract_error())?;
    bounded_json_size(portable, limit).map_err(|_| solution_contract_error())?;
    control.check().map_err(operation_interrupted)
}

fn encode_json(
    portable: &PortableAcceptedResultV2,
    control: &OperationControl,
) -> Result<Vec<u8>, AppError> {
    let encoded = encode_json_controlled(portable, control);
    control.check().map_err(operation_interrupted)?;
    encoded.map_err(|error| export_error(&error))
}

fn encode_csv(
    portable: &PortableAcceptedResultV2,
    control: &OperationControl,
) -> Result<Vec<u8>, AppError> {
    if portable.accepted_result.solution.pack_id.as_str() != "official.workforce" {
        return Err(AppError::Unsupported(eutheto_types::UnsupportedFeature {
            code: "solution.csv_pack_unavailable".to_owned(),
            capability: "Assignment CSV for this domain pack".to_owned(),
        }));
    }
    encode_assignments_csv(&portable.accepted_result.solution, control).map_err(|error| match error
        .code
    {
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
