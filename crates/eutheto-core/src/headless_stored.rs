use super::super::super::{
    EuthetoApp, OrdinaryPublicationHandle, OrdinarySolveLease, export_error, join_error,
    operation_interrupted, solution_contract_error, solution_verification_mismatch, store_error,
};
use super::{
    FirstAccepted, HeadlessCompilationReport, HeadlessService, HeadlessSolveOutcome,
    NativeAlarmCustody, PreparedSolve, SolvePreparationFailure, accepted_evidence,
    interruption_status, known_verification_alarm_metadata, milliseconds, no_result, observed_wall,
    safe_verification_secondary_cause, solve_error, timing_error,
};
use eutheto_domain_ir::{
    AUTHORED_OBJECTIVE_PROFILE_ID, ObjectiveProfileId, PortableAcceptedResult, RunBaselineBinding,
    RunBaselineSelection, RunExternalBaselineBinding, RunInput, RunManifest, RunObjectiveContext,
    RunObjectiveOperation, RunPhaseTimingsV1, RunTerminalOutcomeV1, VerifiedCheckpoint,
};
use eutheto_export::prepare_json_atomic_controlled;
use eutheto_solver_api::ProgressSink;
use eutheto_solver_router::{RouterExecutionRecord, RoutingDecision};
use eutheto_store::{
    AcceptedSourceExpectationV1, CandidateDiagnosticsV1, ExistingSolveRunV1, NewSolveRunV1,
    ScenarioCustodyStamp, SqliteScenarioStore, StoreError,
};
use eutheto_types::{
    AppError, BackendSelection, CancellationToken, Clock, DurationMillis, EntityId,
    KnownVerificationAlarmV1, OperationControl, ParentSolveBudget, RequestId, Revision,
    Rfc3339Timestamp, ScenarioId, ScenarioSnapshotId, ScenarioSnapshotV1, SolutionId, SolveOptions,
    SolveRunId, SolveStatus,
};
use eutheto_workforce::{
    assignment_rules::{
        WorkforceAdmissionBudget, workforce_baseline_reference,
        workforce_external_baseline_reference,
    },
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[path = "headless_stored_repair.rs"]
mod repair;
use crate::application::same_accepted_original;
use repair::RepairOriginal;

#[path = "headless_stored_alternatives.rs"]
mod alternatives;
pub use alternatives::*;

/// Explicit source kind; external records cannot fall back to accepted/manual ancestry.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum StoredBaselineSourceV1 {
    #[default]
    Accepted,
    External {
        entity_id: EntityId,
    },
}

/// Caller-owned retry semantics. These are Rust service inputs, not a new wire format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredObjectiveSelectionV1 {
    pub schema_version: u32,
    pub profile_id: ObjectiveProfileId,
    pub semantic_version: u32,
    pub operation: RunObjectiveOperation,
    pub baseline_source: StoredBaselineSourceV1,
    pub alternatives_capture_id: Option<ScenarioSnapshotId>,
}

#[derive(Clone, Debug)]
pub struct StoredSolveRequest {
    pub request_id: RequestId,
    pub scenario_id: ScenarioId,
    /// Some checks a caller-observed revision; None captures the current revision under the
    /// original parent budget. Neither permits rebasing after compilation or run admission.
    pub expected_revision: Option<Revision>,
    pub options: SolveOptions,
    pub objective_selection: Option<StoredObjectiveSelectionV1>,
}

/// Store-authenticated immutable source and custody handoff for one Alternatives group.
#[derive(Clone)]
pub struct NativeObjectiveCapture {
    scenario_id: ScenarioId,
    revision: Revision,
    snapshot: Arc<ScenarioSnapshotV1>,
    snapshot_id: ScenarioSnapshotId,
    snapshot_created_at: Rfc3339Timestamp,
    custody_stamp: ScenarioCustodyStamp,
    baseline: Option<RunBaselineBinding>,
    external_baseline: Option<RunExternalBaselineBinding>,
    baseline_source: StoredBaselineSourceV1,
    original_custody: Option<OriginalRunCustody>,
}

#[derive(Clone)]
struct OriginalRunCustody {
    handle: OrdinaryPublicationHandle,
    input: Arc<RunInput>,
}
impl NativeObjectiveCapture {
    /// Creates the exact named-profile selector bound to this captured source.
    #[must_use]
    pub fn selection(
        &self,
        profile_id: ObjectiveProfileId,
        operation: RunObjectiveOperation,
    ) -> StoredObjectiveSelectionV1 {
        StoredObjectiveSelectionV1 {
            schema_version: 1,
            profile_id,
            semantic_version: 1,
            operation,
            baseline_source: self.baseline_source,
            alternatives_capture_id: Some(self.snapshot_id),
        }
    }

