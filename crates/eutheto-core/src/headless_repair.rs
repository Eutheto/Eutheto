//! Explicit accepted-original or unverified external-plan capture without scenario mutation.

use super::super::super::{
    manual::manual_integrity_error, operation_interrupted, selected_workforce_assignments,
    solution_contract_error, solution_verification_mismatch,
};
use super::{
    PreparedSolve, SolveOperation, SolvePreparationFailure, preparation_failure, solve_error,
};
use eutheto_domain_ir::{
    AcceptedResultRefV1, MAX_CAPTURED_BASELINE_BYTES, ObjectiveProfileId, PortableAcceptedResult,
    RunBaselineBinding, RunBaselineOriginal, RunBaselineSelection, RunExternalBaselineBinding,
    RunObjectiveContext, RunObjectiveOperation, validate_captured_baseline_size,
};
use eutheto_import::{InspectedBundle, InspectionPolicy, validate_standalone_scenario};
use eutheto_types::{
    AppError, EntityId, OperationControl, Revision, ScenarioDocument, ScenarioSnapshotV1,
    SolutionId,
};
use eutheto_workforce::{
    assignment_rules::{
        WorkforceAdmissionBudget, decode_workforce_assignment, workforce_baseline_reference,
        workforce_external_baseline_reference,
    },
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{BaseSchedule, WorkforceEntity},
};
use serde::Deserialize;
use std::sync::Arc;

/// Explicit original accepted result. JSON is bounded and freshly admitted by Core, not a path.
#[derive(Debug)]
pub enum RepairSource {
    /// A native result identity, or an exact result entry in the input scenario bundle.
    ResultId(SolutionId),
    /// An external portable accepted-result JSON document; its old proof remains inert.
    Json(Vec<u8>),
}

impl SolveOperation {
    /// Repairs a standalone immutable snapshot from an explicitly selected accepted original.
    ///
    /// # Errors
    /// Rejects absent original revisions, imported manual custody, invalid sources and bounds.
    pub fn prepare_repair(
        self,
        snapshot: ScenarioSnapshotV1,
        source: RepairSource,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        validate_standalone_scenario(&snapshot, &InspectionPolicy::default()).map_err(|error| {
            preparation_failure(super::super::super::import_error(&error), &control)
        })?;
        self.prepare_file_repair(snapshot, None, source)
    }

    /// Repairs an immutable scenario bundle using its exact current/historical source records.
    ///
    /// # Errors
    /// Rejects malformed bundles, missing exact originals, imported manual custody and bounds.
    pub fn prepare_bundle_repair(
        self,
        bytes: &[u8],
        source: RepairSource,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        let mut bundle = self
            .service
            .decode_scenario_bundle(bytes, &control)
            .map_err(|error| preparation_failure(error, &control))?;
        let snapshot = bundle
            .scenarios
            .pop()
            .ok_or_else(|| preparation_failure(solution_contract_error(), &control))?;
        self.prepare_file_repair(snapshot, Some(bundle), source)
    }

    /// Repairs against an explicit unverified starting plan in a standalone snapshot.
    ///
    /// # Errors
    /// Rejects absent/wrong-kind sources, malformed snapshots, bounds and compilation failures.
    pub fn prepare_external_repair(
        self,
        snapshot: ScenarioSnapshotV1,
        entity_id: EntityId,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        validate_standalone_scenario(&snapshot, &InspectionPolicy::default()).map_err(|error| {
            preparation_failure(super::super::super::import_error(&error), &control)
        })?;
        self.prepare_file_external_repair(snapshot, None, entity_id)
    }

    /// Repairs against an unverified starting plan while retaining the complete inspected bundle.
    ///
    /// # Errors
    /// Rejects invalid bundles, unavailable sources, bounds and compilation failures.
    pub fn prepare_bundle_external_repair(
        self,
        bytes: &[u8],
        entity_id: EntityId,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        let mut bundle = self
            .service
            .decode_scenario_bundle(bytes, &control)
            .map_err(|error| preparation_failure(error, &control))?;
        let snapshot = bundle
            .scenarios
            .pop()
            .ok_or_else(|| preparation_failure(solution_contract_error(), &control))?;
        self.prepare_file_external_repair(snapshot, Some(bundle), entity_id)
    }

