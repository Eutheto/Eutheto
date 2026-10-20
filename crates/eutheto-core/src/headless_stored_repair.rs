//! Native/external original-source admission for explicit stored repair requests.

use super::super::RepairSource;
use super::super::repair::{decode_repair_source, repair_source_error};
use super::{
    EuthetoApp, OrdinaryReuseCustody, OriginalRunCustody, StoredObjectiveSelectionV1,
    StoredSolveAdmission, StoredSolveFailure, StoredSolveOutcome, StoredSolveRequest,
    load_reused_output, objective_selection_error, operation_interrupted,
    solution_verification_mismatch, solve_error, store_error,
};
use eutheto_domain_ir::{ObjectiveProfileId, PortableAcceptedResult, RunObjectiveOperation};
use eutheto_solver_api::ProgressSink;
use eutheto_store::AcceptedSourceExpectationV1;
use eutheto_types::{
    AppError, OperationControl, ParentSolveBudget, Revision, Rfc3339Timestamp, ScenarioId,
    SolutionId,
};
use eutheto_workforce::{
    assignment_rules::WorkforceAdmissionBudget,
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
};
use std::sync::Arc;
use std::time::Duration;

pub(super) struct RepairOriginal {
    pub portable: Arc<PortableAcceptedResult>,
    pub accepted_source: Option<AcceptedSourceExpectationV1>,
    pub custody: Option<OriginalRunCustody>,
}

impl EuthetoApp {
    /// Repairs from an explicit original without adopting a base or changing the scenario.
    ///
    /// The caller starts this same budget before reading a result-file operand. Exact retries
    /// retain their recorded manual generation; different originals conflict.
    ///
    /// # Errors
    /// Rejects invalid or blocked originals, incompatible manual custody, bounds and solve failures.
    pub async fn solve_stored_repair_with_budget(
        &self,
        mut request: StoredSolveRequest,
        source: RepairSource,
        progress: &mut dyn ProgressSink,
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        // Keep source-owning admission futures off callers' stack, as in solve_stored_inner.
        Box::pin(async move {
            let selection = StoredObjectiveSelectionV1 {
                schema_version: 1,
                profile_id: ObjectiveProfileId::new("official.workforce.objective.minimal-changes")
                    .map_err(|_| StoredSolveFailure::BeforeStart(objective_selection_error()))?,
                semantic_version: 1,
                operation: RunObjectiveOperation::Repair,
                baseline_source: super::StoredBaselineSourceV1::Accepted,
                alternatives_capture_id: None,
            };
            if request
                .objective_selection
                .as_ref()
                .is_some_and(|current| current != &selection)
            {
                return Err(StoredSolveFailure::BeforeStart(objective_selection_error()));
            }
            request.objective_selection = Some(selection);
            request.options.validate().map_err(|_| {
                StoredSolveFailure::BeforeStart(solve_error(
                    "solve.options_invalid",
                    "Solve options are invalid.",
                ))
            })?;
            let control = OperationControl::Solve(budget.phase_view());
            let mut admission = WorkforceAdmissionBudget::new(Some(&control));
            let original = self
                .admit_repair_original(request.scenario_id, source, &control, &mut admission)
                .await
                .map_err(StoredSolveFailure::BeforeStart)?;
            let requested_original = Arc::clone(&original.portable);
            let mut reuse = OrdinaryReuseCustody::new();
            reuse.requested_original = Some(Arc::clone(&requested_original));
            reuse.original_custody = original.custody.clone();
            reuse
                .retain_request_handle(self, request.scenario_id, request.request_id)
                .map_err(StoredSolveFailure::BeforeStart)?;
            if let Some(existing) = self
                .store
                .load_solve_run_by_request(request.request_id)
                .await
                .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
            {
                let outcome =
                    load_reused_output(self, &request, existing, true, None, &mut reuse, None)
                        .await
                        .map_err(StoredSolveFailure::BeforeStart)?;
                if let Some(custody) = &original.custody {
                    custody
                        .handle
                        .admit_input(&custody.input)
                        .map_err(StoredSolveFailure::BeforeStart)?;
                }
                control
                    .check()
                    .map_err(operation_interrupted)
                    .map_err(StoredSolveFailure::BeforeStart)?;
                return Ok(outcome);
            }
            let capture = self
                .capture_native_objective_source(
                    request.scenario_id,
                    request.expected_revision,
                    control.clone(),
                    true,
                    super::StoredBaselineSourceV1::Accepted,
                    started_at,
                    Some(original),
                    &mut admission,
                )
                .await
                .map_err(StoredSolveFailure::BeforeStart)?;
            self.solve_stored_inner(
                request,
                progress,
                StoredSolveAdmission::Budget { budget, started_at },
                None,
                Some(capture),
                Some(requested_original),
            )
            .await
        })
        .await
    }