    /// Returns the immutable revision selected by Store.
    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// Returns the opaque Store-selected snapshot identity.
    #[must_use]
    pub const fn snapshot_id(&self) -> ScenarioSnapshotId {
        self.snapshot_id
    }
    fn admit_original_source(&self) -> Result<(), AppError> {
        if let Some(source) = &self.original_custody {
            source.handle.admit_input(&source.input)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum StoredSolveFailure {
    /// No run was started or acquired by this invocation.
    BeforeStart(AppError),
    Preparation(Box<SolvePreparationFailure>),
    /// This invocation started the run and attempted one terminal cleanup after execution failed.
    AfterStart {
        run_id: SolveRunId,
        error: AppError,
        cleanup: Result<Box<RunManifest>, AppError>,
        known_alarm: Option<KnownVerificationAlarmV1>,
    },
    /// A primary finalization failed or its commit outcome is unknown. Never blindly retry it.
    FinalizationUnconfirmed {
        run_id: SolveRunId,
        result_id: Option<SolutionId>,
        error: AppError,
        known_alarm: Option<KnownVerificationAlarmV1>,
    },
}
/// Observations exist only when this call actually performed preparation and execution.
#[derive(Debug)]
pub struct StoredSolveReport {
    pub compilation: HeadlessCompilationReport,
    pub routing: RoutingDecision,
    pub execution: RouterExecutionRecord,
}

/// Retained authority and optional observations. Secondary output cannot undo this state.
///
/// The stored result identity cannot be relabeled before secondary publication:
/// ```compile_fail
/// let _cannot_relabel: fn(&mut eutheto_core::StoredSolveOutcome) =
///     |outcome| { let _ = &mut outcome.state; };
/// ```
#[derive(Debug)]
pub struct StoredSolveOutcome {
    state: ExistingSolveRunV1,
    pub reused: bool,
    pub report: Option<StoredSolveReport>,
    pub warning: Option<AppError>,
    // Only a freshly loaded store-owned portable record may be published after the primary commit.
    portable: Option<PortableAcceptedResult>,
}

/// An accepted database result remains committed even when this secondary publication fails.
#[derive(Debug)]
pub struct StoredResultPublication {
    pub result_id: SolutionId,
    pub publication: Result<(), AppError>,
}

impl StoredSolveOutcome {
    /// Retained database authority is read-only, including the committed result identity.
    #[must_use]
    pub fn state(&self) -> &ExistingSolveRunV1 {
        &self.state
    }

    #[must_use]
    pub fn accepted_result_id(&self) -> Option<SolutionId> {
        match self
            .state
            .manifest
            .as_ref()
            .map(|manifest| &manifest.outcome)
        {
            Some(RunTerminalOutcomeV1::Accepted { solution_id, .. }) => Some(*solution_id),
            _ => None,
        }
    }

    #[must_use]
    pub fn portable_result(&self) -> Option<&PortableAcceptedResult> {
        self.portable.as_ref()
    }

    /// Publishes only the reloaded durable result, never an in-memory precommit candidate.
    /// Cancellation or output failure is a warning-bearing secondary outcome with the result ID.
    #[must_use]
    pub fn publish_result(
        &self,
        destination: &Path,
        cancellation: &CancellationToken,
    ) -> Option<StoredResultPublication> {
        let result_id = self.accepted_result_id()?;
        let publication = (|| {
            let portable = self.portable.as_ref().ok_or_else(result_reload_warning)?;
            let control = OperationControl::Cancellation(cancellation.clone());
            let prepared = prepare_json_atomic_controlled(destination, portable, &control)
                .map_err(|error| export_error(&error))?;
            prepared
                .publish_controlled(&control)
                .map_err(|error| export_error(&error))
        })();
        Some(StoredResultPublication {
            result_id,
            publication,
        })
    }
}

enum StoredSolveAdmission {
    Cancellation(CancellationToken),
    Budget {
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
    },
    AlternativesMember {
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
        remaining_members: u8,
    },
}

pub type StoredSolveBoundObserver = Arc<dyn Fn(SolveRunId) + Send + Sync>;

impl EuthetoApp {
    /// Reuses this application's exact registries and injected clocks without opening another DB.
    #[must_use]
    pub fn headless_service(&self) -> HeadlessService {
        HeadlessService {
            packs: Arc::clone(&self.pack_registry),
            solvers: Arc::clone(&self.solver_registry),
            clock: Arc::clone(&self.clock),
            monotonic_clock: Arc::clone(&self.monotonic_clock),
            ids: Arc::clone(&self.ids),
        }
    }
    /// Freezes the explicitly selected Workforce baseline and exact Store source for one group.
    ///
    /// Pass the group's original start and parent operation control so source verification
    /// cannot reset or outlive its deadline. Like ordinary solve captures, the snapshot uses
    /// that operation's start anchor; every member retains the same snapshot/start chronology.
    ///
    /// # Errors
    /// Rejects stale or inadmissible source custody, invalid baselines, and interrupted work.
    pub async fn capture_objective_alternatives_source(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Option<Revision>,
        baseline_source: StoredBaselineSourceV1,
        control: OperationControl,
        started_at: Rfc3339Timestamp,
    ) -> Result<NativeObjectiveCapture, AppError> {
        let mut budget = WorkforceAdmissionBudget::new(Some(&control));
        self.capture_native_objective_source(
            scenario_id,
            expected_revision,
            control.clone(),
            true,
            baseline_source,
            started_at,
            None,
            &mut budget,
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn capture_native_objective_source(
        &self,
        scenario_id: ScenarioId,
        expected_revision: Option<Revision>,
        control: OperationControl,
        require_baseline: bool,
        baseline_source: StoredBaselineSourceV1,
        snapshot_created_at: Rfc3339Timestamp,
        requested_original: Option<RepairOriginal>,
        budget: &mut WorkforceAdmissionBudget<'_>,
    ) -> Result<NativeObjectiveCapture, AppError> {
        control.check().map_err(operation_interrupted)?;
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
        let mut original_custody = None;
        let mut accepted_source = None;
        let mut external_baseline = None;
        if !require_baseline && baseline_source != StoredBaselineSourceV1::Accepted {
            return Err(objective_selection_error());
        }
        let (baseline, receipt_command_id, manual_record) = if require_baseline {
            budget.reserve_json(&project.document, 2).map_err(|error| {
                super::super::super::manual::manual_budget_error(
                    error,
                    "/domain",
                    "The Workforce document exceeds native resource limits.",
                )
            })?;
            if project.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
                return Err(objective_selection_error());
            }
            if let StoredBaselineSourceV1::External { entity_id } = baseline_source {
                if requested_original.is_some() {
                    return Err(objective_selection_error());
                }
                external_baseline = Some(super::repair::capture_external_baseline(
                    &project.document,
                    entity_id,
                    budget,
                )?);
                (None, None, None)
            } else {
                let manual;
                let (original, captured_original) = if let Some(requested) = requested_original {
                    accepted_source = requested.accepted_source;
                    original_custody = requested.custody;
                    manual = super::super::super::manual::find_manual_selection(&project.document)?;
                    if manual.is_none() {
                        super::repair::preflight_captured_original(&requested.portable, budget)?;
                    }
                    let (mut original, record) =
                        super::repair::original_record(&requested.portable, budget)?;
                    if manual.is_some() {
                        if accepted_source.is_none() {
                            return Err(solution_verification_mismatch());
                        }
                        let base = super::super::super::workforce_base_schedule(&project.document)?
                            .ok_or_else(objective_baseline_error)?;
                        if base.source_solution_id != original.accepted_result.solution_id
                            || base.source_revision.value() != original.source_revision
                            || budget
                                .pair_content_hash(&base.assignments)
                                .map_err(super::super::super::manual::manual_integrity_error)?
                                != original.pair_content_hash
                        {
                            return Err(solution_verification_mismatch());
                        }
                        original.entity = workforce_baseline_reference(base.id, false)
                            .map_err(|_| solution_contract_error())?;
                        (original, None)
                    } else {
                        let captured =
                            super::repair::capture_original_record(original, record, budget)?;
                        (captured.original, captured.captured_original)
                    }
                } else {
                    let base = super::super::super::workforce_base_schedule(&project.document)?
                        .ok_or_else(objective_baseline_error)?;
                    let stored = self
                        .load_solution(scenario_id, base.source_solution_id)
                        .await?;
                    budget.preflight(&stored.portable).map_err(|error| {
                        super::super::super::manual::manual_budget_error(
                            error,
                            "/original",
                            "The accepted source exceeds native resource limits.",
                        )
                    })?;
                    budget
                        .reserve_json(&stored.portable.run_input, 1)
                        .map_err(|error| {
                            super::super::super::manual::manual_budget_error(
                                error,
                                "/original/runInput",
                                "The accepted source exceeds native resource limits.",
                            )
                        })?;
                    let ordinary_input = stored.portable.run_input.clone();
                    let ordinary_reference = eutheto_domain_ir::AcceptedResultRefV1::from_result(
                        &stored.portable.accepted_result,
                    )
                    .map_err(|_| solution_verification_mismatch())?;
                    let ordinary =
                        self.ordinary_reference_handle(scenario_id, &ordinary_reference)?;
                    if let Some(handle) = &ordinary {
                        handle.admit_input(&ordinary_input)?;
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
                    if let Some(handle) = &ordinary {
                        handle.admit_input(&ordinary_input)?;
                    }
                    let reference = eutheto_domain_ir::AcceptedResultRefV1::from_result(
                        &accepted.portable.accepted_result,
                    )
                    .map_err(|_| solution_verification_mismatch())?;
                    let original = super::super::super::manual::authenticated_original(
                        scenario_id,
                        &base,
                        &accepted.portable.accepted_result,
                        &accepted.portable.run_input,
                        &accepted.portable.run_manifest,
                        &reference,
                        budget,
                    )?;
                    accepted_source = Some(AcceptedSourceExpectationV1::from_stored(&accepted));
                    original_custody = ordinary.map(|handle| OriginalRunCustody {
                        handle,
                        input: Arc::new(ordinary_input),
                    });
                    manual = super::super::super::manual::find_manual_selection(&project.document)?;
                    (original, None)
                };
                let (selection, selected_pair_content_hash, receipt_command_id) =
                    if let Some((manual_id, _, manual)) = &manual {
                        if manual.id != *manual_id
                            || manual.schema_version != 1
                            || manual.admission_source_scenario_id != scenario_id
                            || manual.original != original
                            || budget
                                .manual_selection_checksum(manual)
                                .map_err(super::super::super::manual::manual_integrity_error)?
                                != manual.checksum
                        {
                            return Err(solution_verification_mismatch());
                        }
                        (
                            RunBaselineSelection::ManualSelection {
                                entity: workforce_baseline_reference(manual.id, true)
                                    .map_err(|_| solution_contract_error())?,
                                record_checksum: manual.checksum.clone(),
                                admission_source_scenario_id: manual.admission_source_scenario_id,
                                admission_source_revision: manual.admission_source_revision.value(),
                                admission_source_document_hash: manual
                                    .admission_source_document_hash
                                    .clone(),
                            },
                            budget
                                .pair_content_hash(&manual.assignments)
                                .map_err(super::super::super::manual::manual_integrity_error)?,
                            Some(manual.admission_command_id),
                        )
                    } else {
                        (
                            RunBaselineSelection::OriginalAccepted {},
                            original.pair_content_hash.clone(),
                            None,
                        )
                    };
                let baseline = RunBaselineBinding {
                    original,
                    selection,
                    selected_pair_content_hash,
                    captured_original,
                };
                budget
                    .reserve_json(&baseline, 1)
                    .map_err(super::super::super::manual::manual_integrity_error)?;
                baseline
                    .validate_for(scenario_id)
                    .map_err(|_| solution_verification_mismatch())?;
                (Some(baseline), receipt_command_id, manual)
            }
        } else {
            (None, None, None)
        };
        control.check().map_err(operation_interrupted)?;
        let (capture, receipt) = self
            .store
            .capture_solve_snapshot(
                scenario_id,
                project.summary.revision,
                project.document.clone(),
                snapshot_created_at,
                receipt_command_id,
                control.clone(),
                accepted_source,
            )
            .await
            .map_err(store_error)?;
        if let Some((manual_id, value, manual)) = manual_record {
            let receipt = receipt.ok_or_else(solution_verification_mismatch)?;
            super::super::super::manual::authenticate_manual_receipt(
                receipt,
                manual_id,
                &value,
                &manual,
                scenario_id,
                budget,
            )?;
        } else if receipt.is_some() {
            return Err(solution_verification_mismatch());
        }
        if let Some(source) = &original_custody {
            source.handle.admit_input(&source.input)?;
        }
        control.check().map_err(operation_interrupted)?;
        let project = capture.project;
        let mut snapshot = ScenarioSnapshotV1::current(
            project.summary.revision,
            project.document,
            project.portable.required_capabilities,
        );
        snapshot.semantic_extensions = project.portable.semantic_extensions;
        snapshot.extensions = project.portable.extensions;
        Ok(NativeObjectiveCapture {
            scenario_id,
            revision: snapshot.revision,
            snapshot: Arc::new(snapshot),
            snapshot_id: capture.snapshot_id,
            snapshot_created_at: capture.snapshot_created_at,
            custody_stamp: capture.stamp,
            baseline,
            external_baseline,
            baseline_source,
            original_custody,
        })
    }

    /// Looks up one native request without requiring an executable backend or rebasing its source.
    ///
    /// Native requests do not select an explanation policy. An exact retry inherits its originally
    /// recorded policy, while new requests retain the current default. All other options and source
    /// bindings remain exact. The caller must authenticate its window operation first.
    ///
    /// # Errors
    ///
    /// Rejects invalid solve options, unavailable or corrupt persisted run state,
    /// and retry requests that disagree with the original immutable input.
    pub async fn solve_lookup_native_request(
        &self,
        request: &mut StoredSolveRequest,
    ) -> Result<Option<ExistingSolveRunV1>, AppError> {
        request
            .options
            .validate()
            .map_err(|_| solve_error("solve.options_invalid", "Solve options are invalid."))?;
        validate_objective_selection(request.objective_selection.as_ref())?;
        let Some(existing) = self
            .store
            .load_solve_run_by_request(request.request_id)
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        let requested_mode = request.options.explanation_mode;
        request.options.explanation_mode = existing.input.solve_options.explanation_mode;
        if let Err(error) = validate_retry(request, &existing) {
            request.options.explanation_mode = requested_mode;
            return Err(error);
        }
        Ok(Some(existing))
    }

    /// Owns one finite ordinary solve using an application-owned child token.
    ///
    /// Native callers that need a window/job token should use
    /// [`Self::solve_stored_with_cancellation`]. The legacy entry point remains
    /// app-shutdown aware for the standalone CLI and existing embedders.
    ///
    /// # Errors
    ///
    /// Returns a typed before-start or terminal failure when preparation,
    /// execution, cancellation, or acceptance fails.
    pub async fn solve_stored(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        self.solve_stored_inner(
            request,
            progress,
            StoredSolveAdmission::Cancellation(self.cancellation.child()),
            None,
            None,
            None,
        )
        .await
    }

    /// Owns one finite ordinary solve with a caller/job cancellation token.
    ///
    /// The token is distinct from presentation observer release while still
    /// inheriting application shutdown when the caller supplies a child of the
    /// native window token.
    ///
    /// # Errors
    ///
    /// Returns a typed before-start or terminal failure when preparation,
    /// execution, cancellation, or acceptance fails.
    pub async fn solve_stored_with_cancellation(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
        cancellation: CancellationToken,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        self.solve_stored_inner(
            request,
            progress,
            StoredSolveAdmission::Cancellation(cancellation),
            None,
            None,
            None,
        )
        .await
    }

    /// Owns one finite solve whose parent budget was started before native
    /// admission. This is the desktop path; CLI callers retain the legacy
    /// entry point and therefore preserve their existing budget anchor.
    ///
    /// # Errors
    ///
    /// Returns a typed before-start or terminal failure when preparation,
    /// execution, cancellation, or acceptance fails.
    pub async fn solve_stored_with_budget(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        self.solve_stored_inner(
            request,
            progress,
            StoredSolveAdmission::Budget { budget, started_at },
            None,
            None,
            None,
        )
        .await
    }

    /// Desktop variant that reports the durable run identity as soon as the
    /// start transaction binds it. The callback is advisory and cannot alter
    /// the stored authority.
    ///
    /// # Errors
    ///
    /// Returns a typed before-start or terminal failure when preparation,
    /// execution, cancellation, or acceptance fails.
    pub async fn solve_stored_with_budget_observer(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
        observer: StoredSolveBoundObserver,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        self.solve_stored_inner(
            request,
            progress,
            StoredSolveAdmission::Budget { budget, started_at },
            Some(observer),
            None,
            None,
        )
        .await
    }

    /// Runs one member of a retained native Alternatives capture without recapturing its source.
    ///
    /// `remaining_members` must be 1..=4, including this member. Its worker share is fixed at
    /// first dispatch; preparation and ordinary acceptance keep the unchanged parent deadline.
    /// # Errors
    /// Rejects invalid member counts, mismatched captures, source admission failures, and
    /// failed or interrupted solves.
    #[allow(clippy::too_many_arguments)]
    pub async fn solve_stored_with_budget_observer_in_capture(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
        budget: ParentSolveBudget,
        started_at: Rfc3339Timestamp,
        observer: StoredSolveBoundObserver,
        capture: &NativeObjectiveCapture,
        remaining_members: u8,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        if !(1..=4).contains(&remaining_members) {
            return Err(StoredSolveFailure::BeforeStart(solve_error(
                "solve.alternatives_remaining_members_invalid",
                "The remaining Alternatives member count must be between one and four.",
            )));
        }
        self.solve_stored_inner(
            request,
            progress,
            StoredSolveAdmission::AlternativesMember {
                budget,
                started_at,
                remaining_members,
            },
            Some(observer),
            Some(capture.clone()),
            None,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn solve_stored_inner(
        &self,
        request: StoredSolveRequest,
        progress: &mut dyn ProgressSink,
        admission: StoredSolveAdmission,
        observer: Option<StoredSolveBoundObserver>,
        native_capture: Option<NativeObjectiveCapture>,
        requested_original: Option<Arc<PortableAcceptedResult>>,
    ) -> Result<StoredSolveOutcome, StoredSolveFailure> {
        Box::pin(async move {
            request.options.validate().map_err(|_| {
                StoredSolveFailure::BeforeStart(solve_error(
                    "solve.options_invalid",
                    "Solve options are invalid.",
                ))
            })?;
            validate_objective_selection(request.objective_selection.as_ref())
                .map_err(StoredSolveFailure::BeforeStart)?;
            let mut reuse_custody = OrdinaryReuseCustody::new();
            reuse_custody.requested_original = requested_original;
            reuse_custody.original_custody = native_capture
                .as_ref()
                .and_then(|capture| capture.original_custody.clone());
            reuse_custody
                .retain_request_handle(self, request.scenario_id, request.request_id)
                .map_err(StoredSolveFailure::BeforeStart)?;
            if let Some(existing) = self
                .store
                .load_solve_run_by_request(request.request_id)
                .await
                .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
            {
                return load_reused_output(
                    self,
                    &request,
                    existing,
                    true,
                    None,
                    &mut reuse_custody,
                    observer.as_ref(),
                )
                .await
                .map_err(StoredSolveFailure::BeforeStart);
            }
            if request
                .objective_selection
                .as_ref()
                .is_some_and(|selection| selection.alternatives_capture_id.is_some())
                && native_capture.is_none()
            {
                return Err(StoredSolveFailure::BeforeStart(objective_capture_missing()));
            }
            if let Some(capture) = &native_capture {
                validate_native_capture(
                    &request,
                    capture,
                    reuse_custody.requested_original.as_deref(),
                )
                .map_err(StoredSolveFailure::BeforeStart)?;
            }
            let remaining_members = match &admission {
                StoredSolveAdmission::AlternativesMember {
                    remaining_members, ..
                } => Some(*remaining_members),
                _ => None,
            };
            let mut operation = match admission {
                StoredSolveAdmission::Cancellation(cancellation) => self
                    .headless_service()
                    .begin_solve(request.request_id, request.options.clone(), cancellation),
                StoredSolveAdmission::Budget { budget, started_at }
                | StoredSolveAdmission::AlternativesMember {
                    budget, started_at, ..
                } => self.headless_service().begin_solve_with_budget(
                    request.request_id,
                    request.options.clone(),
                    budget,
                    started_at,
                ),
            }
            .map_err(StoredSolveFailure::BeforeStart)?;
            operation.remaining_members = remaining_members;
            let capture = if let Some(capture) = native_capture {
                capture
            } else {
                let needs_baseline =
                    objective_selection_requires_baseline(request.objective_selection.as_ref());
                let control = operation.control();
                let mut budget = WorkforceAdmissionBudget::new(Some(&control));
                self.capture_native_objective_source(
                    request.scenario_id,
                    request.expected_revision,
                    operation.control(),
                    needs_baseline,
                    request
                        .objective_selection
                        .as_ref()
                        .map_or(StoredBaselineSourceV1::Accepted, |selection| {
                            selection.baseline_source
                        }),
                    operation.started_at,
                    None,
                    &mut budget,
                )
                .await
                .map_err(StoredSolveFailure::BeforeStart)?
            };
            let snapshot = capture.snapshot.as_ref().clone();
            let objective_context = objective_context_for_request(&request, &capture)
                .map_err(StoredSolveFailure::BeforeStart)?;
            let prepared = operation
                .prepare_captured_with_objective_context(snapshot, None, Some(objective_context))
                .map_err(StoredSolveFailure::Preparation)?;
            prepared
                .operation
                .control()
                .check()
                .map_err(|reason| StoredSolveFailure::BeforeStart(operation_interrupted(reason)))?;
            capture
                .admit_original_source()
                .map_err(StoredSolveFailure::BeforeStart)?;
            let _ = self.reconcile_ordinary_solve_custody().await;
            let start_request = prepared.store_request();
            let ordinary_lease = match self.reserve_ordinary_solve_custody(&start_request) {
                Ok(lease) => lease,
                Err(reservation_error) => {
                    reuse_custody
                        .retain_request_handle(self, request.scenario_id, request.request_id)
                        .map_err(StoredSolveFailure::BeforeStart)?;
                    if let Some(existing) = self
                        .store
                        .load_solve_run_by_request(request.request_id)
                        .await
                        .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
                    {
                        return load_reused_output(
                            self,
                            &request,
                            existing,
                            true,
                            None,
                            &mut reuse_custody,
                            observer.as_ref(),
                        )
                        .await
                        .map_err(StoredSolveFailure::BeforeStart);
                    }
                    return Err(StoredSolveFailure::BeforeStart(reservation_error));
                }
            };
            let started = match self
                .store
                .start_solve_run(
                    start_request,
                    capture.custody_stamp.clone(),
                    Some(capture.snapshot_id),
                    prepared.operation.control(),
                )
                .await
            {
                Ok(started) => started,
                Err(error @ StoreError::SolveRequestIdConflict { .. }) => {
                    let _ = ordinary_lease.release_unowned_start();
                    // A concurrent owner may have bound Auto under a different current runtime.
                    // Reconcile the caller's original semantics by reading; do not reroute or retry start.
                    reuse_custody
                        .retain_request_handle(self, request.scenario_id, request.request_id)
                        .map_err(StoredSolveFailure::BeforeStart)?;
                    if let Some(existing) = self
                        .store
                        .load_solve_run_by_request(request.request_id)
                        .await
                        .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
                    {
                        return load_reused_output(
                            self,
                            &request,
                            existing,
                            true,
                            None,
                            &mut reuse_custody,
                            observer.as_ref(),
                        )
                        .await
                        .map_err(StoredSolveFailure::BeforeStart);
                    }
                    return Err(StoredSolveFailure::BeforeStart(store_error(error)));
                }
                Err(error) => {
                    if known_precommit_rejection(&error) {
                        let _ = ordinary_lease.release_unowned_start();
                        return Err(StoredSolveFailure::BeforeStart(store_error(error)));
                    }
                    let start_error = store_error(error);
                    reuse_custody
                        .retain_request_handle(self, request.scenario_id, request.request_id)
                        .map_err(StoredSolveFailure::BeforeStart)?;
                    let Ok(existing) = self
                        .store
                        .load_solve_run_by_request(request.request_id)
                        .await
                    else {
                        return Err(StoredSolveFailure::BeforeStart(start_error));
                    };
                    let Some(existing) = existing else {
                        let _ = ordinary_lease.release_unowned_start();
                        return Err(StoredSolveFailure::BeforeStart(start_error));
                    };
                    if let Err(error) = validate_retry(&request, &existing) {
                        if existing.input.run_id != prepared.operation.run_id {
                            let _ = ordinary_lease.release_unowned_start();
                        }
                        return Err(StoredSolveFailure::BeforeStart(error));
                    }
                    if existing.input.run_id != prepared.operation.run_id
                        || existing.manifest.is_some()
                    {
                        let _ = ordinary_lease.release_unowned_start();
                        return load_reused_output(
                            self,
                            &request,
                            existing,
                            true,
                            None,
                            &mut reuse_custody,
                            observer.as_ref(),
                        )
                        .await
                        .map_err(StoredSolveFailure::BeforeStart);
                    }
                    if ordinary_lease
                        .bind_started(&existing.input, existing.started_at)
                        .is_err()
                    {
                        return Err(StoredSolveFailure::BeforeStart(start_error));
                    }
                    if let Some(observer) = observer.as_ref()
                        && let Err(payload) =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                observer(existing.input.run_id);
                            }))
                    {
                        drop(payload);
                    }
                    let owner = SolveOwner {
                        input: existing.input.clone(),
                        budget: prepared.operation.budget.clone(),
                        started_at: existing.started_at,
                        clock: Arc::clone(&self.clock),
                    };
                    let cleanup = owner
                        .cleanup(&self.store, None, Some(&ordinary_lease))
                        .await
                        .map(Box::new);
                    let _ = self.reconcile_ordinary_solve_custody().await;
                    return Err(StoredSolveFailure::AfterStart {
                        run_id: existing.input.run_id,
                        error: start_error,
                        cleanup,
                        known_alarm: None,
                    });
                }
            };
            if started.reused {
                capture
                    .admit_original_source()
                    .map_err(StoredSolveFailure::BeforeStart)?;
                let _ = ordinary_lease.release_unowned_start();
                reuse_custody
                    .retain_request_handle(self, request.scenario_id, request.request_id)
                    .map_err(StoredSolveFailure::BeforeStart)?;
                let existing = self
                    .store
                    .load_solve_run_by_request(request.request_id)
                    .await
                    .map_err(|error| StoredSolveFailure::BeforeStart(store_error(error)))?
                    .ok_or_else(|| StoredSolveFailure::BeforeStart(solution_contract_error()))?;
                return load_reused_output(
                    self,
                    &request,
                    existing,
                    true,
                    None,
                    &mut reuse_custody,
                    observer.as_ref(),
                )
                .await
                .map_err(StoredSolveFailure::BeforeStart);
            }
            let owner = SolveOwner {
                input: started.input.clone(),
                budget: prepared.operation.budget.clone(),
                started_at: started.started_at,
                clock: Arc::clone(&self.clock),
            };
            if let Err(error) = ordinary_lease.bind_started(&started.input, started.started_at) {
                let cleanup = owner.cleanup(&self.store, None, None).await.map(Box::new);
                let _ = self.reconcile_ordinary_solve_custody().await;
                return Err(StoredSolveFailure::AfterStart {
                    run_id: started.input.run_id,
                    error,
                    cleanup,
                    known_alarm: None,
                });
            }
            if let Err(error) = capture.admit_original_source() {
                let cleanup = owner
                    .cleanup(&self.store, None, Some(&ordinary_lease))
                    .await
                    .map(Box::new);
                let _ = self.reconcile_ordinary_solve_custody().await;
                return Err(StoredSolveFailure::AfterStart {
                    run_id: started.input.run_id,
                    error,
                    cleanup,
                    known_alarm: None,
                });
            }
            let observer_failed = if let Some(observer) = observer.as_ref() {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    observer(started.input.run_id);
                })) {
                    Ok(()) => false,
                    Err(payload) => {
                        drop(payload);
                        true
                    }
                }
            } else {
                false
            };
            // Only this branch owns finalization. All post-start execution errors enter cleanup.
            let mut alarm_custody: Option<NativeAlarmCustody> = None;
            let mut terminal_observation_unusable = false;
            let execution = async {
                if observer_failed {
                    return Err(solution_contract_error());
                }
                if started.started_at != prepared.operation.started_at {
                    return Err(solution_contract_error());
                }
                let loaded = self
                    .store
                    .load_solve_input(started.input.run_id)
                    .await
                    .map_err(store_error)?;
                if loaded.input != started.input
                    || loaded.document != prepared.snapshot.document
                    || loaded.input.snapshot_id != capture.snapshot_id
                    || loaded.input.snapshot_created_at != capture.snapshot_created_at
                    || loaded.input.snapshot_document_hash != prepared.document_hash
                {
                    return Err(solution_contract_error());
                }
                let (sender, checkpoint_task) = if request.options.collect_intermediate_solutions {
                    let work = ordinary_lease.checkpoint_work()?;
                    let (sender, mut receiver) = tokio::sync::mpsc::channel::<FirstAccepted>(1);
                    let app = self.clone();
                    let input = started.input.clone();
                    let task = tokio::spawn(async move {
                        let Some(first) = receiver.recv().await else {
                            return Ok::<_, AppError>(false);
                        };
                        let evidence = accepted_evidence(&first.result);
                        let checkpoint = VerifiedCheckpoint::new(
                            input.clone(),
                            first.observed_after_milliseconds,
                            first.verified_after_milliseconds,
                            first.accepted_at,
                            first.result,
                            evidence,
                        )
                        .map_err(|_| solution_contract_error())?;
                        let committed = app
                            .store
                            .save_verified_checkpoint(input.run_id, checkpoint)
                            .await
                            .map_err(store_error)?;
                        app.publish_solve_verified(&committed, &work)?;
                        Ok(true)
                    });
                    (Some(sender), Some(task))
                } else {
                    (None, None)
                };
                let executed = prepared
                    .execute_bound(
                        loaded.input,
                        progress,
                        sender,
                        &mut alarm_custody,
                        Some(&ordinary_lease),
                    )
                    .await;
                let checkpoint_published = if let Some(task) = checkpoint_task {
                    task.await.map_err(join_error).and_then(|result| result)
                } else {
                    Ok(false)
                };
                let (checkpoint_published, mut outcome) = match executed {
                    Err(error) => {
                        let _ = checkpoint_published;
                        return Err(error);
                    }
                    Ok(outcome) => (checkpoint_published?, outcome),
                };
                if let Some(custody) = alarm_custody.as_mut() {
                    custody.retain_elapsed_bound(outcome.last_observed_elapsed);
                    custody.retain_wall_observation(outcome.manifest.finished_at);
                } else if let Some(alarm) = outcome.alarm.clone() {
                    let mut custody = NativeAlarmCustody::new(
                        &outcome.run_input,
                        outcome.operation.started_at,
                        alarm,
                        Some(outcome.last_observed_elapsed),
                    );
                    custody.retain_wall_observation(outcome.manifest.finished_at);
                    alarm_custody = Some(custody);
                }
                if let Some(alarm) = outcome.alarm.as_ref() {
                    ordinary_lease.capture_alarm(alarm);
                }
                merge_ordinary_alarm_observations(&ordinary_lease, &mut alarm_custody)?;
                if checkpoint_published && alarm_custody.is_some() {
                    let finished_at = observed_wall(self.clock.as_ref())?;
                    let elapsed = checked_elapsed_safely(&owner.budget);
                    ordinary_lease.observe_wall(finished_at);
                    if let Some(elapsed) = elapsed {
                        ordinary_lease.observe_elapsed(elapsed);
                    }
                    merge_ordinary_alarm_observations(&ordinary_lease, &mut alarm_custody)?;
                    let custody = alarm_custody.as_ref().ok_or_else(solution_contract_error)?;
                    let trusted_elapsed = trusted_alarm_elapsed(custody, finished_at, elapsed);
                    outcome.manifest =
                        build_alarm_cleanup_manifest(custody, finished_at, trusted_elapsed)?
                            .with_native_observations(outcome.manifest.native_observations.clone())
                            .map_err(|_| solution_contract_error())?;
                }
                if let Err(error) = outcome.refresh_admission_with_lease(Some(&ordinary_lease)) {
                    terminal_observation_unusable = true;
                    return Err(error);
                }
                Ok(outcome)
            }
            .await;
            let outcome = match execution {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = merge_ordinary_alarm_observations(&ordinary_lease, &mut alarm_custody);
                    let cleanup = if terminal_observation_unusable && alarm_custody.is_none() {
                        // The failed actual pair cannot be repaired by another ordinary sample.
                        Err(timing_error())
                    } else {
                        owner
                            .cleanup(&self.store, alarm_custody.as_ref(), Some(&ordinary_lease))
                            .await
                            .map(Box::new)
                    };
                    let _ = self.reconcile_ordinary_solve_custody().await;
                    let known_alarm = alarm_custody.as_ref().and_then(|custody| {
                        let terminalization = if cleanup.is_ok() {
                            eutheto_types::VerificationAlarmTerminalization::Confirmed
                        } else {
                            eutheto_types::VerificationAlarmTerminalization::Unconfirmed
                        };
                        let cause = safe_verification_secondary_cause(&error).or_else(|| {
                            cleanup
                                .as_ref()
                                .err()
                                .and_then(safe_verification_secondary_cause)
                        });
                        if let Some(cause) = cause {
                            ordinary_lease.retain_secondary_cause(
                                eutheto_types::KnownVerificationSecondaryCauseCode::from_safe_code(
                                    cause,
                                ),
                            );
                        }
                        known_verification_alarm_metadata(custody, terminalization, cause)
                    });
                    return Err(StoredSolveFailure::AfterStart {
                        run_id: owner.input.run_id,
                        error,
                        cleanup,
                        known_alarm,
                    });
                }
            };
            let HeadlessSolveOutcome {
                run_input,
                manifest,
                compilation,
                routing,
                execution,
                accepted,
                evidence,
                infeasibility,
                ..
            } = outcome;
            let result_id = accepted.as_ref().map(|result| result.solution.solution_id);
            let report = Some(StoredSolveReport {
                compilation,
                routing,
                execution,
            });
            let finalization = match accepted {
                Some(accepted) => {
                    self.store
                        .finalize_accepted_run(accepted, manifest.clone(), evidence)
                        .await
                }
                None if matches!(
                    manifest.outcome,
                    RunTerminalOutcomeV1::VerificationAlarm { .. }
                ) =>
                {
                    self.store
                        .finalize_quarantined_run(
                            manifest.clone(),
                            CandidateDiagnosticsV1::default(),
                        )
                        .await
                }
                None => {
                    let retained = infeasibility
                        .map(|material| material.bind(&run_input, &manifest))
                        .transpose()
                        .map_err(|error| StoredSolveFailure::FinalizationUnconfirmed {
                            run_id: run_input.run_id,
                            result_id: None,
                            error,
                            known_alarm: None,
                        })?
                        .flatten();
                    self.store
                        .finalize_terminal_run(manifest.clone(), retained)
                        .await
                }
            };
            if let Err(error) = finalization {
                if let Some(custody) = alarm_custody.as_ref() {
                    if let Ok(existing) =
                        load_compatible_alarm_readback(&self.store, &run_input, &manifest, custody)
                            .await
                    {
                        let outcome =
                            load_committed_output(&self.store, existing, false, report).await;
                        let _ = self.reconcile_ordinary_solve_custody().await;
                        return Ok(outcome);
                    }
                    let error = store_error(error);
                    let cause = safe_verification_secondary_cause(&error)
                        .or(Some("storage.operation_failed"));
                    if let Some(cause) = cause {
                        ordinary_lease.retain_secondary_cause(
                            eutheto_types::KnownVerificationSecondaryCauseCode::from_safe_code(
                                cause,
                            ),
                        );
                    }
                    let _ = self.reconcile_ordinary_solve_custody().await;
                    let known_alarm = known_verification_alarm_metadata(
                        custody,
                        eutheto_types::VerificationAlarmTerminalization::Unconfirmed,
                        cause,
                    );
                    return Err(StoredSolveFailure::FinalizationUnconfirmed {
                        run_id: run_input.run_id,
                        result_id: None,
                        error,
                        known_alarm,
                    });
                }
                if matches!(error, StoreError::SolveRunTerminalConflict(_))
                    && reuse_custody
                        .retain_request_handle(self, request.scenario_id, request.request_id)
                        .is_ok()
                    && let Ok(Some(existing)) = self
                        .store
                        .load_solve_run_by_request(run_input.request_id)
                        .await
                    && existing.input == run_input
                    && existing.manifest.is_some()
                {
                    let existing_result_id =
                        existing
                            .manifest
                            .as_ref()
                            .and_then(|manifest| match &manifest.outcome {
                                RunTerminalOutcomeV1::Accepted { solution_id, .. } => {
                                    Some(*solution_id)
                                }
                                _ => None,
                            });
                    let original_error = store_error(error);
                    match load_reused_output(
                        self,
                        &request,
                        existing,
                        false,
                        report,
                        &mut reuse_custody,
                        None,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            let _ = self.reconcile_ordinary_solve_custody().await;
                            return Ok(outcome);
                        }
                        Err(admission_error) => {
                            let known_alarm = known_alarm_from_error(&admission_error);
                            let _ = self.reconcile_ordinary_solve_custody().await;
                            return Err(StoredSolveFailure::FinalizationUnconfirmed {
                                run_id: run_input.run_id,
                                result_id: existing_result_id,
                                error: original_error,
                                known_alarm,
                            });
                        }
                    }
                }
                if known_precommit_rejection(&error) {
                    let _ = merge_ordinary_alarm_observations(&ordinary_lease, &mut alarm_custody);
                    let cleanup = owner
                        .cleanup(&self.store, None, Some(&ordinary_lease))
                        .await
                        .map(Box::new);
                    let _ = self.reconcile_ordinary_solve_custody().await;
                    return Err(StoredSolveFailure::AfterStart {
                        run_id: run_input.run_id,
                        error: store_error(error),
                        cleanup,
                        known_alarm: None,
                    });
                }
                let error = store_error(error);
                let _ = self.reconcile_ordinary_solve_custody().await;
                return Err(StoredSolveFailure::FinalizationUnconfirmed {
                    run_id: run_input.run_id,
                    result_id,
                    error,
                    known_alarm: None,
                });
            }
            let state = ExistingSolveRunV1 {
                input: run_input,
                started_at: manifest.started_at,
                manifest: Some(manifest),
            };
            let outcome = load_committed_output(&self.store, state, false, report).await;
            let _ = self.reconcile_ordinary_solve_custody().await;
            Ok(outcome)
        })
        .await
    }
}