    fn prepare_file_external_repair(
        self,
        snapshot: ScenarioSnapshotV1,
        bundle: Option<InspectedBundle>,
        entity_id: EntityId,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        let context = (|| {
            let mut budget = WorkforceAdmissionBudget::new(Some(&control));
            let baseline = capture_external_baseline(&snapshot.document, entity_id, &mut budget)?;
            Ok(RunObjectiveContext {
                schema_version: 3,
                pack_id: snapshot.document.domain_pack.id.clone(),
                profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")
                    .map_err(|_| solution_contract_error())?,
                semantic_version: 1,
                operation: RunObjectiveOperation::Repair,
                baseline: None,
                external_baseline: Some(Box::new(baseline)),
            })
        })()
        .map_err(|error| preparation_failure(error, &control))?;
        self.prepare_captured_with_objective_context(snapshot, bundle, Some(context))
    }

    fn prepare_file_repair(
        self,
        snapshot: ScenarioSnapshotV1,
        bundle: Option<InspectedBundle>,
        source: RepairSource,
    ) -> Result<PreparedSolve, Box<SolvePreparationFailure>> {
        let control = self.control();
        let context = (|| {
            if snapshot.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
                return Err(repair_source_error());
            }
            if snapshot.document.domain.entities.values().any(|value| {
                value.get("kind").and_then(serde_json::Value::as_str) == Some("manualSelection")
            }) {
                return Err(solve_error(
                    "solve.repair_manual_custody_unavailable",
                    "A file's manual selection has no native receipt custody; use its native stored scenario.",
                ));
            }
            let original = match source {
                RepairSource::Json(bytes) => decode_repair_source(&bytes, &control)?,
                RepairSource::ResultId(id) => {
                    let bytes = bundle.as_ref()
                        .and_then(|bundle| bundle.additional_entries.get(&format!("results/{id}.json")))
                        .ok_or_else(repair_source_error)?;
                    let original = decode_repair_source(bytes, &control)?;
                    if original.result_id != id {
                        return Err(repair_source_error());
                    }
                    original
                }
            };
            if original.scenario_id != snapshot.document.scenario_id {
                return Err(repair_source_error());
            }
            let original_snapshot = if snapshot.revision.value() == original.scenario_revision {
                &snapshot
            } else {
                bundle.as_ref()
                    .and_then(|bundle| bundle.scenario_revisions.iter().find(|source| {
                        source.document.scenario_id == original.scenario_id
                            && source.revision.value() == original.scenario_revision
                    }))
                    .ok_or_else(|| solve_error(
                        "solve.repair_original_revision_required",
                        "Repair requires the exact original scenario revision; include it and the result in the input scenario bundle.",
                    ))?
            };
            self.service.verify_result(original_snapshot, &original, &control)?;
            let baseline = captured_original(&original, &control)?;
            repair_context(snapshot.document.domain_pack.id.clone(), baseline)
        })().map_err(|error| preparation_failure(error, &control))?;
        self.prepare_captured_with_objective_context(snapshot, bundle, Some(context))
    }
}

pub(super) fn capture_external_baseline(
    document: &ScenarioDocument,
    entity_id: EntityId,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<RunExternalBaselineBinding, AppError> {
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
        return Err(external_baseline_error());
    }
    let raw = document
        .domain
        .entities
        .get(&entity_id)
        .ok_or_else(external_baseline_error)?;
    budget
        .reserve_json(raw, 1)
        .map_err(manual_integrity_error)?;
    let WorkforceEntity::ExternalStartingPlan(record) =
        WorkforceEntity::deserialize(raw).map_err(|_| external_baseline_error())?
    else {
        return Err(external_baseline_error());
    };
    if record.id != entity_id || record.schema_version != 1 {
        return Err(external_baseline_error());
    }
    Ok(RunExternalBaselineBinding {
        entity: workforce_external_baseline_reference(entity_id)
            .map_err(|_| solution_contract_error())?,
        selected_pair_content_hash: budget
            .pair_content_hash(&record.assignments)
            .map_err(manual_integrity_error)?,
    })
}

fn external_baseline_error() -> AppError {
    solve_error(
        "solve.external_baseline_invalid",
        "The selected unverified starting plan is not available in this scenario revision.",
    )
}