    pub(super) async fn admit_repair_original(
        &self,
        scenario_id: ScenarioId,
        source: RepairSource,
        control: &OperationControl,
        budget: &mut WorkforceAdmissionBudget<'_>,
    ) -> Result<RepairOriginal, AppError> {
        control.check().map_err(operation_interrupted)?;
        let external = match source {
            RepairSource::ResultId(id) => {
                return self
                    .admit_native_repair_original(scenario_id, id, None, control, budget)
                    .await;
            }
            RepairSource::Json(bytes) => decode_repair_source(&bytes, control)?,
        };
        if external.scenario_id != scenario_id
            || external.run_input.pack_id.as_str() != WORKFORCE_PACK_ID
        {
            return Err(repair_source_error());
        }
        self.counterfactual
            .guard_run_publication(scenario_id, external.run_input.run_id)
            .await?;
        let ordinary = self.ordinary_run_handle(scenario_id, external.run_input.run_id)?;
        if let Some(handle) = &ordinary {
            handle.admit_input(&external.run_input)?;
        }
        if ordinary.is_some()
            || self
                .store
                .has_result_or_run_identity(
                    external.result_id,
                    external.run_input.run_id,
                    control.clone(),
                )
                .await
                .map_err(store_error)?
        {
            // A known identity never falls back after any native mismatch, alarm or custody refusal.
            return self
                .admit_native_repair_original(
                    scenario_id,
                    external.result_id,
                    Some(external),
                    control,
                    budget,
                )
                .await;
        }
        budget
            .preflight(&external)
            .map_err(super::super::super::super::manual::manual_integrity_error)?;
        let snapshot = self
            .store
            .load_scenario_source(
                scenario_id,
                Revision::new(external.scenario_revision),
                control.clone(),
            )
            .await
            .map_err(store_error)?;
        self.headless_service()
            .verify_result(&snapshot, &external, control)?;
        Ok(RepairOriginal {
            portable: Arc::new(external),
            accepted_source: None,
            custody: None,
        })
    }

    async fn admit_native_repair_original(
        &self,
        scenario_id: ScenarioId,
        solution_id: SolutionId,
        file: Option<PortableAcceptedResult>,
        control: &OperationControl,
        budget: &mut WorkforceAdmissionBudget<'_>,
    ) -> Result<RepairOriginal, AppError> {
        if let Some(handle) = self.ordinary_solution_handle(scenario_id, solution_id)? {
            handle.admit()?;
        }
        let stored = self.load_solution(scenario_id, solution_id).await?;
        if stored.portable.run_input.pack_id.as_str() != WORKFORCE_PACK_ID
            || file.as_ref().is_some_and(|file| file != &stored.portable)
        {
            return Err(repair_source_error());
        }
        budget
            .preflight(&stored.portable)
            .map_err(super::super::super::super::manual::manual_integrity_error)?;
        let reference =
            eutheto_domain_ir::AcceptedResultRefV1::from_result(&stored.portable.accepted_result)
                .map_err(|_| solution_verification_mismatch())?;
        let ordinary = self.ordinary_reference_handle(scenario_id, &reference)?;
        if let Some(handle) = &ordinary {
            handle.admit_input(&stored.portable.run_input)?;
        }
        let accepted = self
            .ensure_fresh_accepted_until(
                stored,
                control.clone(),
                tokio::time::Instant::now() + Duration::from_secs(30),
                None,
            )
            .await?;
        crate::native_observations::validate_approved_worker_origin(
            &accepted.portable.run_input,
            &accepted.portable.run_manifest,
            &self.solver_registry,
        )
        .map_err(|_| solution_verification_mismatch())?;
        let custody = if let Some(handle) = ordinary {
            handle.admit_input(&accepted.portable.run_input)?;
            budget
                .reserve_json(&accepted.portable.run_input, 1)
                .map_err(super::super::super::super::manual::manual_integrity_error)?;
            Some(OriginalRunCustody {
                handle,
                input: Arc::new(accepted.portable.run_input.clone()),
            })
        } else {
            None
        };
        let accepted_source = Some(AcceptedSourceExpectationV1::from_stored(&accepted));
        Ok(RepairOriginal {
            portable: Arc::new(accepted.portable),
            accepted_source,
            custody,
        })
    }
}