fn known_precommit_rejection(error: &StoreError) -> bool {
    if matches!(
        error,
        StoreError::InvalidPersistedRun(_)
            | StoreError::InvalidPersistedResult(_)
            | StoreError::InvalidPersistedDiagnostic(_)
            | StoreError::SnapshotMismatch(_)
            | StoreError::InvalidScenarioIdentity(_)
            | StoreError::IdentityCollision(_)
            | StoreError::NumericRange
            | StoreError::Json(_)
    ) {
        return true;
    }
    #[cfg(debug_assertions)]
    if matches!(error, StoreError::InjectedFailure) {
        return true;
    }
    false
}

fn merge_ordinary_alarm_observations(
    lease: &OrdinarySolveLease,
    custody: &mut Option<NativeAlarmCustody>,
) -> Result<(), AppError> {
    let bounds = lease.observation_bounds()?;
    if let Some(custody) = custody {
        if let Some(elapsed) = bounds.latest_elapsed {
            custody.retain_elapsed_bound(elapsed);
        }
        if let Some(wall) = bounds.latest_wall {
            custody.retain_wall_observation(wall);
        }
    }
    Ok(())
}

impl PreparedSolve {
    fn store_request(&self) -> NewSolveRunV1 {
        NewSolveRunV1 {
            objective_context: Some(self.objective_context.clone()),
            run_id: self.operation.run_id,
            request_id: self.operation.request_id,
            scenario_id: self.snapshot.document.scenario_id,
            expected_revision: self.snapshot.revision,
            planning_ir_schema_version: self.problem.schema_version,
            compiler_version: self.problem.metadata.compiler_version.clone(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            backend_id: self.identity.backend_id().clone(),
            backend_version: self.identity.backend_version().to_owned(),
            adapter_version: self.identity.adapter_version().to_owned(),
            worker_version: self.identity.worker_version().to_owned(),
            solver_version: self.identity.solver_version().to_owned(),
            protocol_major: self.identity.protocol_major(),
            protocol_minor: self.identity.protocol_minor(),
            model_hash: self.report.summary.canonical_ir_hash.clone(),
            objective_policy_hash: self.objective_policy_hash.clone(),
            solve_options: self.operation.options.clone(),
            temporary_condition_hash: None,
            started_at: self.operation.started_at,
        }
    }
}

fn checked_elapsed_safely(budget: &ParentSolveBudget) -> Option<Duration> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| budget.checked_elapsed()))
        .ok()
        .flatten()
}