pub(super) fn decode_repair_source(
    bytes: &[u8],
    control: &OperationControl,
) -> Result<PortableAcceptedResult, AppError> {
    control.check().map_err(operation_interrupted)?;
    let result = PortableAcceptedResult::from_json(bytes).map_err(|_| repair_source_error())?;
    control.check().map_err(operation_interrupted)?;
    Ok(result)
}

pub(super) fn captured_original(
    original: &PortableAcceptedResult,
    control: &OperationControl,
) -> Result<RunBaselineBinding, AppError> {
    let mut budget = WorkforceAdmissionBudget::new(Some(control));
    budget.preflight(original).map_err(manual_integrity_error)?;
    preflight_captured_original(original, &mut budget)?;
    let (binding, record) = original_record(original, &mut budget)?;
    capture_original_record(binding, record, &mut budget)
}

/// Checks only the new captured form; native in-snapshot/manual baselines keep their own capacity.
pub(super) fn preflight_captured_original(
    original: &PortableAcceptedResult,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<(), AppError> {
    let mut remaining = CaptureByteLimit(MAX_CAPTURED_BASELINE_BYTES);
    // Empty brackets already account for the array overhead. Count borrowed selected pairs
    // before selected_workforce_assignments allocates its sets/vector. Ordering changes no bytes;
    // that existing projection still owns canonical ordering and duplicate rejection.
    let empty = WorkforceEntity::BaseSchedule(BaseSchedule {
        id: EntityId::from_uuid(original.result_id.as_uuid()),
        source_solution_id: original.result_id,
        source_revision: Revision::new(original.scenario_revision),
        assignments: Vec::new(),
    });
    serde_json::to_writer(&mut remaining, &empty).map_err(|_| repair_capture_limit())?;
    let mut first = true;
    for assignment in &original.accepted_result.solution.assignments {
        budget.step().map_err(manual_integrity_error)?;
        let (pair, selected) = decode_workforce_assignment(assignment)
            .map_err(|_| solution_verification_mismatch())?;
        if selected {
            if !first {
                std::io::Write::write_all(&mut remaining, b",")
                    .map_err(|_| repair_capture_limit())?;
            }
            serde_json::to_writer(&mut remaining, &pair).map_err(|_| repair_capture_limit())?;
            first = false;
        }
    }
    Ok(())
}

struct CaptureByteLimit(usize);

impl std::io::Write for CaptureByteLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("captured baseline JSON bytes"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn repair_capture_limit() -> AppError {
    solve_error(
        "solve.repair_capture_limit",
        "The complete captured repair baseline exceeds its 512 KiB limit.",
    )
}

pub(super) fn original_record(
    original: &PortableAcceptedResult,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<(RunBaselineOriginal, BaseSchedule), AppError> {
    budget
        .reserve_json(&original.accepted_result, 1)
        .map_err(manual_integrity_error)?;
    let base = BaseSchedule {
        id: EntityId::from_uuid(original.result_id.as_uuid()),
        source_solution_id: original.result_id,
        source_revision: Revision::new(original.scenario_revision),
        assignments: selected_workforce_assignments(&original.accepted_result)?,
    };
    let hash = budget
        .pair_content_hash(&base.assignments)
        .map_err(manual_integrity_error)?;
    let binding = RunBaselineOriginal {
        entity: workforce_baseline_reference(base.id, false)
            .map_err(|_| solution_contract_error())?,
        pair_content_hash: hash,
        accepted_result: AcceptedResultRefV1::from_result(&original.accepted_result)
            .map_err(|_| solution_contract_error())?,
        source_scenario_id: original.scenario_id,
        source_revision: original.scenario_revision,
        source_document_hash: original.run_input.snapshot_document_hash.clone(),
        source_run_id: original.run_input.run_id,
        source_run_input_checksum: original.run_input.checksum.clone(),
        source_run_manifest_checksum: original.run_manifest.checksum.clone(),
    };
    Ok((binding, base))
}

pub(super) fn capture_original_record(
    binding: RunBaselineOriginal,
    base: BaseSchedule,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<RunBaselineBinding, AppError> {
    let record = WorkforceEntity::BaseSchedule(base);
    validate_captured_baseline_size(&record).map_err(|_| repair_capture_limit())?;
    budget
        .reserve_json(&record, 1)
        .map_err(manual_integrity_error)?;
    let captured = serde_json::to_value(record).map_err(|_| solution_contract_error())?;
    Ok(RunBaselineBinding {
        selected_pair_content_hash: binding.pair_content_hash.clone(),
        original: binding,
        selection: RunBaselineSelection::OriginalAccepted {},
        captured_original: Some(Arc::new(captured)),
    })
}

pub(super) fn repair_context(
    pack_id: eutheto_types::PackId,
    baseline: RunBaselineBinding,
) -> Result<RunObjectiveContext, AppError> {
    Ok(RunObjectiveContext {
        schema_version: if baseline.captured_original.is_some() {
            2
        } else {
            1
        },
        pack_id,
        profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")
            .map_err(|_| solution_contract_error())?,
        semantic_version: 1,
        operation: RunObjectiveOperation::Repair,
        baseline: Some(Box::new(baseline)),
        external_baseline: None,
    })
}

pub(super) fn repair_source_error() -> AppError {
    solve_error(
        "solve.repair_source_invalid",
        "The requested repair source must be an exact accepted Workforce result for this scenario.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eutheto_domain_ir::{
        AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId, DomainEntityKindId,
        DomainEntityRef,
    };

    fn assignment(
        index: usize,
        selected: bool,
    ) -> Result<DomainAssignment, Box<dyn std::error::Error>> {
        let pair_id =
            format!("01890a5d-ac96-7b64-9f74-bbfcf30f9f80.01890a5d-ac96-7b64-9f74-{index:012x}");
        Ok(DomainAssignment {
            id: DomainAssignmentId::new(format!("official.workforce.assignment.{pair_id}"))?,
            entity: DomainEntityRef {
                kind: DomainEntityKindId::new("official.workforce.assignment")?,
                id: DomainEntityId::new(pair_id)?,
            },
            value: AssignmentValue::Boolean(selected),
            evidence: Vec::new(),
        })
    }

    #[test]
    fn capture_preflight_matches_complete_canonical_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut original: PortableAcceptedResult = serde_json::from_slice(include_bytes!(
            "../../eutheto-domain-ir/tests/fixtures/portable_accepted_v2.json"
        ))?;
        let mut base = BaseSchedule {
            id: EntityId::from_uuid(original.result_id.as_uuid()),
            source_solution_id: original.result_id,
            source_revision: Revision::new(original.scenario_revision),
            assignments: Vec::new(),
        };
        let empty_bytes = serde_json::to_vec(&WorkforceEntity::BaseSchedule(base.clone()))?.len();
        let (pair, _) = decode_workforce_assignment(&assignment(0, true)?)?;
        let pair_bytes = serde_json::to_vec(&pair)?.len();
        let count = (MAX_CAPTURED_BASELINE_BYTES - empty_bytes + 1) / (pair_bytes + 1);
        original.accepted_result.solution.assignments = (0..count)
            .rev()
            .map(|index| assignment(index, true))
            .collect::<Result<Vec<_>, _>>()?;
        original
            .accepted_result
            .solution
            .assignments
            .push(assignment(count, false)?);
        base.assignments = selected_workforce_assignments(&original.accepted_result)
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        assert!(base.assignments.windows(2).all(|pairs| pairs[0] < pairs[1]));
        let actual_bytes = serde_json::to_vec(&WorkforceEntity::BaseSchedule(base.clone()))?.len();
        assert_eq!(actual_bytes, empty_bytes + count * (pair_bytes + 1) - 1);
        assert!(MAX_CAPTURED_BASELINE_BYTES - actual_bytes < pair_bytes + 1);
        preflight_captured_original(&original, &mut WorkforceAdmissionBudget::new(None))
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;

        original
            .accepted_result
            .solution
            .assignments
            .last_mut()
            .ok_or("missing assignment")?
            .value = AssignmentValue::Boolean(true);
        base.assignments = selected_workforce_assignments(&original.accepted_result)
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        assert!(validate_captured_baseline_size(&WorkforceEntity::BaseSchedule(base)).is_err());
        assert!(matches!(
            preflight_captured_original(&original, &mut WorkforceAdmissionBudget::new(None)),
            Err(AppError::Protocol(error)) if error.code == "solve.repair_capture_limit"
        ));

        original.accepted_result.solution.assignments =
            vec![assignment(0, true)?, assignment(0, false)?];
        assert!(selected_workforce_assignments(&original.accepted_result).is_err());
        Ok(())
    }
}
