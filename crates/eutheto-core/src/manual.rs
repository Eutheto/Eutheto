use super::{
    DomainPackError, EuthetoApp, operation_interrupted, selected_workforce_assignments,
    solution_contract_error, solution_verification_mismatch, store_error, validate,
    validation_error, workforce_base_schedule,
};
use eutheto_domain_ir::{AcceptedResultRefV1, RunBaselineOriginal};
use eutheto_store::{
    AcceptedSourceExpectationV1, CommandWrite, HistoryEntry, JournalWrite,
    NATIVE_PRIVATE_HISTORY_COMMAND_TYPE, RedoBranchPolicy, StoreError,
};
use eutheto_types::{
    ActorRef, AppError, AssignmentId, CancellationToken, Change, ChangeKind, ChangeSet, CommandId,
    CommandResult, CommandSource, EntityId, OperationControl, RequestId, Revision,
    ScenarioDocument, ScenarioId, ValidationDelta, ValidationReport, ValidationSeverity,
};
use eutheto_workforce::{
    assignment_rules::{
        MAX_SELECTED_PAIRS, WorkforceAdmissionBudget, clear_workforce_manual_after_base_change,
        workforce_baseline_reference,
    },
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{AssignmentLock, AssignmentPair, ManualSelection, WorkforceEntity},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[path = "manual_review.rs"]
mod review;
pub(super) use review::controlled_wait;
pub use review::{
    MANUAL_EDIT_SCHEMA_VERSION, MAX_MANUAL_EDIT_REQUEST_BYTES, MAX_MANUAL_EDIT_RESPONSE_BYTES,
    ManualEditAppliedV1, ManualEditChangeV1, ManualEditDispositionV1, ManualEditEvaluationV1,
    ManualEditIntentV1, ManualEditReadV1, ManualEditRequestV1, ManualEditResultV1,
    ManualEditReviewV1, ManualEditRowV1, ManualEditSourceV1, ManualProtectionChangeV1,
    ManualProtectionV1,
};
use review::{ManualAction, ManualInput};

/// Current native complete-manual-selection admission request schema.
pub const MANUAL_SELECTION_ADMISSION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
pub enum ManualLockChangeV1 {
    /// Insert or replace one exact lock record.
    Upsert(AssignmentLock),
    /// Remove one existing lock record.
    Remove(AssignmentId),
}

/// Rust-only native manual admission input. The caller supplies data, never provenance or authority.
#[derive(Clone, Debug)]
pub struct ManualSelectionAdmissionRequestV1 {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub command_id: CommandId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub actor: ActorRef,
    pub original: AcceptedResultRefV1,
    pub assignments: Vec<AssignmentPair>,
    pub lock_changes: Vec<ManualLockChangeV1>,
    pub truncate_redo: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
// Manual writes borrow the source; authenticated decoding owns the same inline receipt format.
#[allow(clippy::large_enum_variant)]
enum NativePrivateHistoryV1<Source = ScenarioDocument> {
    ManualSelection {
        schema_version: u32,
        source_document: Source,
        transition: ProtectedTransition,
    },
    ProtectedEffect {
        schema_version: u32,
        public_command: Value,
        transition: ProtectedTransition,
        before_document_hash: String,
        after_document_hash: String,
        before_document_updated_at: eutheto_types::Rfc3339Timestamp,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedTransition {
    before: ProtectedState,
    after: ProtectedState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedState {
    manual_selection: Option<ProtectedRecord>,
    lock_values: BTreeMap<AssignmentId, Option<Value>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedRecord {
    id: EntityId,
    value: Value,
}

#[derive(Clone, Copy)]
struct ManualReceipt<'a> {
    source_document: &'a ScenarioDocument,
    inverse_source: &'a ScenarioDocument,
    transition: &'a ProtectedTransition,
    inverse_transition: &'a ProtectedTransition,
}

#[allow(clippy::large_enum_variant)]
enum ReplayReceipt<'a> {
    SelectedEntry(&'a HistoryEntry, ManualReceipt<'a>),
    AppliedPrefix(Option<HistoryEntry>),
}

pub(super) struct ProtectedEffectWrite {
    pub command: Value,
    pub inverse: Option<Value>,
    pub change_set: ChangeSet,
}

impl ProtectedTransition {
    fn reversed(&self) -> Self {
        Self {
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }
}

impl EuthetoApp {
    /// Authenticates the original accepted base, evaluates the complete proposed selection, and
    /// commits the private receipt, lock changes and scenario document in one `Store` transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if the request is invalid, authentication fails, evaluation fails, or the
    /// `Store` transaction cannot be committed.
    pub async fn admit_manual_selection(
        &self,
        request: ManualSelectionAdmissionRequestV1,
        control: OperationControl,
    ) -> Result<ManualEditAppliedV1, AppError> {
        let result = self
            .run_manual_blocking(
                ManualInput {
                    schema_version: request.schema_version,
                    request_id: request.request_id,
                    command_id: Some(request.command_id),
                    scenario_id: request.scenario_id,
                    expected_revision: request.expected_revision,
                    actor: Some(request.actor),
                    original: Some(request.original),
                    assignments: request.assignments,
                    lock_changes: request.lock_changes,
                    truncate_redo: request.truncate_redo,
                    action: ManualAction::Native,
                },
                control,
            )
            .await?;
        match result {
            ManualEditResultV1::Applied { applied } => Ok(applied),
            _ => Err(solution_contract_error()),
        }
    }

    // This cohesive admission transaction preserves its ordered validation and custody checks.
    #[allow(clippy::too_many_lines)]
    async fn run_manual(
        &self,
        mut request: ManualInput,
        control: OperationControl,
    ) -> Result<ManualEditResultV1, AppError> {
        if request.schema_version != MANUAL_SELECTION_ADMISSION_SCHEMA_VERSION {
            return Err(super::protocol_error(
                "manual.schema_unsupported",
                "The manual-selection request schema is unsupported.",
                false,
            ));
        }
        if let Some(original) = &request.original {
            original.validate().map_err(|_| solution_contract_error())?;
        }
        if !u64::try_from(request.assignments.len()).is_ok_and(|len| len <= MAX_SELECTED_PAIRS) {
            return Err(validation_error(
                "manual.selection_too_large",
                "/assignments",
                "The complete manual selection exceeds the Workforce pair limit.",
            ));
        }
        if !u64::try_from(request.lock_changes.len()).is_ok_and(|len| len <= MAX_SELECTED_PAIRS) {
            return Err(validation_error(
                "manual.lock_changes_too_large",
                "/lockChanges",
                "Manual admission exceeds the Workforce lock-change limit.",
            ));
        }
        let scenario_id = request.scenario_id;
        let mutation = controlled_wait(self.scenario_lock(scenario_id), &control).await?;
        let _guard = controlled_wait(mutation.lock(), &control).await?;
        let mut budget = WorkforceAdmissionBudget::new(Some(&control));
        control.check().map_err(operation_interrupted)?;
        budget.preflight(&request.actor).map_err(|error| {
            manual_budget_error(
                error,
                "/actor",
                "The manual request exceeds native resource limits.",
            )
        })?;
        budget.preflight(&request.original).map_err(|error| {
            manual_budget_error(
                error,
                "/original",
                "The manual request exceeds native resource limits.",
            )
        })?;
        budget.preflight(&request.assignments).map_err(|error| {
            manual_budget_error(
                error,
                "/assignments",
                "The manual request exceeds native resource limits.",
            )
        })?;
        budget.preflight(&request.lock_changes).map_err(|error| {
            manual_budget_error(
                error,
                "/lockChanges",
                "The manual request exceeds native resource limits.",
            )
        })?;

        let initial = controlled_wait(self.store.get_project(scenario_id), &control)
            .await?
            .map_err(|error| review::controlled_store_error(error, &control))?;
        if initial.summary.revision != request.expected_revision {
            return Err(AppError::Conflict {
                expected_revision: request.expected_revision,
                actual_revision: initial.summary.revision,
            });
        }
        if initial.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
            return Err(super::unsupported_project_pack(
                &initial.document.domain_pack,
            ));
        }
        budget.reserve_json(&initial.document, 1).map_err(|error| {
            manual_budget_error(
                error,
                "/domain",
                "The Workforce document exceeds native resource limits.",
            )
        })?;
        let base = workforce_base_schedule(&initial.document)?
            .ok_or_else(|| validation_error(
                "manual.base_missing",
                "/domain/entities",
                "Adopt an independently accepted Workforce result before admitting a manual selection.",
            ))?;
        let stored = controlled_wait(
            self.load_solution(scenario_id, base.source_solution_id),
            &control,
        )
        .await??;
        let reference = AcceptedResultRefV1::from_result(&stored.portable.accepted_result)
            .map_err(|_| solution_verification_mismatch())?;
        if request
            .original
            .as_ref()
            .is_some_and(|original| original != &reference)
        {
            return Err(solution_verification_mismatch());
        }
        let ordinary = self.ordinary_reference_handle(scenario_id, &reference)?;
        budget.preflight(&stored.portable).map_err(|error| {
            manual_budget_error(
                error,
                "/original",
                "The accepted source exceeds native resource limits.",
            )
        })?;
        budget
            .reserve_json(&stored.portable.run_input, 1)
            .map_err(|error| {
                manual_budget_error(
                    error,
                    "/original/runInput",
                    "The accepted source exceeds native resource limits.",
                )
            })?;
        let original_run_input = stored.portable.run_input.clone();
        if let Some(handle) = &ordinary {
            handle.admit_input(&original_run_input)?;
        }
        let accepted = controlled_wait(
            self.ensure_fresh_accepted_until(
                stored,
                control.clone(),
                tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                None,
            ),
            &control,
        )
        .await??;
        crate::native_observations::validate_approved_worker_origin(
            &accepted.portable.run_input,
            &accepted.portable.run_manifest,
            &self.solver_registry,
        )
        .map_err(|_| solution_verification_mismatch())?;
        if let Some(handle) = &ordinary {
            handle.admit_input(&original_run_input)?;
        }
        let original = authenticated_original(
            scenario_id,
            &base,
            &accepted.portable.accepted_result,
            &accepted.portable.run_input,
            &accepted.portable.run_manifest,
            &reference,
            &mut budget,
        )?;
        let accepted_source = AcceptedSourceExpectationV1::from_stored(&accepted);
        let old_manual = find_manual_selection(&initial.document)?;
        if let Some((manual_id, _, manual)) = &old_manual
            && (manual.id != *manual_id
                || manual.schema_version != 1
                || manual.admission_source_scenario_id != scenario_id
                || budget
                    .manual_selection_checksum(manual)
                    .map_err(manual_integrity_error)?
                    != manual.checksum
                || manual.original != original)
        {
            return Err(solution_verification_mismatch());
        }

        let (source, stamp) = if let Some((manual_id, value, manual)) = &old_manual {
            let lookup = self
                .store
                .lookup_private_history(
                    scenario_id,
                    request.expected_revision,
                    manual.admission_command_id,
                    control.clone(),
                    Some(accepted_source.clone()),
                )
                .await
                .map_err(|error| review::controlled_store_error(error, &control))?;
            let entry = lookup.entry.ok_or_else(solution_verification_mismatch)?;
            authenticate_manual_receipt(
                entry,
                *manual_id,
                value,
                manual,
                scenario_id,
                &mut budget,
            )?;
            if lookup.stamp.document_hash() != document_hash(&initial.document)? {
                return Err(store_error(StoreError::StaleScenarioCustody));
            }
            (initial.document, lookup.stamp)
        } else {
            budget
                .reserve_json(&initial.document, 1)
                .map_err(manual_integrity_error)?;
            let (captured, stamp) = self
                .store
                .capture_scenario_custody(
                    scenario_id,
                    request.expected_revision,
                    initial.document,
                    control.clone(),
                    Some(accepted_source),
                )
                .await
                .map_err(|error| review::controlled_store_error(error, &control))?;
            (captured.document, stamp)
        };
        control.check().map_err(operation_interrupted)?;

        let current_assignments = old_manual
            .as_ref()
            .map_or(&base.assignments, |(_, _, manual)| &manual.assignments);
        budget
            .reserve_json(current_assignments, 1)
            .map_err(manual_integrity_error)?;
        let source_locks = review::locks(&source, &mut budget)?;
        let shift_catalogue = review::shift_catalogue(&source, &mut budget)?;
        let source_document_hash = stamp.document_hash().to_owned();
        let source_view = review::source_view(
            &source,
            request.expected_revision,
            &source_document_hash,
            &original,
            old_manual.as_ref(),
            current_assignments,
            &source_locks,
        )?;
        if let ManualAction::Read { offset, limit } = &request.action {
            budget
                .reserve(
                    0,
                    (current_assignments.len() + source_locks.len()) as u64,
                    ((current_assignments.len() + source_locks.len()) as u64) * 48,
                )
                .map_err(manual_integrity_error)?;
            let pairs: BTreeSet<_> = current_assignments
                .iter()
                .copied()
                .chain(source_locks.keys().copied())
                .collect();
            let total_rows = u32::try_from(pairs.len()).map_err(|_| solution_contract_error())?;
            if *offset > total_rows {
                return Err(validation_error(
                    "manual.page_invalid",
                    "/offset",
                    "The page offset exceeds the complete row count.",
                ));
            }
            let rows = pairs
                .into_iter()
                .skip(*offset as usize)
                .take(*limit as usize)
                .map(|pair| {
                    review::row(
                        &source,
                        &shift_catalogue,
                        pair,
                        current_assignments,
                        &source_locks,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let page_len = u32::try_from(rows.len()).map_err(|_| solution_contract_error())?;
            let end = offset
                .checked_add(page_len)
                .ok_or_else(solution_contract_error)?;
            let result = ManualEditResultV1::Read {
                read: ManualEditReadV1 {
                    schema_version: 1,
                    source: source_view,
                    offset: *offset,
                    total_rows,
                    next_offset: (end < total_rows).then_some(end),
                    rows,
                },
            };
            if let Some(handle) = &ordinary {
                handle.admit_input(&original_run_input)?;
            }
            if let ManualEditResultV1::Read { read } = &result {
                review::wire_bound(read, MAX_MANUAL_EDIT_RESPONSE_BYTES - 128)?;
            }
            control.check().map_err(operation_interrupted)?;
            return Ok(result);
        }
        budget
            .preflight(&initial.portable.semantic_extensions)
            .and_then(|()| budget.preflight(&initial.portable.extensions))
            .map_err(manual_integrity_error)?;
        let mut occupied = BTreeSet::new();
        eutheto_types::visit_document_identities(
            &source,
            initial
                .portable
                .semantic_extensions
                .values()
                .chain(source.extensions.values())
                .chain(initial.portable.extensions.values()),
            &mut |identity| {
                budget.step().map_err(manual_integrity_error)?;
                if !occupied.contains(&identity) {
                    budget.reserve(1, 0, 64).map_err(manual_integrity_error)?;
                    occupied.insert(identity);
                }
                Ok::<(), AppError>(())
            },
        )?;
        let intent = match &request.action {
            ManualAction::Review(intent) | ManualAction::Apply { intent, .. } => Some(intent),
            _ => None,
        };
        let touched = if let Some(intent) = intent {
            budget.preflight(intent).map_err(manual_integrity_error)?;
            request.assignments = current_assignments.clone();
            let (changes, touched) = review::derive_intent(
                &source,
                &shift_catalogue,
                intent,
                &mut request.assignments,
                &source_locks,
                &mut budget,
                &source_view.document_hash,
                &occupied,
            )?;
            request.lock_changes = changes;
            touched
        } else {
            BTreeSet::new()
        };

        let mut assignments = request.assignments;
        budget.sort_work(assignments.len()).map_err(|error| {
            manual_budget_error(
                error,
                "/assignments",
                "The manual selection exceeds native resource limits.",
            )
        })?;
        budget.steps(assignments.len() as u64).map_err(|error| {
            manual_budget_error(
                error,
                "/assignments",
                "The manual selection exceeds native resource limits.",
            )
        })?;
        assignments.sort_unstable();
        if assignments.windows(2).any(|window| window[0] == window[1]) {
            return Err(validation_error(
                "manual.selection_duplicate",
                "/assignments",
                "A manual selection cannot contain duplicate person–shift pairs.",
            ));
        }
        // Review retains one prospective document, not Apply's private journal images.
        budget
            .reserve_json(&source, 1)
            .map_err(manual_integrity_error)?;
        let mut prospective = source.clone();
        let (mut lock_before, mut lock_after) =
            apply_lock_changes(&mut prospective, &request.lock_changes, &mut budget)?;
        let evaluation = budget
            .evaluate_required_selection(&prospective, &assignments)
            .map_err(manual_integrity_error)?;
        let valid = evaluation.obligations.remaining.is_empty()
            && evaluation
                .evaluations
                .iter()
                .all(|outcome| outcome.satisfied);
        if let Some(intent) = intent {
            let prospective_locks = review::locks(&prospective, &mut budget)?;
            let digest = review::approval_digest(
                &source,
                request.expected_revision,
                &original,
                intent,
                &assignments,
                &source_locks,
                &prospective_locks,
                &mut budget,
            )?;
            let changes = touched
                .into_iter()
                .map(|pair| {
                    let before = review::row(
                        &source,
                        &shift_catalogue,
                        pair,
                        current_assignments,
                        &source_locks,
                    )?;
                    let mut after = review::row(
                        &prospective,
                        &shift_catalogue,
                        pair,
                        &assignments,
                        &prospective_locks,
                    )?;
                    if !source_locks.contains_key(&pair) {
                        after.lock_id = None;
                    }
                    Ok(ManualEditChangeV1 { before, after })
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            let evaluations = evaluation
                .evaluations
                .into_iter()
                .map(|evaluation| {
                    let owner_paths =
                        review::owner_paths(&source, &shift_catalogue, &evaluation, &mut budget)?;
                    Ok(ManualEditEvaluationV1 {
                        evaluation,
                        owner_paths,
                    })
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            let reviewed = ManualEditReviewV1 {
                schema_version: 1,
                source: source_view,
                intent: intent.clone(),
                changes,
                evaluations,
                disposition: if valid {
                    ManualEditDispositionV1::Valid
                } else {
                    ManualEditDispositionV1::Invalid
                },
                approval_digest: valid.then_some(digest.clone()),
            };
            review::wire_bound(&reviewed, MAX_MANUAL_EDIT_RESPONSE_BYTES - 128)?;
            if let Some(handle) = &ordinary {
                handle.admit_input(&original_run_input)?;
            }
            control.check().map_err(operation_interrupted)?;
            if matches!(&request.action, ManualAction::Review(_)) {
                return Ok(ManualEditResultV1::Review { review: reviewed });
            }
            if let ManualAction::Apply {
                approval_digest, ..
            } = &request.action
                && approval_digest != &digest
            {
                return Err(validation_error(
                    "manual.review_stale",
                    "/approvalDigest",
                    "The reviewed source or intent changed; review it again.",
                ));
            }
        }
        if !valid {
            return Err(validation_error(
                "manual.required_selection_invalid",
                "/intent",
                "The complete prospective selection does not satisfy every Required obligation; review the native findings.",
            ));
        }
        let mut allocated = BTreeSet::new();
        allocated.extend(
            request
                .lock_changes
                .iter()
                .filter_map(|change| match change {
                    ManualLockChangeV1::Upsert(lock) => Some(lock.id.as_uuid()),
                    ManualLockChangeV1::Remove(_) => None,
                }),
        );
        if intent.is_some() {
            for change in &request.lock_changes {
                let ManualLockChangeV1::Upsert(lock) = change else {
                    continue;
                };
                let pair = AssignmentPair {
                    person_id: lock.person_id,
                    shift_id: lock.shift_id,
                };
                if source_locks.contains_key(&pair) {
                    continue;
                }
                budget.step().map_err(manual_integrity_error)?;
                let id = AssignmentId::from_uuid(super::allocate_unique_uuid(
                    self.ids.as_ref(),
                    &occupied,
                    &mut allocated,
                )?);
                let mut committed_lock = lock.clone();
                committed_lock.id = id;
                let value =
                    serde_json::to_value(committed_lock).map_err(|_| solution_contract_error())?;
                prospective.domain.locked_assignments.remove(&lock.id);
                prospective
                    .domain
                    .locked_assignments
                    .insert(id, value.clone());
                lock_before.remove(&lock.id);
                lock_after.remove(&lock.id);
                lock_before.insert(id, None);
                lock_after.insert(id, Some(value));
            }
        }
        let manual_id = EntityId::from_uuid(super::allocate_unique_uuid(
            self.ids.as_ref(),
            &occupied,
            &mut allocated,
        )?);
        let mut record = ManualSelection {
            schema_version: 1,
            id: manual_id,
            original,
            admission_source_scenario_id: scenario_id,
            admission_source_revision: request.expected_revision,
            admission_source_document_hash: source_document_hash,
            admission_command_id: request.command_id.ok_or_else(solution_contract_error)?,
            assignments,
            checksum: String::new(),
        };
        budget.reserve_json(&record, 1).map_err(|error| {
            manual_budget_error(
                error,
                "/assignments",
                "The manual selection exceeds native resource limits.",
            )
        })?;
        record.checksum = budget
            .manual_selection_checksum(&record)
            .map_err(manual_integrity_error)?;

        let before_manual = old_manual.as_ref().map(|(id, value, _)| ProtectedRecord {
            id: *id,
            value: value.clone(),
        });
        if let Some((id, _, _)) = &old_manual {
            prospective.domain.entities.remove(id);
        }
        budget.preflight(&prospective).map_err(|error| {
            manual_budget_error(
                error,
                "/domain",
                "The proposed Workforce document exceeds native resource limits.",
            )
        })?;
        let selection = WorkforceEntity::ManualSelection(record);
        budget.reserve_json(&selection, 1).map_err(|error| {
            manual_budget_error(
                error,
                "/assignments",
                "The manual selection exceeds native resource limits.",
            )
        })?;
        let selection_value =
            serde_json::to_value(selection).map_err(|_| solution_contract_error())?;
        budget.reserve_json(&selection_value, 1).map_err(|error| {
            manual_budget_error(
                error,
                "/assignments",
                "The manual selection exceeds native resource limits.",
            )
        })?;
        prospective
            .domain
            .entities
            .insert(manual_id, selection_value.clone());
        validate_manual_document(&prospective, &self.pack_registry)?;
        let created_at = self.clock.now();
        prospective.metadata.updated_at = created_at;
        control.check().map_err(operation_interrupted)?;

        let transition = ProtectedTransition {
            before: ProtectedState {
                manual_selection: before_manual,
                lock_values: lock_before,
            },
            after: ProtectedState {
                manual_selection: Some(ProtectedRecord {
                    id: manual_id,
                    value: selection_value,
                }),
                lock_values: lock_after,
            },
        };
        budget.reserve_json(&transition, 4).map_err(|error| {
            manual_budget_error(
                error,
                "/domain",
                "The private history entry exceeds native resource limits.",
            )
        })?;
        budget
            .reserve_json(&source, 2)
            .map_err(manual_integrity_error)?;
        let command = serde_json::to_value(NativePrivateHistoryV1::ManualSelection {
            schema_version: 1,
            source_document: &source,
            transition: transition.clone(),
        })
        .map_err(|_| solution_contract_error())?;
        let inverse = serde_json::to_value(NativePrivateHistoryV1::ManualSelection {
            schema_version: 1,
            source_document: &source,
            transition: transition.reversed(),
        })
        .map_err(|_| solution_contract_error())?;
        let change_set = protected_change_set(&transition);
        let previous_validation = validate(&source, &self.pack_registry);
        let next_validation = validate(&prospective, &self.pack_registry);
        let validation_delta = validation_delta(&previous_validation, &next_validation);
        review::wire_bound(
            &(request.expected_revision, &change_set, &validation_delta),
            MAX_MANUAL_EDIT_RESPONSE_BYTES - 4096,
        )?;
        let actor = request.actor.ok_or_else(solution_contract_error)?;
        let request_id = request.request_id;
        let command_id = request.command_id.ok_or_else(solution_contract_error)?;
        let expected_revision = request.expected_revision;
        let branch_policy = if request.truncate_redo {
            RedoBranchPolicy::Truncate
        } else {
            RedoBranchPolicy::Reject
        };
        if let Some(handle) = &ordinary {
            handle.admit_input(&original_run_input)?;
        }
        let committed = self
            .store
            .execute_command(
                scenario_id,
                expected_revision,
                branch_policy,
                stamp,
                control.clone(),
                move |current| {
                    if current != &source {
                        return Err(StoreError::StaleScenarioCustody);
                    }
                    Ok(CommandWrite {
                        document: prospective,
                        journal: JournalWrite {
                            command_type: NATIVE_PRIVATE_HISTORY_COMMAND_TYPE.to_owned(),
                            command,
                            command_id,
                            inverse: Some(inverse),
                            actor: private_actor(actor),
                            source: CommandSource::Desktop,
                            summary: "Admitted private Workforce selection".to_owned(),
                            created_at,
                        },
                        output: CommandResult {
                            new_revision: expected_revision,
                            change_set,
                            validation_delta,
                            inverse: None,
                        },
                    })
                },
            )
            .await
            .map_err(|error| review::controlled_store_error(error, &control))?;
        let source_admission_failure = ordinary
            .as_ref()
            .and_then(|handle| handle.admit_input(&original_run_input).err());
        let mut output = committed.output;
        output.new_revision = committed.new_revision;
        self.publish_scenario_events(scenario_id, request_id, &output);
        Ok(ManualEditResultV1::Applied {
            applied: ManualEditAppliedV1 {
                schema_version: 1,
                command_result: output,
                source_admission_failure,
            },
        })
    }
}

fn protected_base_transition(
    original: &ScenarioDocument,
    prospective: &mut ScenarioDocument,
    control: Option<&OperationControl>,
) -> Result<Option<ProtectedTransition>, StoreError> {
    if original.domain_pack.id.as_str() != WORKFORCE_PACK_ID {
        return Ok(None);
    }
    let removed = clear_workforce_manual_after_base_change(original, prospective, control)
        .map_err(|_error| {
            if control.is_some_and(|control| control.check().is_err()) {
                return StoreError::OperationCancelled;
            }
            StoreError::CommandApplication {
                code: "workforce.manual_finalization_failed".to_owned(),
                message: "The Workforce manual selection could not be safely finalized.".to_owned(),
                field_path: None,
            }
        })?;
    let Some((id, value)) = removed else {
        return Ok(None);
    };
    Ok(Some(ProtectedTransition {
        before: ProtectedState {
            manual_selection: Some(ProtectedRecord { id, value }),
            lock_values: BTreeMap::new(),
        },
        after: ProtectedState {
            manual_selection: None,
            lock_values: BTreeMap::new(),
        },
    }))
}

pub(super) fn finalize_base_change(
    original: &ScenarioDocument,
    prospective: &mut ScenarioDocument,
    public_command: &Value,
    public_inverse: Option<&Value>,
    control: Option<&OperationControl>,
) -> Result<Option<ProtectedEffectWrite>, StoreError> {
    let Some(transition) = protected_base_transition(original, prospective, control)? else {
        return Ok(None);
    };
    let before_document_updated_at = original.metadata.updated_at;
    let before_document_hash = document_hash(original).map_err(|_| private_history_integrity())?;
    let after_document_hash = protected_document_hash(prospective, before_document_updated_at)?;
    let command = serde_json::to_value(
        NativePrivateHistoryV1::<ScenarioDocument>::ProtectedEffect {
            schema_version: 1,
            public_command: public_command.clone(),
            transition: transition.clone(),
            before_document_hash: before_document_hash.clone(),
            after_document_hash: after_document_hash.clone(),
            before_document_updated_at,
        },
    )?;
    let inverse = public_inverse
        .cloned()
        .map(|public_command| {
            serde_json::to_value(
                NativePrivateHistoryV1::<ScenarioDocument>::ProtectedEffect {
                    schema_version: 1,
                    public_command,
                    transition: transition.reversed(),
                    before_document_hash,
                    after_document_hash,
                    before_document_updated_at,
                },
            )
        })
        .transpose()?;
    Ok(Some(ProtectedEffectWrite {
        command,
        inverse,
        change_set: protected_change_set(&transition),
    }))
}

fn protected_document_hash(
    document: &mut ScenarioDocument,
    source_updated_at: eutheto_types::Rfc3339Timestamp,
) -> Result<String, StoreError> {
    let updated_at = std::mem::replace(&mut document.metadata.updated_at, source_updated_at);
    let hash = document_hash(document).map_err(|_| private_history_integrity());
    document.metadata.updated_at = updated_at;
    hash
}

pub(super) fn finalize_standalone_base_change(
    original: &ScenarioDocument,
    prospective: &mut ScenarioDocument,
    control: Option<&OperationControl>,
) -> Result<Option<ChangeSet>, StoreError> {
    Ok(protected_base_transition(original, prospective, control)?
        .as_ref()
        .map(protected_change_set))
}

pub(super) fn related_replay_receipt_id(
    entry: &HistoryEntry,
    scenario_id: ScenarioId,
) -> Result<Option<CommandId>, StoreError> {
    if entry.command_type != NATIVE_PRIVATE_HISTORY_COMMAND_TYPE {
        return Ok(None);
    }
    let command_kind = private_history_kind(&entry.command)?;
    match command_kind {
        "manualSelection" => {
            require_private_history_schema(&entry.command, command_kind)?;
            let inverse = entry
                .inverse
                .as_ref()
                .ok_or_else(private_history_integrity)?;
            require_private_history_schema(inverse, command_kind)?;
            let selected = if entry.applied {
                inverse
            } else {
                &entry.command
            };
            let record = selected
                .pointer("/transition/after/manualSelection")
                .ok_or_else(private_history_integrity)?;
            private_record_receipt_id(record, scenario_id, entry.id, !entry.applied)
        }
        "protectedEffect" => {
            require_private_history_schema(&entry.command, command_kind)?;
            let record = entry
                .command
                .pointer("/transition/before/manualSelection")
                .ok_or_else(private_history_integrity)?;
            match private_record_receipt_id(record, scenario_id, entry.id, false)? {
                Some(command_id) => Ok(Some(command_id)),
                None if record.is_null() => Err(private_history_integrity()),
                None => Ok(None),
            }
        }
        _ => Err(private_history_integrity()),
    }
}

fn private_history_kind(value: &Value) -> Result<&str, StoreError> {
    value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(private_history_integrity)
}

fn require_private_history_schema(value: &Value, expected_kind: &str) -> Result<(), StoreError> {
    if private_history_kind(value)? != expected_kind
        || value.get("schema_version").and_then(Value::as_u64) != Some(1)
    {
        return Err(private_history_integrity());
    }
    Ok(())
}

fn private_record_receipt_id(
    record: &Value,
    scenario_id: ScenarioId,
    history_command_id: CommandId,
    allow_current: bool,
) -> Result<Option<CommandId>, StoreError> {
    if record.is_null() {
        return Ok(None);
    }
    let source_scenario = record
        .pointer("/value/admissionSourceScenarioId")
        .ok_or_else(private_history_integrity)?;
    let original_scenario = record
        .pointer("/value/original/sourceScenarioId")
        .ok_or_else(private_history_integrity)?;
    let admission_scenario: ScenarioId =
        serde_json::from_value(source_scenario.clone()).map_err(StoreError::Json)?;
    let original_scenario: ScenarioId =
        serde_json::from_value(original_scenario.clone()).map_err(StoreError::Json)?;
    if admission_scenario != original_scenario {
        return Err(private_history_integrity());
    }
    let id = record
        .pointer("/value/admissionCommandId")
        .ok_or_else(private_history_integrity)?;
    let command_id: CommandId = serde_json::from_value(id.clone()).map_err(StoreError::Json)?;
    if admission_scenario == scenario_id && command_id == history_command_id {
        return if allow_current {
            Ok(None)
        } else {
            Err(private_history_integrity())
        };
    }
    Ok(Some(command_id))
}

fn private_history_integrity() -> StoreError {
    StoreError::Integrity("private history entry failed native authentication".to_owned())
}

fn private_transition_lock_maps_bounded(value: &Value) -> bool {
    [
        "/transition/before/lockValues",
        "/transition/after/lockValues",
    ]
    .into_iter()
    .all(|path| {
        value
            .pointer(path)
            .and_then(Value::as_object)
            .is_some_and(|values| {
                u64::try_from(values.len()).is_ok_and(|len| len <= MAX_SELECTED_PAIRS)
            })
    })
}

// The owned error is required by existing `map_err` callbacks at the conversion boundary.
#[allow(clippy::needless_pass_by_value)]
fn replay_budget_error(error: DomainPackError) -> StoreError {
    match error {
        DomainPackError::Cancelled | DomainPackError::BudgetExpired => {
            StoreError::OperationCancelled
        }
        DomainPackError::ResourceLimitExceeded => StoreError::CommandApplication {
            code: "command.resource_limit".to_owned(),
            message: "Native private history exceeded resource limits.".to_owned(),
            field_path: None,
        },
        _ => private_history_integrity(),
    }
}

fn authenticate_replay_record(
    record: &ProtectedRecord,
    receipt: ReplayReceipt<'_>,
    scenario_id: ScenarioId,
    source_capture: Option<&ScenarioDocument>,
    budget: &mut WorkforceAdmissionBudget<'_>,
    control: &OperationControl,
) -> Result<(), StoreError> {
    let mut owned_receipt = None;
    let (receipt, decoded) = match receipt {
        ReplayReceipt::SelectedEntry(entry, decoded) => (Some(entry), Some(decoded)),
        ReplayReceipt::AppliedPrefix(entry) => {
            owned_receipt = entry;
            (owned_receipt.as_ref(), None)
        }
    };
    let selected_entry = decoded.is_some();
    budget
        .reserve_json(&record.value, 1)
        .map_err(replay_budget_error)?;
    let entity: WorkforceEntity =
        serde_json::from_value(record.value.clone()).map_err(|_| private_history_integrity())?;
    let WorkforceEntity::ManualSelection(manual) = entity else {
        return Err(private_history_integrity());
    };
    if manual.id != record.id
        || manual.schema_version != 1
        || budget
            .manual_selection_checksum(&manual)
            .map_err(replay_budget_error)?
            != manual.checksum
    {
        return Err(private_history_integrity());
    }
    if manual.admission_source_scenario_id == scenario_id
        && manual.original.source_scenario_id == scenario_id
    {
        let receipt = receipt.ok_or_else(private_history_integrity)?;
        if let Some(captured) = source_capture {
            authenticate_captured_original_base(&manual, scenario_id, captured, budget)?;
        }
        if !selected_entry && !receipt.applied {
            return Err(private_history_integrity());
        }
        let result = if let Some(decoded) = decoded {
            validate_manual_receipt_header(receipt, &manual).and_then(|()| {
                authenticate_decoded_manual_receipt(
                    record.id,
                    &record.value,
                    &manual,
                    scenario_id,
                    decoded,
                    budget,
                )
            })
        } else {
            authenticate_owned_manual_receipt(
                owned_receipt.ok_or_else(private_history_integrity)?,
                record.id,
                &record.value,
                &manual,
                scenario_id,
                budget,
            )
            .map(drop)
        };
        return result.map_err(|_| {
            if control.check().is_err() {
                StoreError::OperationCancelled
            } else {
                private_history_integrity()
            }
        });
    }
    if manual.admission_source_scenario_id == scenario_id
        || manual.original.source_scenario_id == scenario_id
        || manual.admission_source_scenario_id != manual.original.source_scenario_id
        || receipt.is_some()
    {
        return Err(private_history_integrity());
    }
    let source_capture = source_capture.ok_or_else(private_history_integrity)?;
    authenticate_captured_original_base(&manual, scenario_id, source_capture, budget)
}

fn authenticate_captured_original_base(
    manual: &ManualSelection,
    scenario_id: ScenarioId,
    source_capture: &ScenarioDocument,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<(), StoreError> {
    if source_capture.scenario_id != scenario_id {
        return Err(private_history_integrity());
    }
    let base = workforce_base_schedule(source_capture)
        .map_err(|_| private_history_integrity())?
        .ok_or_else(private_history_integrity)?;
    let reference =
        workforce_baseline_reference(base.id, false).map_err(|_| private_history_integrity())?;
    if manual.original.entity != reference
        || manual.original.accepted_result.solution_id != base.source_solution_id
        || manual.original.source_revision != base.source_revision.value()
    {
        return Err(private_history_integrity());
    }
    // Foreign ancestry hashes describe pre-remap pairs; private document commitments
    // bind restoration of the copied local bytes without minting worker authority.
    if manual.original.source_scenario_id == scenario_id
        && manual.original.pair_content_hash
            != budget
                .pair_content_hash(&base.assignments)
                .map_err(replay_budget_error)?
    {
        return Err(private_history_integrity());
    }
    Ok(())
}
// This cohesive replay path authenticates journal lineage before applying either transition.
#[allow(clippy::too_many_lines)]
pub(super) fn apply_private_history(
    mut history: eutheto_store::HistoryCommand,
    current_revision: Revision,
    source: CommandSource,
    registry: &super::DomainPackRegistry,
    cancellation: &CancellationToken,
) -> Result<(ScenarioDocument, CommandResult), StoreError> {
    let control = OperationControl::Cancellation(cancellation.clone());
    control
        .check()
        .map_err(|_| StoreError::OperationCancelled)?;
    let selected_entry_command = if history.entry.applied {
        history
            .entry
            .inverse
            .as_ref()
            .ok_or_else(private_history_integrity)?
    } else {
        &history.entry.command
    };
    if history.entry.command_type != NATIVE_PRIVATE_HISTORY_COMMAND_TYPE
        || &history.command != selected_entry_command
        || history.entry.actor.actor_id.is_some()
        || history.entry.actor.display_name != "Private history entry"
    {
        return Err(private_history_integrity());
    }
    let inverse_value = history
        .entry
        .inverse
        .as_ref()
        .ok_or_else(private_history_integrity)?;
    if !private_transition_lock_maps_bounded(&history.entry.command)
        || !private_transition_lock_maps_bounded(inverse_value)
    {
        return Err(private_history_integrity());
    }
    let mut budget = WorkforceAdmissionBudget::new(Some(&control));
    budget
        .reserve_json(&history.document, 1)
        .and_then(|()| budget.reserve_json(&history.entry.command, 1))
        .and_then(|()| budget.reserve_json(inverse_value, 1))
        .and_then(|()| budget.reserve_json(&history.command, 1))
        .map_err(replay_budget_error)?;
    // Move charged bodies into the existing owned decoder; do not retain copied source trees.
    history.command = Value::Null;
    let forward: NativePrivateHistoryV1 =
        serde_json::from_value(history.entry.command.take()).map_err(StoreError::Json)?;
    let inverse: NativePrivateHistoryV1 = serde_json::from_value(
        history
            .entry
            .inverse
            .take()
            .ok_or_else(private_history_integrity)?,
    )
    .map_err(StoreError::Json)?;
    let scenario_id = history.document.scenario_id;
    match (forward, inverse) {
        (
            NativePrivateHistoryV1::ManualSelection {
                schema_version: 1,
                source_document: forward_source,
                transition: forward_transition,
            },
            NativePrivateHistoryV1::ManualSelection {
                schema_version: 1,
                source_document: inverse_source,
                transition: inverse_transition,
            },
        ) => {
            if history.entry.source != CommandSource::Desktop
                || history.entry.summary != "Admitted private Workforce selection"
            {
                return Err(private_history_integrity());
            }
            if forward_source != inverse_source
                || inverse_transition.before != forward_transition.after
                || inverse_transition.after != forward_transition.before
            {
                return Err(private_history_integrity());
            }
            let source_document = forward_source;
            let source_steps =
                u64::try_from(source_document.domain.entities.len()).map_err(|_| {
                    StoreError::CommandApplication {
                        code: "command.resource_limit".to_owned(),
                        message: "Native private history exceeded resource limits.".to_owned(),
                        field_path: None,
                    }
                })?;
            budget.steps(source_steps).map_err(replay_budget_error)?;
            let new_record = forward_transition
                .after
                .manual_selection
                .as_ref()
                .ok_or_else(private_history_integrity)?;
            authenticate_replay_record(
                new_record,
                ReplayReceipt::SelectedEntry(
                    &history.entry,
                    ManualReceipt {
                        source_document: &source_document,
                        inverse_source: &inverse_source,
                        transition: &forward_transition,
                        inverse_transition: &inverse_transition,
                    },
                ),
                scenario_id,
                Some(&source_document),
                &mut budget,
                &control,
            )?;
            if history.entry.applied {
                if let Some(restored) = &forward_transition.before.manual_selection {
                    authenticate_replay_record(
                        restored,
                        ReplayReceipt::AppliedPrefix(history.related_private_receipt.take()),
                        scenario_id,
                        Some(&source_document),
                        &mut budget,
                        &control,
                    )?;
                } else if history.related_private_receipt.is_some() {
                    return Err(private_history_integrity());
                }
            } else if history.related_private_receipt.is_some() {
                return Err(private_history_integrity());
            }
            let source_updated_at = source_document.metadata.updated_at;
            let mut document = if history.entry.applied {
                budget
                    .reserve_json(&source_document, 1)
                    .map_err(replay_budget_error)?;
                let mut expected = source_document.clone();
                apply_protected_transition(&mut expected, &forward_transition, &mut budget)?;
                expected.metadata.updated_at = history.entry.created_at;
                if expected != history.document {
                    return Err(StoreError::StaleScenarioCustody);
                }
                expected
            } else {
                if source_document != history.document {
                    return Err(StoreError::StaleScenarioCustody);
                }
                source_document
            };
            let transition = if history.entry.applied {
                &inverse_transition
            } else {
                &forward_transition
            };
            apply_protected_transition(&mut document, transition, &mut budget)?;
            document.metadata.updated_at = if history.entry.applied {
                source_updated_at
            } else {
                history.target_document_updated_at
            };
            validate_document_for_replay(&document, registry)?;
            let result = result_for_private_replay(
                &history.document,
                &document,
                current_revision,
                protected_change_set(transition),
                registry,
            );
            Ok((document, result))
        }
        (
            NativePrivateHistoryV1::ProtectedEffect {
                schema_version: 1,
                public_command: forward_public_command,
                transition: forward_transition,
                before_document_hash,
                after_document_hash,
                before_document_updated_at,
            },
            NativePrivateHistoryV1::ProtectedEffect {
                schema_version: 1,
                public_command: inverse_public_command,
                transition: inverse_transition,
                before_document_hash: inverse_before_document_hash,
                after_document_hash: inverse_after_document_hash,
                before_document_updated_at: inverse_before_document_updated_at,
            },
        ) => {
            if inverse_transition.before != forward_transition.after
                || inverse_transition.after != forward_transition.before
                || before_document_hash != inverse_before_document_hash
                || after_document_hash != inverse_after_document_hash
                || before_document_updated_at != inverse_before_document_updated_at
                || forward_transition.before.manual_selection.is_none()
                || forward_transition.after.manual_selection.is_some()
                || !forward_transition.before.lock_values.is_empty()
                || !forward_transition.after.lock_values.is_empty()
                || history.entry.summary != "Updated private Workforce selection"
                || history.document.domain_pack.id.as_str() != WORKFORCE_PACK_ID
            {
                return Err(private_history_integrity());
            }
            let source_record = forward_transition
                .before
                .manual_selection
                .as_ref()
                .ok_or_else(private_history_integrity)?;
            let (public_command, transition) = if history.entry.applied {
                (&inverse_public_command, &inverse_transition)
            } else {
                (&forward_public_command, &forward_transition)
            };
            budget
                .preflight(&history.document)
                .map_err(replay_budget_error)?;
            let entry_document_hash = if history.entry.applied {
                protected_document_hash(&mut history.document, before_document_updated_at)?
            } else {
                document_hash(&history.document).map_err(|_| private_history_integrity())?
            };
            let expected_entry_hash = if history.entry.applied {
                &after_document_hash
            } else {
                &before_document_hash
            };
            if &entry_document_hash != expected_entry_hash {
                return Err(private_history_integrity());
            }
            budget
                .reserve_json(public_command, 1)
                .map_err(replay_budget_error)?;
            let command =
                serde_json::from_value(public_command.clone()).map_err(StoreError::Json)?;
            let mut envelope = super::CommandEnvelope {
                command_id: history.entry.id,
                scenario_id,
                expected_revision: current_revision,
                actor: private_actor(ActorRef {
                    actor_id: None,
                    display_name: "History".to_owned(),
                }),
                source,
                command,
            };
            super::ensure_supported_document(&history.document, registry)?;
            budget
                .reserve_json(&history.document, 1)
                .map_err(replay_budget_error)?;
            let mut applied = super::apply_command_with_registry(
                &history.document,
                current_revision,
                &envelope,
                registry,
                cancellation,
            )
            .map_err(|error| super::command_store_error(&error, Some(&envelope.command)))?;
            if !budget
                .effective_base_changed(&history.document, &applied.document)
                .map_err(replay_budget_error)?
            {
                return Err(private_history_integrity());
            }
            let opposite = if history.entry.applied {
                &forward_public_command
            } else {
                &inverse_public_command
            };
            budget
                .reserve_json(opposite, 1)
                .map_err(replay_budget_error)?;
            budget
                .reserve_json(&applied.document, 1)
                .map_err(replay_budget_error)?;
            envelope.command =
                serde_json::from_value(opposite.clone()).map_err(StoreError::Json)?;
            let reconstructed = super::apply_command_with_registry(
                &applied.document,
                current_revision,
                &envelope,
                registry,
                cancellation,
            )
            .map_err(|error| super::command_store_error(&error, Some(&envelope.command)))?;
            budget
                .preflight(&reconstructed.document)
                .map_err(replay_budget_error)?;
            if reconstructed.document != history.document {
                return Err(private_history_integrity());
            }
            applied.document.metadata.updated_at = if history.entry.applied {
                before_document_updated_at
            } else {
                history.entry.created_at
            };
            let source_capture = if history.entry.applied {
                &applied.document
            } else {
                &history.document
            };
            authenticate_replay_record(
                source_record,
                ReplayReceipt::AppliedPrefix(history.related_private_receipt.take()),
                scenario_id,
                Some(source_capture),
                &mut budget,
                &control,
            )?;
            apply_protected_transition(&mut applied.document, transition, &mut budget)?;
            budget
                .preflight(&applied.document)
                .map_err(replay_budget_error)?;
            let target_document_hash = if history.entry.applied {
                document_hash(&applied.document).map_err(|_| private_history_integrity())?
            } else {
                protected_document_hash(&mut applied.document, before_document_updated_at)?
            };
            let expected_target_hash = if history.entry.applied {
                &before_document_hash
            } else {
                &after_document_hash
            };
            if &target_document_hash != expected_target_hash {
                return Err(private_history_integrity());
            }
            validate_document_for_replay(&applied.document, registry)?;
            let mut result = applied.result;
            result
                .change_set
                .changes
                .extend(protected_change_set(transition).changes);
            result.validation_delta = validation_delta(
                &validate(&history.document, registry),
                &validate(&applied.document, registry),
            );
            result.inverse = None;
            Ok((applied.document, result))
        }
        _ => Err(private_history_integrity()),
    }
}

pub(super) fn authenticated_original(
    scenario_id: ScenarioId,
    base: &eutheto_workforce::model::BaseSchedule,
    accepted: &eutheto_domain_ir::AcceptedResult,
    input: &eutheto_domain_ir::RunInput,
    manifest: &eutheto_domain_ir::RunManifest,
    requested: &AcceptedResultRefV1,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<RunBaselineOriginal, AppError> {
    let reference =
        AcceptedResultRefV1::from_result(accepted).map_err(|_| solution_verification_mismatch())?;
    if &reference != requested
        || input.scenario_id != scenario_id
        || accepted.solution.scenario_id != scenario_id
        || base.source_solution_id != accepted.solution.solution_id
        || base.source_revision.value() != input.scenario_revision
        || manifest.run_id != input.run_id
        || manifest.run_input_checksum != input.checksum
    {
        return Err(solution_verification_mismatch());
    }
    budget
        .reserve_json(base, 1)
        .and_then(|()| budget.reserve_json(accepted, 1))
        .and_then(|()| budget.reserve_json(input, 1))
        .and_then(|()| budget.reserve_json(manifest, 1))
        .map_err(manual_integrity_error)?;
    let selected = selected_workforce_assignments(accepted)?;
    let base_hash = budget
        .pair_content_hash(&base.assignments)
        .map_err(manual_integrity_error)?;
    let accepted_hash = budget
        .pair_content_hash(&selected)
        .map_err(manual_integrity_error)?;
    if base_hash != accepted_hash {
        return Err(solution_verification_mismatch());
    }
    let original = RunBaselineOriginal {
        entity: workforce_baseline_reference(base.id, false)
            .map_err(|_| solution_contract_error())?,
        pair_content_hash: base_hash,
        accepted_result: reference,
        source_scenario_id: input.scenario_id,
        source_revision: input.scenario_revision,
        source_document_hash: input.snapshot_document_hash.clone(),
        source_run_id: input.run_id,
        source_run_input_checksum: input.checksum.clone(),
        source_run_manifest_checksum: manifest.checksum.clone(),
    };
    budget
        .reserve_json(&original, 1)
        .map_err(manual_integrity_error)?;
    Ok(original)
}

pub(super) fn find_manual_selection(
    document: &ScenarioDocument,
) -> Result<Option<(EntityId, Value, ManualSelection)>, AppError> {
    let mut found = None;
    for (id, value) in &document.domain.entities {
        if value.get("kind").and_then(Value::as_str) != Some("manualSelection") {
            continue;
        }
        let entity: WorkforceEntity =
            serde_json::from_value(value.clone()).map_err(|_| solution_verification_mismatch())?;
        let WorkforceEntity::ManualSelection(record) = entity else {
            return Err(solution_verification_mismatch());
        };
        if record.id != *id || found.replace((*id, value.clone(), record)).is_some() {
            return Err(solution_verification_mismatch());
        }
    }
    Ok(found)
}

pub(super) fn authenticate_manual_receipt(
    entry: HistoryEntry,
    manual_id: EntityId,
    manual_value: &Value,
    manual: &ManualSelection,
    scenario_id: ScenarioId,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<(), AppError> {
    if !entry.applied {
        return Err(solution_verification_mismatch());
    }
    authenticate_owned_manual_receipt(entry, manual_id, manual_value, manual, scenario_id, budget)
        .map(drop)
}

fn validate_manual_receipt_header(
    entry: &HistoryEntry,
    manual: &ManualSelection,
) -> Result<(), AppError> {
    if entry.command_type != NATIVE_PRIVATE_HISTORY_COMMAND_TYPE
        || entry.id != manual.admission_command_id
        || entry.actor.actor_id.is_some()
        || entry.actor.display_name != "Private history entry"
        || entry.source != CommandSource::Desktop
        || entry.summary != "Admitted private Workforce selection"
        || entry.revision_before != manual.admission_source_revision
    {
        return Err(solution_verification_mismatch());
    }
    Ok(())
}

fn validate_manual_receipt_entry<'a>(
    entry: &'a HistoryEntry,
    manual: &ManualSelection,
) -> Result<&'a Value, AppError> {
    validate_manual_receipt_header(entry, manual)?;
    let inverse = entry
        .inverse
        .as_ref()
        .ok_or_else(solution_verification_mismatch)?;
    if !private_transition_lock_maps_bounded(&entry.command)
        || !private_transition_lock_maps_bounded(inverse)
    {
        return Err(solution_verification_mismatch());
    }
    Ok(inverse)
}

pub(super) fn authenticate_owned_manual_receipt(
    entry: HistoryEntry,
    manual_id: EntityId,
    manual_value: &Value,
    manual: &ManualSelection,
    scenario_id: ScenarioId,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<ScenarioDocument, AppError> {
    let inverse = validate_manual_receipt_entry(&entry, manual)?;
    budget
        .reserve_json(&entry.command, 1)
        .and_then(|()| budget.reserve_json(inverse, 1))
        .map_err(manual_integrity_error)?;
    let forward: NativePrivateHistoryV1 =
        serde_json::from_value(entry.command).map_err(|_| solution_verification_mismatch())?;
    let inverse: NativePrivateHistoryV1 =
        serde_json::from_value(entry.inverse.ok_or_else(solution_verification_mismatch)?)
            .map_err(|_| solution_verification_mismatch())?;
    let (
        NativePrivateHistoryV1::ManualSelection {
            schema_version: 1,
            source_document,
            transition,
        },
        NativePrivateHistoryV1::ManualSelection {
            schema_version: 1,
            source_document: inverse_source,
            transition: inverse_transition,
        },
    ) = (forward, inverse)
    else {
        return Err(solution_verification_mismatch());
    };
    authenticate_decoded_manual_receipt(
        manual_id,
        manual_value,
        manual,
        scenario_id,
        ManualReceipt {
            source_document: &source_document,
            inverse_source: &inverse_source,
            transition: &transition,
            inverse_transition: &inverse_transition,
        },
        budget,
    )?;
    Ok(source_document)
}

fn authenticate_decoded_manual_receipt(
    manual_id: EntityId,
    manual_value: &Value,
    manual: &ManualSelection,
    scenario_id: ScenarioId,
    receipt: ManualReceipt<'_>,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<(), AppError> {
    let ManualReceipt {
        source_document,
        inverse_source,
        transition,
        inverse_transition,
    } = receipt;
    budget
        .preflight(manual_value)
        .map_err(manual_integrity_error)?;
    budget
        .preflight(&source_document)
        .and_then(|()| budget.reserve(1, 0, 64))
        .map_err(manual_integrity_error)?;
    let steps = u64::try_from(source_document.domain.entities.len())
        .ok()
        .and_then(|count| {
            count.checked_add(u64::try_from(transition.before.lock_values.len()).ok()?)
        })
        .and_then(|count| {
            count.checked_add(u64::try_from(transition.after.lock_values.len()).ok()?)
        })
        .ok_or_else(solution_verification_mismatch)?;
    budget.steps(steps).map_err(manual_integrity_error)?;
    let source_manual =
        find_manual_value(source_document).map_err(|_| solution_verification_mismatch())?;
    let locks_match = transition.before.lock_values.iter().all(|(id, value)| {
        source_document
            .domain
            .locked_assignments
            .get(id)
            .is_some_and(|actual| value.as_ref() == Some(actual))
            || (value.is_none() && !source_document.domain.locked_assignments.contains_key(id))
    });
    if source_document.scenario_id != scenario_id
        || document_hash(source_document)? != manual.admission_source_document_hash
        || transition
            .after
            .manual_selection
            .as_ref()
            .is_none_or(|record| record.id != manual_id || &record.value != manual_value)
        || transition.before != inverse_transition.after
        || transition.after != inverse_transition.before
        || source_document != inverse_source
        || source_manual
            != transition
                .before
                .manual_selection
                .as_ref()
                .map(|record| (&record.id, &record.value))
        || !locks_match
        || transition
            .before
            .lock_values
            .keys()
            .ne(transition.after.lock_values.keys())
    {
        return Err(solution_verification_mismatch());
    }
    Ok(())
}

// The paired maps are the before/after values for the same lock transition.
#[allow(clippy::type_complexity)]
fn apply_lock_changes(
    document: &mut ScenarioDocument,
    changes: &[ManualLockChangeV1],
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<
    (
        BTreeMap<AssignmentId, Option<Value>>,
        BTreeMap<AssignmentId, Option<Value>>,
    ),
    AppError,
> {
    budget.reserve_json(changes, 2).map_err(|error| {
        manual_budget_error(
            error,
            "/lockChanges",
            "The lock changes exceed native resource limits.",
        )
    })?;
    let mut changed_ids = BTreeSet::new();
    let mut final_count = u64::try_from(document.domain.locked_assignments.len())
        .map_err(|_| solution_contract_error())?;
    for change in changes {
        budget.step().map_err(|error| {
            manual_budget_error(
                error,
                "/lockChanges",
                "The lock changes exceed native resource limits.",
            )
        })?;
        let (id, upsert) = match change {
            ManualLockChangeV1::Upsert(lock) => (lock.id, true),
            ManualLockChangeV1::Remove(id) => (*id, false),
        };
        if !changed_ids.insert(id) {
            return Err(validation_error(
                "manual.duplicate_lock_change",
                "/lockChanges",
                "A lock may be changed only once in one manual admission.",
            ));
        }
        let exists = document.domain.locked_assignments.contains_key(&id);
        if upsert && !exists {
            final_count = final_count
                .checked_add(1)
                .ok_or_else(solution_contract_error)?;
        } else if !upsert && exists {
            final_count = final_count
                .checked_sub(1)
                .ok_or_else(solution_contract_error)?;
        } else if !upsert {
            return Err(validation_error(
                "manual.lock_missing",
                "/lockChanges",
                "A removed assignment lock must exist in the current Workforce document.",
            ));
        }
    }
    if final_count > MAX_SELECTED_PAIRS {
        return Err(validation_error(
            "manual.lock_changes_too_large",
            "/lockChanges",
            "The Workforce document exceeds the supported assignment lock limit.",
        ));
    }
    let mut before = BTreeMap::new();
    let mut after = BTreeMap::new();
    for change in changes {
        budget.step().map_err(|error| {
            manual_budget_error(
                error,
                "/lockChanges",
                "The lock changes exceed native resource limits.",
            )
        })?;
        let (id, value) = match change {
            ManualLockChangeV1::Upsert(lock) => (
                lock.id,
                Some(serde_json::to_value(lock).map_err(|_| solution_contract_error())?),
            ),
            ManualLockChangeV1::Remove(id) => (*id, None),
        };
        let previous = document.domain.locked_assignments.get(&id).cloned();
        before.insert(id, previous);
        after.insert(id, value.clone());
        if let Some(value) = value {
            document.domain.locked_assignments.insert(id, value);
        } else {
            document.domain.locked_assignments.remove(&id);
        }
    }
    Ok((before, after))
}

fn apply_protected_transition(
    document: &mut ScenarioDocument,
    transition: &ProtectedTransition,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<(), StoreError> {
    let before_len = transition.before.lock_values.len();
    let after_len = transition.after.lock_values.len();
    if !u64::try_from(before_len).is_ok_and(|len| len <= MAX_SELECTED_PAIRS)
        || !u64::try_from(after_len).is_ok_and(|len| len <= MAX_SELECTED_PAIRS)
    {
        return Err(private_history_integrity());
    }
    let lock_steps = u64::try_from(before_len)
        .ok()
        .and_then(|before| before.checked_add(u64::try_from(after_len).ok()?))
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(private_history_integrity)?;
    let entity_steps =
        u64::try_from(document.domain.entities.len()).map_err(|_| private_history_integrity())?;
    budget
        .steps(
            lock_steps
                .checked_add(entity_steps)
                .ok_or_else(private_history_integrity)?,
        )
        .map_err(replay_budget_error)?;
    if !transition
        .before
        .lock_values
        .keys()
        .eq(transition.after.lock_values.keys())
    {
        return Err(private_history_integrity());
    }
    let current = find_manual_value(document)?;
    if current
        != transition
            .before
            .manual_selection
            .as_ref()
            .map(|record| (&record.id, &record.value))
    {
        return Err(StoreError::StaleScenarioCustody);
    }
    let current_id = current.map(|(id, _)| *id);
    for (id, expected) in &transition.before.lock_values {
        if document.domain.locked_assignments.get(id) != expected.as_ref() {
            return Err(StoreError::StaleScenarioCustody);
        }
    }
    budget
        .reserve_json(&transition.after, 1)
        .map_err(replay_budget_error)?;
    if let Some(id) = current_id {
        document.domain.entities.remove(&id);
    }
    if let Some(manual) = &transition.after.manual_selection {
        document
            .domain
            .entities
            .insert(manual.id, manual.value.clone());
    }
    for (id, value) in &transition.after.lock_values {
        if let Some(value) = value {
            document
                .domain
                .locked_assignments
                .insert(*id, value.clone());
        } else {
            document.domain.locked_assignments.remove(id);
        }
    }
    Ok(())
}

pub(super) fn find_manual_value(
    document: &ScenarioDocument,
) -> Result<Option<(&EntityId, &Value)>, StoreError> {
    let mut values =
        document.domain.entities.iter().filter(|(_, value)| {
            value.get("kind").and_then(Value::as_str) == Some("manualSelection")
        });
    let value = values.next();
    if values.next().is_some() {
        return Err(StoreError::StaleScenarioCustody);
    }
    Ok(value)
}

fn protected_change_set(transition: &ProtectedTransition) -> ChangeSet {
    let mut changes = Vec::new();
    let before = &transition.before.manual_selection;
    let after = &transition.after.manual_selection;
    if before != after {
        let Some(record) = after.as_ref().or(before.as_ref()) else {
            unreachable!()
        };
        changes.push(Change {
            kind: match (before, after) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Removed,
                (Some(_), Some(_)) => ChangeKind::Updated,
                (None, None) => unreachable!(),
            },
            path: format!("/domain/entities/{}", record.id),
            before: before.as_ref().map(|_| Value::Bool(true)),
            after: after.as_ref().map(|_| Value::Bool(true)),
        });
    }
    for (id, before) in &transition.before.lock_values {
        let after = transition
            .after
            .lock_values
            .get(id)
            .and_then(Option::as_ref);
        let present_before = before.is_some();
        let present_after = after.is_some();
        if present_before != present_after || before.as_ref() != after {
            changes.push(Change {
                kind: match (present_before, present_after) {
                    (false, true) => ChangeKind::Locked,
                    (true, false) => ChangeKind::Unlocked,
                    _ => ChangeKind::Updated,
                },
                path: format!("/domain/lockedAssignments/{id}"),
                before: Some(Value::Bool(present_before)),
                after: Some(Value::Bool(present_after)),
            });
        }
    }
    ChangeSet { changes }
}

pub(super) fn validation_delta(
    before: &ValidationReport,
    after: &ValidationReport,
) -> ValidationDelta {
    let added = after
        .issues
        .iter()
        .filter(|issue| !before.issues.contains(issue))
        .cloned()
        .collect();
    let mut resolved = Vec::new();
    for issue in &before.issues {
        if !after
            .issues
            .iter()
            .any(|candidate| candidate.code == issue.code)
            && !resolved.contains(&issue.code)
        {
            resolved.push(issue.code.clone());
        }
    }
    ValidationDelta { added, resolved }
}

fn result_for_private_replay(
    before: &ScenarioDocument,
    after: &ScenarioDocument,
    revision: Revision,
    change_set: ChangeSet,
    registry: &super::DomainPackRegistry,
) -> CommandResult {
    CommandResult {
        new_revision: revision,
        change_set,
        validation_delta: validation_delta(&validate(before, registry), &validate(after, registry)),
        inverse: None,
    }
}

fn validate_manual_document(
    document: &ScenarioDocument,
    registry: &super::DomainPackRegistry,
) -> Result<(), AppError> {
    super::validate_document_shape(document)
        .map_err(|error| store_error(super::command_store_error(&error, None)))?;
    let report = validate(document, registry);
    if report
        .issues
        .iter()
        .any(|issue| issue.severity == ValidationSeverity::Error)
    {
        return Err(AppError::Validation(report));
    }
    Ok(())
}

fn validate_document_for_replay(
    document: &ScenarioDocument,
    registry: &super::DomainPackRegistry,
) -> Result<(), StoreError> {
    super::validate_document_shape(document)
        .map_err(|error| super::command_store_error(&error, None))?;
    super::ensure_supported_document(document, registry)?;
    Ok(())
}

pub(super) fn private_actor(_actor: ActorRef) -> ActorRef {
    ActorRef {
        actor_id: None,
        display_name: "Private history entry".to_owned(),
    }
}

fn document_hash(document: &ScenarioDocument) -> Result<String, AppError> {
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, document).map_err(|_| solution_contract_error())?;
    Ok(hasher.finalize().to_hex().to_string())
}

// These owned errors are consumed by existing `map_err` callbacks at conversion boundaries.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn manual_budget_error(error: DomainPackError, path: &str, message: &str) -> AppError {
    super::domain_interruption(&error)
        .unwrap_or_else(|| validation_error("manual.resource_limit", path, message))
}
// These owned errors are consumed by existing `map_err` callbacks at conversion boundaries.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn manual_integrity_error(error: DomainPackError) -> AppError {
    super::domain_interruption(&error).unwrap_or_else(solution_verification_mismatch)
}