struct SolveOwner {
    input: RunInput,
    budget: ParentSolveBudget,
    started_at: Rfc3339Timestamp,
    clock: Arc<dyn Clock>,
}

impl SolveOwner {
    async fn cleanup(
        &self,
        store: &SqliteScenarioStore,
        alarm: Option<&NativeAlarmCustody>,
        ordinary_lease: Option<&OrdinarySolveLease>,
    ) -> Result<RunManifest, AppError> {
        if let Some(custody) = alarm {
            return self.cleanup_alarm(store, custody, ordinary_lease).await;
        }
        let interruption = OperationControl::Solve(self.budget.phase_view())
            .check()
            .err()
            .map(interruption_status);
        let finished_at = observed_wall(self.clock.as_ref())?;
        let elapsed_observation = checked_elapsed_safely(&self.budget);
        if let Some(lease) = ordinary_lease {
            lease.observe_wall(finished_at);
            if let Some(elapsed) = elapsed_observation {
                lease.observe_elapsed(elapsed);
            }
            if !super::ordinary_terminal_observation_is_trusted(
                lease.observation_bounds()?,
                finished_at,
                elapsed_observation,
            ) {
                return Err(timing_error());
            }
        }
        let elapsed = elapsed_observation.and_then(|elapsed| milliseconds(elapsed).ok());
        let outcome = if elapsed.is_some() {
            no_result(interruption.unwrap_or(SolveStatus::BackendFailed))
        } else {
            // An incomplete run with unusable timing cannot invent a measured terminal duration.
            RunTerminalOutcomeV1::Interrupted
        };
        let manifest = RunManifest::new(
            self.input.run_id,
            self.input.checksum.clone(),
            outcome,
            self.started_at,
            finished_at,
            elapsed,
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )
        .map_err(|_| timing_error())?;
        let manifest = manifest
            .with_native_observations(
                ordinary_lease
                    .and_then(OrdinarySolveLease::native_observations)
                    .map(Arc::unwrap_or_clone),
            )
            .map_err(|_| solution_contract_error())?;
        store
            .finalize_terminal_run(manifest.clone(), None)
            .await
            .map_err(store_error)?;
        Ok(manifest)
    }

    async fn cleanup_alarm(
        &self,
        store: &SqliteScenarioStore,
        custody: &NativeAlarmCustody,
        ordinary_lease: Option<&OrdinarySolveLease>,
    ) -> Result<RunManifest, AppError> {
        if custody.run_id != self.input.run_id
            || custody.input_checksum != self.input.checksum
            || custody.started_at != self.started_at
            || custody.alarm.category == eutheto_verify::CorrectnessAlarmCategory::ClockFailed
        {
            return Err(solution_contract_error());
        }
        // Exactly one paired final observation. Invalid values cause an all-unmeasured manifest;
        // the observed wall timestamp is retained without adjustment.
        let finished_at = observed_wall(self.clock.as_ref())?;
        let elapsed = checked_elapsed_safely(&self.budget);
        if let Some(lease) = ordinary_lease {
            lease.observe_wall(finished_at);
            if let Some(elapsed) = elapsed {
                lease.observe_elapsed(elapsed);
            }
        }
        let measured_elapsed = trusted_alarm_elapsed(custody, finished_at, elapsed);
        let manifest = build_alarm_cleanup_manifest(custody, finished_at, measured_elapsed)?;
        let manifest = manifest
            .with_native_observations(
                ordinary_lease
                    .and_then(OrdinarySolveLease::native_observations)
                    .map(Arc::unwrap_or_clone),
            )
            .map_err(|_| solution_contract_error())?;
        let finalization_error = store
            .finalize_quarantined_run(manifest.clone(), CandidateDiagnosticsV1::default())
            .await
            .err();
        match load_compatible_alarm_readback(store, &self.input, &manifest, custody).await {
            Ok(existing) => existing.manifest.ok_or_else(solution_contract_error),
            Err(readback_error) => Err(finalization_error.map_or(readback_error, store_error)),
        }
    }
}

fn trusted_alarm_elapsed(
    custody: &NativeAlarmCustody,
    finished_at: Rfc3339Timestamp,
    elapsed: Option<Duration>,
) -> Option<DurationMillis> {
    elapsed
        .filter(|elapsed| {
            custody
                .trusted_elapsed_bound
                .is_none_or(|bound| *elapsed >= bound)
        })
        .filter(|_| finished_at >= custody.trusted_wall_bound)
        .and_then(|elapsed| milliseconds(elapsed).ok())
}
fn build_alarm_cleanup_manifest(
    custody: &NativeAlarmCustody,
    finished_at: Rfc3339Timestamp,
    elapsed: Option<DurationMillis>,
) -> Result<RunManifest, AppError> {
    if let Some(elapsed) = elapsed {
        RunManifest::new(
            custody.run_id,
            custody.input_checksum.clone(),
            RunTerminalOutcomeV1::VerificationAlarm {
                diagnostic_code: custody.alarm.diagnostic_code.clone(),
            },
            custody.started_at,
            finished_at,
            Some(elapsed),
            None,
            None,
            RunPhaseTimingsV1::default(),
            Vec::new(),
        )
        .map_err(|_| timing_error())
    } else {
        RunManifest::new_clock_failed_alarm(
            custody.run_id,
            custody.input_checksum.clone(),
            custody.alarm.diagnostic_code.clone(),
            custody.started_at,
            finished_at,
        )
        .map_err(|_| timing_error())
    }
}

async fn load_compatible_alarm_readback(
    store: &SqliteScenarioStore,
    input: &RunInput,
    expected: &RunManifest,
    custody: &NativeAlarmCustody,
) -> Result<ExistingSolveRunV1, AppError> {
    let existing = store
        .load_solve_run_by_request(input.request_id)
        .await
        .map_err(store_error)?;
    let Some(existing) = existing else {
        return Err(store_error(StoreError::SolveRunTerminalConflict(
            custody.run_id,
        )));
    };
    let exact_alarm = matches!(
        &expected.outcome,
        RunTerminalOutcomeV1::VerificationAlarm { diagnostic_code }
            if diagnostic_code == &custody.alarm.diagnostic_code
    );
    if exact_alarm
        && expected.run_id == input.run_id
        && expected.run_input_checksum == input.checksum
        && expected.started_at == existing.started_at
        && existing.input == *input
        && existing.manifest.as_ref() == Some(expected)
    {
        Ok(existing)
    } else {
        Err(store_error(StoreError::SolveRunTerminalConflict(
            custody.run_id,
        )))
    }
}

fn validate_objective_selection(
    selection: Option<&StoredObjectiveSelectionV1>,
) -> Result<(), AppError> {
    let Some(selection) = selection else {
        return Ok(());
    };
    if selection.schema_version != 1 || selection.semantic_version != 1 {
        return Err(objective_selection_error());
    }
    let profile = selection.profile_id.as_str();
    let authored = profile == AUTHORED_OBJECTIVE_PROFILE_ID;
    let workforce = matches!(
        profile,
        "official.workforce.objective.balanced"
            | "official.workforce.objective.preference-focused"
            | "official.workforce.objective.fairness-first"
            | "official.workforce.objective.minimal-changes"
    );
    if (!authored && !workforce)
        || (authored
            && (selection.operation != RunObjectiveOperation::Optimize
                || selection.alternatives_capture_id.is_some()
                || selection.baseline_source != StoredBaselineSourceV1::Accepted))
    {
        return Err(objective_selection_error());
    }
    Ok(())
}

fn objective_selection_requires_baseline(selection: Option<&StoredObjectiveSelectionV1>) -> bool {
    selection.is_some_and(|selection| {
        selection.alternatives_capture_id.is_some()
            || selection.operation == RunObjectiveOperation::Repair
            || selection.profile_id.as_str() == "official.workforce.objective.minimal-changes"
            || matches!(
                selection.baseline_source,
                StoredBaselineSourceV1::External { .. }
            )
    })
}

fn objective_context_for_request(
    request: &StoredSolveRequest,
    capture: &NativeObjectiveCapture,
) -> Result<RunObjectiveContext, AppError> {
    let pack_id = capture.snapshot.document.domain_pack.id.clone();
    let Some(selection) = &request.objective_selection else {
        return RunObjectiveContext::authored(pack_id).map_err(|_| objective_selection_error());
    };
    if selection.profile_id.as_str() == AUTHORED_OBJECTIVE_PROFILE_ID {
        return RunObjectiveContext::authored(pack_id).map_err(|_| objective_selection_error());
    }
    if pack_id.as_str() != WORKFORCE_PACK_ID {
        return Err(objective_selection_error());
    }
    let baseline = objective_selection_requires_baseline(Some(selection))
        .then(|| capture.baseline.clone())
        .flatten()
        .map(Box::new);
    let external_baseline = capture.external_baseline.clone().map(Box::new);
    let context = RunObjectiveContext {
        schema_version: if external_baseline.is_some() {
            3
        } else if baseline
            .as_ref()
            .is_some_and(|binding| binding.captured_original.is_some())
        {
            2
        } else {
            1
        },
        pack_id: pack_id.clone(),
        profile_id: selection.profile_id.clone(),
        semantic_version: selection.semantic_version,
        operation: selection.operation,
        baseline,
        external_baseline,
    };
    context
        .validate_for(&pack_id, request.scenario_id)
        .map_err(|_| objective_selection_error())?;
    Ok(context)
}

fn validate_native_capture(
    request: &StoredSolveRequest,
    capture: &NativeObjectiveCapture,
    requested_original: Option<&PortableAcceptedResult>,
) -> Result<(), AppError> {
    let selection = request.objective_selection.as_ref();
    if capture.scenario_id != request.scenario_id
        || capture.snapshot.document.scenario_id != request.scenario_id
        || capture.snapshot.revision != capture.revision
        || capture.custody_stamp.scenario_id() != request.scenario_id
        || capture.custody_stamp.revision() != capture.revision
        || selection.is_none_or(|selection| selection.baseline_source != capture.baseline_source)
        || request
            .expected_revision
            .is_some_and(|revision| revision != capture.revision)
        || if let Some(original) = requested_original {
            selection.is_none_or(|selection| selection.operation != RunObjectiveOperation::Repair)
                || capture
                    .baseline
                    .as_ref()
                    .is_none_or(|baseline| !same_accepted_original(&baseline.original, original))
        } else {
            selection.and_then(|selection| selection.alternatives_capture_id)
                != Some(capture.snapshot_id)
        }
        || match (&capture.baseline, &capture.external_baseline) {
            (Some(baseline), None) => baseline.original.source_scenario_id != request.scenario_id,
            (None, Some(_)) => !matches!(
                capture.baseline_source,
                StoredBaselineSourceV1::External { .. }
            ),
            _ => true,
        }
    {
        return Err(objective_capture_missing());
    }
    Ok(())
}

fn validate_retry_objective(
    request: &StoredSolveRequest,
    input: &RunInput,
) -> Result<(), AppError> {
    let retained = input.objective_context.as_ref();
    let conflict = || {
        store_error(StoreError::SolveRequestIdConflict {
            request_id: request.request_id,
        })
    };
    let Some(selection) = &request.objective_selection else {
        if retained.is_none_or(|context| {
            RunObjectiveContext::authored(input.pack_id.clone())
                .is_ok_and(|authored| context == &authored)
        }) {
            return Ok(());
        }
        return Err(conflict());
    };
    if selection.profile_id.as_str() == AUTHORED_OBJECTIVE_PROFILE_ID {
        let expected = RunObjectiveContext::authored(input.pack_id.clone())
            .map_err(|_| objective_selection_error())?;
        return if retained == Some(&expected) {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    let Some(context) = retained else {
        return Err(conflict());
    };
    if input.pack_id.as_str() != WORKFORCE_PACK_ID
        || context.profile_id != selection.profile_id
        || context.semantic_version != selection.semantic_version
        || context.operation != selection.operation
        || context
            .validate_for(&input.pack_id, input.scenario_id)
            .is_err()
        || (objective_selection_requires_baseline(Some(selection)) && !context.has_baseline())
        || match selection.baseline_source {
            StoredBaselineSourceV1::Accepted => context.external_baseline.is_some(),
            StoredBaselineSourceV1::External { entity_id } => {
                context.external_baseline.as_ref().is_none_or(|baseline| {
                    !workforce_external_baseline_reference(entity_id)
                        .is_ok_and(|reference| reference == baseline.entity)
                })
            }
        }
        || selection
            .alternatives_capture_id
            .is_some_and(|capture_id| input.snapshot_id != capture_id)
    {
        return Err(conflict());
    }
    Ok(())
}

fn objective_selection_error() -> AppError {
    solve_error(
        "solve.objective_selection_invalid",
        "The recorded objective selection is invalid.",
    )
}

fn objective_baseline_error() -> AppError {
    solve_error(
        "solve.objective_baseline_required",
        "This objective selection requires a fresh accepted Workforce baseline.",
    )
}

fn objective_capture_missing() -> AppError {
    solve_error(
        "solve.objective_capture_missing",
        "The Alternatives capture does not match this stored solve request.",
    )
}

fn validate_retry(
    request: &StoredSolveRequest,
    existing: &ExistingSolveRunV1,
) -> Result<(), AppError> {
    let mut options = request.options.clone();
    if options.backend == BackendSelection::Auto {
        options.backend = BackendSelection::Specific(existing.input.backend_id.clone());
    }
    if existing.input.request_id != request.request_id
        || existing.input.scenario_id != request.scenario_id
        || request
            .expected_revision
            .is_some_and(|revision| existing.input.scenario_revision != revision.value())
        || existing.input.temporary_condition_hash.is_some()
        || existing.input.solve_options != options
    {
        return Err(store_error(StoreError::SolveRequestIdConflict {
            request_id: request.request_id,
        }));
    }
    validate_retry_objective(request, &existing.input)
}

struct OrdinaryReuseCustody {
    request_handles: [Option<OrdinaryPublicationHandle>; 3],
    run_handle: Option<OrdinaryPublicationHandle>,
    requested_original: Option<Arc<PortableAcceptedResult>>,
    original_custody: Option<OriginalRunCustody>,
}

impl OrdinaryReuseCustody {
    fn new() -> Self {
        Self {
            request_handles: [None, None, None],
            run_handle: None,
            requested_original: None,
            original_custody: None,
        }
    }

    fn retain_request_handle(
        &mut self,
        app: &EuthetoApp,
        scenario_id: ScenarioId,
        request_id: RequestId,
    ) -> Result<(), AppError> {
        let Some(handle) = app.ordinary_request_handle(scenario_id, request_id)? else {
            return Ok(());
        };
        let Some(slot) = self.request_handles.iter_mut().find(|slot| slot.is_none()) else {
            return Err(solution_contract_error());
        };
        *slot = Some(handle);
        Ok(())
    }

    fn admit_existing(
        &mut self,
        app: &EuthetoApp,
        scenario_id: ScenarioId,
        input: &RunInput,
    ) -> Result<(), AppError> {
        self.run_handle = app.ordinary_run_handle(scenario_id, input.run_id)?;
        self.check_input(input, None, None)
    }

    fn check_input(
        &self,
        input: &RunInput,
        fresh_request: Option<&OrdinaryPublicationHandle>,
        fresh_run: Option<&OrdinaryPublicationHandle>,
    ) -> Result<(), AppError> {
        for handle in self.request_handles.iter().flatten() {
            handle.validate_input(input)?;
        }
        if let Some(handle) = &self.run_handle {
            handle.validate_input(input)?;
        }
        if let Some(handle) = fresh_request {
            handle.validate_input(input)?;
        }
        if let Some(handle) = fresh_run {
            handle.validate_input(input)?;
        }

        for handle in self.request_handles.iter().flatten() {
            handle.admit()?;
        }
        if let Some(handle) = &self.run_handle {
            handle.admit()?;
        }
        if let Some(handle) = fresh_request {
            handle.admit()?;
        }
        if let Some(handle) = fresh_run {
            handle.admit()?;
        }
        if let Some(original) = &self.original_custody {
            original.handle.admit_input(&original.input)?;
        }
        if let Some(original) = &self.requested_original
            && input
                .objective_context
                .as_ref()
                .and_then(|context| context.baseline.as_ref())
                .is_none_or(|baseline| !same_accepted_original(&baseline.original, original))
        {
            return Err(store_error(StoreError::SolveRequestIdConflict {
                request_id: input.request_id,
            }));
        }
        Ok(())
    }

    fn recheck_after_load(
        &self,
        app: &EuthetoApp,
        scenario_id: ScenarioId,
        input: &RunInput,
    ) -> Result<(), AppError> {
        let fresh_request = app.ordinary_request_handle(scenario_id, input.request_id)?;
        let fresh_run = app.ordinary_run_handle(scenario_id, input.run_id)?;
        self.check_input(input, fresh_request.as_ref(), fresh_run.as_ref())
    }
}

async fn load_reused_output(
    app: &EuthetoApp,
    request: &StoredSolveRequest,
    existing: ExistingSolveRunV1,
    reused: bool,
    report: Option<StoredSolveReport>,
    custody: &mut OrdinaryReuseCustody,
    observer: Option<&StoredSolveBoundObserver>,
) -> Result<StoredSolveOutcome, AppError> {
    validate_retry(request, &existing)?;
    custody.admit_existing(app, request.scenario_id, &existing.input)?;
    if let Some(manifest) = &existing.manifest {
        crate::native_observations::validate_fingerprint(&existing.input, manifest)
            .map_err(|_| solution_contract_error())?;
    }
    if let Some(observer) = observer {
        observer(existing.input.run_id);
    }
    let input = existing.input.clone();
    let outcome = load_committed_output(&app.store, existing, reused, report).await;
    custody.recheck_after_load(app, request.scenario_id, &input)?;
    Ok(outcome)
}

fn known_alarm_from_error(error: &AppError) -> Option<KnownVerificationAlarmV1> {
    match error {
        AppError::Verification(failure) => failure.known_alarm.clone(),
        _ => None,
    }
}

async fn load_committed_output(
    store: &SqliteScenarioStore,
    state: ExistingSolveRunV1,
    reused: bool,
    report: Option<StoredSolveReport>,
) -> StoredSolveOutcome {
    let mut outcome = StoredSolveOutcome {
        state,
        reused,
        report,
        warning: None,
        portable: None,
    };
    if outcome.accepted_result_id().is_none()
        && let Some(report) = &mut outcome.report
    {
        report.execution.selected_candidate = None;
    }
    if let Some(result_id) = outcome.accepted_result_id() {
        match store.load_accepted_result(result_id).await {
            Ok(stored)
                if stored.portable.run_input == outcome.state.input
                    && Some(&stored.portable.run_manifest) == outcome.state.manifest.as_ref() =>
            {
                outcome.portable = Some(stored.portable);
            }
            Ok(_) | Err(_) => outcome.warning = Some(result_reload_warning()),
        }
    }
    outcome
}

fn result_reload_warning() -> AppError {
    solve_error(
        "solve.committed_result_unavailable",
        "The result is committed, but its retained portable output could not be loaded. Retry output by result identity without rerunning the solve.",
    )
}

#[cfg(test)]
mod alarm_cleanup_tests {
    use super::*;
    use eutheto_domain_ir::RunClockFailureV1;
    use eutheto_verify::CorrectnessAlarmCategory;

    fn custody() -> Result<NativeAlarmCustody, Box<dyn std::error::Error>> {
        Ok(NativeAlarmCustody {
            run_id: "018f47f2-e880-7000-8000-000000000010".parse()?,
            input_checksum: "ab".repeat(32),
            started_at: Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?,
            alarm: eutheto_verify::CorrectnessAlarm {
                category: CorrectnessAlarmCategory::ProjectionFailed,
                diagnostic_code: CorrectnessAlarmCategory::ProjectionFailed.code().to_owned(),
            },
            trusted_elapsed_bound: Some(Duration::from_millis(100)),
            trusted_wall_bound: Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?,
            wall_observation_failed: false,
        })
    }

    #[test]
    fn cleanup_alarm_manifest_measures_only_a_valid_final_pair()
    -> Result<(), Box<dyn std::error::Error>> {
        let custody = custody()?;
        let finished_at = Rfc3339Timestamp::parse("2026-01-01T00:00:01Z")?;
        assert_eq!(
            trusted_alarm_elapsed(&custody, finished_at, Some(Duration::from_millis(99))),
            None
        );
        assert_eq!(
            trusted_alarm_elapsed(&custody, finished_at, Some(Duration::from_millis(100))),
            Some(DurationMillis::new(100)?)
        );
        let measured =
            build_alarm_cleanup_manifest(&custody, finished_at, Some(DurationMillis::new(1_000)?))
                .map_err(|_| "measured alarm manifest was rejected")?;
        assert_eq!(measured.finished_at, finished_at);
        assert_eq!(
            measured.elapsed_milliseconds,
            Some(DurationMillis::new(1_000)?)
        );
        assert_eq!(measured.clock_failure, None);
        assert_eq!(measured.first_incumbent_milliseconds, None);
        assert_eq!(measured.first_verified_feasible_milliseconds, None);
        assert_eq!(measured.phase_timings, RunPhaseTimingsV1::default());
        assert!(matches!(
            measured.outcome,
            RunTerminalOutcomeV1::VerificationAlarm { ref diagnostic_code }
                if diagnostic_code == "verification.projection_failed"
        ));

        let regressed_wall = Rfc3339Timestamp::parse("2025-12-31T23:59:59Z")?;
        let bad_monotonic = build_alarm_cleanup_manifest(
            &custody,
            finished_at,
            trusted_alarm_elapsed(&custody, finished_at, Some(Duration::from_millis(99))),
        )
        .map_err(|_| "unmeasured monotonic alarm manifest was rejected")?;
        assert_eq!(bad_monotonic.finished_at, finished_at);
        assert_eq!(bad_monotonic.elapsed_milliseconds, None);
        assert_eq!(
            bad_monotonic.clock_failure,
            Some(RunClockFailureV1::UnreliableObservation {})
        );
        assert_eq!(
            trusted_alarm_elapsed(&custody, regressed_wall, Some(Duration::from_millis(100))),
            None
        );
        let quarantined = build_alarm_cleanup_manifest(&custody, regressed_wall, None)
            .map_err(|_| "unmeasured wall alarm manifest was rejected")?;
        assert_eq!(quarantined.finished_at, regressed_wall);
        assert_eq!(quarantined.elapsed_milliseconds, None);
        assert_eq!(quarantined.first_incumbent_milliseconds, None);
        assert_eq!(quarantined.first_verified_feasible_milliseconds, None);
        assert_eq!(quarantined.phase_timings, RunPhaseTimingsV1::default());
        assert_eq!(
            quarantined.clock_failure,
            Some(RunClockFailureV1::UnreliableObservation {})
        );
        assert!(matches!(
            quarantined.outcome,
            RunTerminalOutcomeV1::VerificationAlarm { ref diagnostic_code }
                if diagnostic_code == "verification.projection_failed"
        ));
        Ok(())
    }

    #[test]
    fn checkpoint_observations_bound_and_recover_alarm_cleanup_measurement()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut checkpoint_custody = custody()?;
        let checkpoint_accepted_at = Rfc3339Timestamp::parse("2026-01-01T00:00:09Z")?;
        checkpoint_custody.retain_wall_observation(checkpoint_accepted_at);
        let checkpoint_event_at = Rfc3339Timestamp::parse("2026-01-01T00:00:10Z")?;
        checkpoint_custody.retain_wall_observation(checkpoint_event_at);
        let regressed_after_checkpoint = Rfc3339Timestamp::parse("2026-01-01T00:00:05Z")?;
        assert_eq!(
            trusted_alarm_elapsed(
                &checkpoint_custody,
                regressed_after_checkpoint,
                Some(Duration::from_millis(200)),
            ),
            None
        );
        let marker =
            build_alarm_cleanup_manifest(&checkpoint_custody, regressed_after_checkpoint, None)
                .map_err(|_| "wall-regressed alarm manifest was rejected")?;
        assert_eq!(marker.finished_at, regressed_after_checkpoint);
        assert_eq!(marker.elapsed_milliseconds, None);
        assert_eq!(
            marker.clock_failure,
            Some(RunClockFailureV1::UnreliableObservation {})
        );
        checkpoint_custody.retain_wall_observation(regressed_after_checkpoint);
        let recovered_after_checkpoint = Rfc3339Timestamp::parse("2026-01-01T00:00:11Z")?;
        let recovered_elapsed = trusted_alarm_elapsed(
            &checkpoint_custody,
            recovered_after_checkpoint,
            Some(Duration::from_millis(200)),
        );
        assert_eq!(recovered_elapsed, Some(DurationMillis::new(200)?));
        let recovered = build_alarm_cleanup_manifest(
            &checkpoint_custody,
            recovered_after_checkpoint,
            recovered_elapsed,
        )
        .map_err(|_| "recovered wall alarm manifest was rejected")?;
        assert_eq!(recovered.finished_at, recovered_after_checkpoint);
        assert_eq!(
            recovered.elapsed_milliseconds,
            Some(DurationMillis::new(200)?)
        );
        assert_eq!(recovered.clock_failure, None);
        Ok(())
    }

    fn ordinary_run_fixture() -> Result<(NewSolveRunV1, RunInput), Box<dyn std::error::Error>> {
        let run_id = "018f47f2-e880-7000-8000-000000000201".parse::<SolveRunId>()?;
        let request_id = "018f47f2-e880-7000-8000-000000000202".parse::<RequestId>()?;
        let scenario_id = "018f47f2-e880-7000-8000-000000000203".parse::<ScenarioId>()?;
        let snapshot_id = "018f47f2-e880-7000-8000-000000000204".parse::<ScenarioSnapshotId>()?;
        let started_at = Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?;
        let pack_id = eutheto_types::PackId::new("official.synthetic")?;
        let backend_id = eutheto_types::BackendId::new("ortools.cp-sat")?;
        let model_hash = eutheto_domain_ir::blake3_hex(b"model");
        let objective_policy_hash = eutheto_domain_ir::blake3_hex(b"objective-policy");
        let solve_options = SolveOptions {
            backend: BackendSelection::Auto,
            mode: eutheto_types::SolveMode::Balanced,
            time_limit_milliseconds: DurationMillis::new(30_000)?,
            memory_limit_bytes: Some(64 * 1024 * 1024),
            worker_threads: eutheto_types::WorkerThreadPolicy::Exact(1),
            random_seed: 7,
            solution_limit: Some(1),
            stop_after_first_feasible: true,
            collect_intermediate_solutions: false,
            explanation_mode: eutheto_types::ExplanationMode::Standard,
            preserve_existing: eutheto_types::PreservationPolicy::None,
            reproducibility: eutheto_types::ReproducibilityMode::Deterministic,
            resource_limits: eutheto_types::ResourceLimits {
                max_entities: 1_000,
                max_rules: 1_000,
                max_variables: 10_000,
                max_constraints: 10_000,
            },
        };
        let input = RunInput::new(
            run_id,
            request_id,
            scenario_id,
            7,
            snapshot_id,
            eutheto_domain_ir::blake3_hex(b"snapshot"),
            started_at,
            pack_id,
            1,
            1,
            "compiler-1".to_owned(),
            "app-1".to_owned(),
            backend_id.clone(),
            "backend-1".to_owned(),
            "adapter-1".to_owned(),
            "worker-1".to_owned(),
            "solver-1".to_owned(),
            1,
            0,
            model_hash.clone(),
            objective_policy_hash.clone(),
            solve_options.clone(),
            "America/Chicago".parse()?,
            None,
            None,
        )?;
        let request = NewSolveRunV1 {
            objective_context: None,
            run_id,
            request_id,
            scenario_id,
            expected_revision: Revision::new(7),
            planning_ir_schema_version: 1,
            compiler_version: "compiler-1".to_owned(),
            application_version: "app-1".to_owned(),
            backend_id,
            backend_version: "backend-1".to_owned(),
            adapter_version: "adapter-1".to_owned(),
            worker_version: "worker-1".to_owned(),
            solver_version: "solver-1".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            model_hash,
            objective_policy_hash,
            solve_options,
            temporary_condition_hash: None,
            started_at,
        };
        Ok((request, input))
    }

    #[tokio::test]
    async fn non_alarm_cleanup_rejects_wall_time_before_the_ordinary_lease_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let started_at = Rfc3339Timestamp::parse("2026-01-01T00:00:00Z")?;
        let accepted_at = Rfc3339Timestamp::parse("2026-01-01T00:00:02Z")?;
        let finished_at = Rfc3339Timestamp::parse("2026-01-01T00:00:01Z")?;
        let (request, input) = ordinary_run_fixture()?;
        let directory = tempfile::tempdir()?;
        let app = EuthetoApp::open(crate::AppDependencies {
            paths: crate::AppPaths {
                database: directory.path().join("cleanup.sqlite"),
                safety_backups: directory.path().join("backups"),
            },
            clock: Arc::new(eutheto_types::FixedClock::new(finished_at)),
            monotonic_clock: Arc::new(eutheto_types::FixedMonotonicClock::default()),
            ids: Arc::new(eutheto_types::SystemIdGenerator),
            cancellation: CancellationToken::new(),
        })
        .await
        .map_err(|error| {
            std::io::Error::other(format!("opening cleanup app fixture: {error:?}"))
        })?;
        let lease = app
            .reserve_ordinary_solve_custody(&request)
            .map_err(|error| {
                std::io::Error::other(format!("reserving cleanup lease: {error:?}"))
            })?;
        lease
            .bind_started(&input, started_at)
            .map_err(|error| std::io::Error::other(format!("binding cleanup lease: {error:?}")))?;
        lease
            .record_first_accepted(
                "018f47f2-e880-7000-8000-000000000205".parse::<SolutionId>()?,
                &eutheto_domain_ir::blake3_hex(b"accepted-result"),
                accepted_at,
            )
            .map_err(|error| {
                std::io::Error::other(format!("recording cleanup acceptance: {error:?}"))
            })?;
        let budget = ParentSolveBudget::new(
            DurationMillis::new(1_000)?,
            Arc::new(eutheto_types::FixedMonotonicClock::default()),
            CancellationToken::new(),
        )?;
        let owner = SolveOwner {
            input,
            budget,
            started_at,
            clock: Arc::new(eutheto_types::FixedClock::new(finished_at)),
        };

        let Err(error) = owner.cleanup(&app.store, None, Some(&lease)).await else {
            return Err(std::io::Error::other(
                "cleanup accepted a regressed native wall observation",
            )
            .into());
        };
        assert!(matches!(
            error,
            AppError::Protocol(failure) if failure.code == "solve.clock_invalid"
        ));
        let bounds = lease
            .observation_bounds()
            .map_err(|error| std::io::Error::other(format!("reading cleanup bounds: {error:?}")))?;
        assert_eq!(bounds.latest_wall, Some(accepted_at));
        assert!(bounds.unreliable);
        Ok(())
    }
}
