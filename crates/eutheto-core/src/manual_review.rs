use super::{
    AcceptedResultRefV1, ActorRef, AppError, AssignmentId, AssignmentLock, AssignmentPair,
    BTreeMap, BTreeSet, CommandId, CommandResult, Deserialize, EntityId, EuthetoApp,
    MAX_SELECTED_PAIRS, ManualLockChangeV1, ManualSelection, OperationControl, RequestId, Revision,
    RunBaselineOriginal, ScenarioDocument, ScenarioId, Serialize, StoreError, Value,
    WorkforceAdmissionBudget, manual_integrity_error, operation_interrupted,
    solution_contract_error, solution_verification_mismatch, store_error, validation_error,
};
use eutheto_domain_ir::{DomainEntityRef, RuleEvaluation, VerificationValue};
use eutheto_types::PersonId;
use eutheto_workforce::{
    ids::ShiftId,
    model::LockState,
    temporal::{ResolvedShift, ResolvedShiftOrigin},
};
use std::io::Write;

pub const MANUAL_EDIT_SCHEMA_VERSION: u32 = 1;
pub const MAX_MANUAL_EDIT_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_MANUAL_EDIT_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManualProtectionV1 {
    Unlocked,
    Hard,
    Soft { stability_weight: u32 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualProtectionChangeV1 {
    pub person_id: PersonId,
    pub shift_id: ShiftId,
    pub protection: ManualProtectionV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManualEditIntentV1 {
    Add {
        person_id: PersonId,
        shift_id: ShiftId,
        protection_changes: Vec<ManualProtectionChangeV1>,
    },
    Remove {
        person_id: PersonId,
        shift_id: ShiftId,
        protection_changes: Vec<ManualProtectionChangeV1>,
    },
    Replace {
        incumbent_person_id: PersonId,
        target_person_id: PersonId,
        shift_id: ShiftId,
        protection_changes: Vec<ManualProtectionChangeV1>,
    },
    Protection {
        person_id: PersonId,
        shift_id: ShiftId,
        protection: ManualProtectionV1,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManualEditRequestV1 {
    Read {
        schema_version: u32,
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        offset: u32,
        limit: u32,
    },
    Review {
        schema_version: u32,
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        intent: ManualEditIntentV1,
    },
    Apply {
        schema_version: u32,
        request_id: RequestId,
        scenario_id: ScenarioId,
        expected_revision: Revision,
        intent: ManualEditIntentV1,
        approval_digest: String,
        command_id: CommandId,
        actor: ActorRef,
        truncate_redo: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualEditSourceV1 {
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub document_hash: String,
    pub original: RunBaselineOriginal,
    pub current_manual_id: Option<EntityId>,
    pub current_manual_checksum: Option<String>,
    pub selected_count: u32,
    pub protection_count: u32,
    pub authored_optimize_unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualEditRowV1 {
    pub person_id: PersonId,
    pub shift_id: ShiftId,
    pub person_label: String,
    pub shift_label: String,
    pub selected: bool,
    pub lock_id: Option<AssignmentId>,
    pub protection: ManualProtectionV1,
    pub person_available: bool,
    pub shift_available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualEditReadV1 {
    pub schema_version: u32,
    pub source: ManualEditSourceV1,
    pub offset: u32,
    pub total_rows: u32,
    pub next_offset: Option<u32>,
    pub rows: Vec<ManualEditRowV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualEditEvaluationV1 {
    pub evaluation: RuleEvaluation,
    pub owner_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualEditChangeV1 {
    pub before: ManualEditRowV1,
    pub after: ManualEditRowV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManualEditDispositionV1 {
    Valid,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualEditReviewV1 {
    pub schema_version: u32,
    pub source: ManualEditSourceV1,
    pub intent: ManualEditIntentV1,
    pub changes: Vec<ManualEditChangeV1>,
    pub evaluations: Vec<ManualEditEvaluationV1>,
    pub disposition: ManualEditDispositionV1,
    pub approval_digest: Option<String>,
}

/// A committed receipt remains successful even if a subsequent established source alarm fires.
#[derive(Clone, Debug)]
pub struct ManualEditAppliedV1 {
    pub schema_version: u32,
    pub command_result: CommandResult,
    pub source_admission_failure: Option<AppError>,
}

#[derive(Clone, Debug)]
pub enum ManualEditResultV1 {
    Read { read: ManualEditReadV1 },
    Review { review: ManualEditReviewV1 },
    Applied { applied: ManualEditAppliedV1 },
}

pub(super) enum ManualAction {
    Read {
        offset: u32,
        limit: u32,
    },
    Review(ManualEditIntentV1),
    Apply {
        intent: ManualEditIntentV1,
        approval_digest: String,
    },
    Native,
}
pub(super) struct ManualInput {
    pub schema_version: u32,
    pub request_id: RequestId,
    pub command_id: Option<CommandId>,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub actor: Option<ActorRef>,
    pub original: Option<AcceptedResultRefV1>,
    pub assignments: Vec<AssignmentPair>,
    pub lock_changes: Vec<ManualLockChangeV1>,
    pub truncate_redo: bool,
    pub action: ManualAction,
}

pub(super) fn wire_bound<T: Serialize>(value: &T, max: usize) -> Result<(), AppError> {
    struct Bound(usize, usize);
    impl Write for Bound {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("manual wire bound"))?;
            if self.0 > self.1 {
                return Err(std::io::Error::other("manual wire bound"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Bound(0, max), value).map_err(|_| {
        validation_error(
            "manual.wire_bound",
            "/manual",
            "The complete manual request or response exceeds its byte limit.",
        )
    })
}

impl EuthetoApp {
    /// Native owned, stateless read/review/apply. The complete authenticated selection is authoritative.
    /// # Errors
    /// Rejects stale custody, malformed intent, interruption, incomplete evaluation and resource limits.
    pub async fn manual_edit(
        &self,
        request: ManualEditRequestV1,
        control: OperationControl,
    ) -> Result<ManualEditResultV1, AppError> {
        control.check().map_err(operation_interrupted)?;
        match &request {
            ManualEditRequestV1::Read { schema_version, .. }
            | ManualEditRequestV1::Review { schema_version, .. }
            | ManualEditRequestV1::Apply { schema_version, .. }
                if *schema_version != 1 =>
            {
                return Err(super::super::protocol_error(
                    "manual.schema_unsupported",
                    "The manual-edit request schema is unsupported.",
                    false,
                ));
            }
            _ => {}
        }
        match &request {
            ManualEditRequestV1::Review { intent, .. }
            | ManualEditRequestV1::Apply { intent, .. } => check_intent_shape(intent)?,
            ManualEditRequestV1::Read { .. } => {}
        }
        wire_bound(&request, MAX_MANUAL_EDIT_REQUEST_BYTES)?;
        let input = match request {
            ManualEditRequestV1::Read {
                schema_version,
                request_id,
                scenario_id,
                expected_revision,
                offset,
                limit,
            } => {
                if limit == 0 || limit > 100 {
                    return Err(intent_error("The page limit must be between 1 and 100."));
                }
                ManualInput {
                    schema_version,
                    request_id,
                    scenario_id,
                    expected_revision,
                    command_id: None,
                    actor: None,
                    original: None,
                    assignments: Vec::new(),
                    lock_changes: Vec::new(),
                    truncate_redo: false,
                    action: ManualAction::Read { offset, limit },
                }
            }
            ManualEditRequestV1::Review {
                schema_version,
                request_id,
                scenario_id,
                expected_revision,
                intent,
            } => ManualInput {
                schema_version,
                request_id,
                scenario_id,
                expected_revision,
                command_id: None,
                actor: None,
                original: None,
                assignments: Vec::new(),
                lock_changes: Vec::new(),
                truncate_redo: false,
                action: ManualAction::Review(intent),
            },
            ManualEditRequestV1::Apply {
                schema_version,
                request_id,
                scenario_id,
                expected_revision,
                intent,
                approval_digest,
                command_id,
                actor,
                truncate_redo,
            } => ManualInput {
                schema_version,
                request_id,
                scenario_id,
                expected_revision,
                command_id: Some(command_id),
                actor: Some(actor),
                original: None,
                assignments: Vec::new(),
                lock_changes: Vec::new(),
                truncate_redo,
                action: ManualAction::Apply {
                    intent,
                    approval_digest,
                },
            },
        };
        self.run_manual_blocking(input, control).await
    }

    pub(super) async fn run_manual_blocking(
        &self,
        request: ManualInput,
        control: OperationControl,
    ) -> Result<ManualEditResultV1, AppError> {
        control.check().map_err(operation_interrupted)?;
        let mut caller_guard = super::super::CancelOnDrop::new(control.cancellation().clone());
        let query_slot = controlled_wait(
            self.acquire_accepted_query_slot(control.cancellation()),
            &control,
        )
        .await??;
        let app = self.clone();
        let runtime = tokio::runtime::Handle::current();
        let result = tokio::task::spawn_blocking(move || {
            let _query_slot = query_slot;
            runtime.block_on(app.run_manual(request, control))
        })
        .await
        .map_err(super::super::join_error)?;
        caller_guard.disarm();
        result
    }
}

fn intent_error(message: &'static str) -> AppError {
    validation_error("manual.intent_invalid", "/intent", message)
}
fn pair(person_id: PersonId, shift_id: ShiftId) -> AssignmentPair {
    AssignmentPair {
        person_id,
        shift_id,
    }
}
fn protection(state: LockState) -> ManualProtectionV1 {
    match state {
        LockState::Unlocked {} => ManualProtectionV1::Unlocked,
        LockState::Hard {} => ManualProtectionV1::Hard,
        LockState::Soft { stability_weight } => ManualProtectionV1::Soft { stability_weight },
    }
}

pub(super) fn locks(
    document: &ScenarioDocument,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<BTreeMap<AssignmentPair, AssignmentLock>, AppError> {
    let mut result = BTreeMap::new();
    for (id, value) in &document.domain.locked_assignments {
        budget.step().map_err(manual_integrity_error)?;
        budget
            .reserve_json(value, 2)
            .map_err(manual_integrity_error)?;
        let lock: AssignmentLock =
            serde_json::from_value(value.clone()).map_err(|_| solution_contract_error())?;
        if lock.id != *id
            || result
                .insert(pair(lock.person_id, lock.shift_id), lock)
                .is_some()
        {
            return Err(solution_verification_mismatch());
        }
    }
    Ok(result)
}

pub(super) fn source_view(
    document: &ScenarioDocument,
    revision: Revision,
    document_hash: &str,
    original: &RunBaselineOriginal,
    manual: Option<&(EntityId, Value, ManualSelection)>,
    assignments: &[AssignmentPair],
    locks: &BTreeMap<AssignmentPair, AssignmentLock>,
) -> Result<ManualEditSourceV1, AppError> {
    Ok(ManualEditSourceV1 {
        scenario_id: document.scenario_id,
        revision,
        document_hash: document_hash.to_owned(),
        original: original.clone(),
        current_manual_id: manual.map(|(id, _, _)| *id),
        current_manual_checksum: manual.map(|(_, _, record)| record.checksum.clone()),
        selected_count: u32::try_from(assignments.len()).map_err(|_| solution_contract_error())?,
        protection_count: u32::try_from(locks.len()).map_err(|_| solution_contract_error())?,
        authored_optimize_unavailable_reason: locks
            .values()
            .any(|lock| matches!(lock.state, LockState::Soft { .. }))
            .then(|| "Soft protection requires the Repair MinimalChanges objective; ordinary Authored Optimize is unavailable.".to_owned()),
    })
}

pub(super) type ShiftCatalogue = BTreeMap<ShiftId, ResolvedShift>;

pub(super) fn shift_catalogue(
    document: &ScenarioDocument,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<ShiftCatalogue, AppError> {
    let resolved = budget
        .resolved_shifts(document)
        .map_err(manual_integrity_error)?;
    let mut catalogue = BTreeMap::new();
    for shift in resolved {
        budget.step().map_err(manual_integrity_error)?;
        budget.reserve(1, 1, 96).map_err(manual_integrity_error)?;
        if catalogue.insert(shift.id, shift).is_some() {
            return Err(solution_verification_mismatch());
        }
    }
    Ok(catalogue)
}

fn shift_owner(shift: &ResolvedShift) -> EntityId {
    match shift.origin {
        ResolvedShiftOrigin::Generated { template_id, .. } => template_id.as_entity_id(),
        ResolvedShiftOrigin::Manual | ResolvedShiftOrigin::Detached { .. } => {
            shift.id.as_entity_id()
        }
    }
}

fn current_pair(
    document: &ScenarioDocument,
    catalogue: &ShiftCatalogue,
    pair: AssignmentPair,
) -> Result<(), AppError> {
    if document
        .domain
        .entities
        .get(&EntityId::from_uuid(pair.person_id.as_uuid()))
        .and_then(|person| person.get("kind"))
        .and_then(Value::as_str)
        != Some("person")
    {
        return Err(intent_error(
            "The person is unavailable in the current source.",
        ));
    }
    if !catalogue.contains_key(&pair.shift_id) {
        return Err(intent_error(
            "The exact work occurrence is unavailable in the current source.",
        ));
    }
    Ok(())
}

pub(super) fn row(
    document: &ScenarioDocument,
    catalogue: &ShiftCatalogue,
    pair: AssignmentPair,
    assignments: &[AssignmentPair],
    locks: &BTreeMap<AssignmentPair, AssignmentLock>,
) -> Result<ManualEditRowV1, AppError> {
    let person = document
        .domain
        .entities
        .get(&EntityId::from_uuid(pair.person_id.as_uuid()))
        .filter(|person| person.get("kind").and_then(Value::as_str) == Some("person"));
    let person_label = person
        .map(|person| {
            person
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(solution_contract_error)
        })
        .transpose()?
        .unwrap_or("Person unavailable in current source");
    let shift = catalogue.get(&pair.shift_id);
    let shift_label = if let Some(shift) = shift {
        let definition = document
            .domain
            .entities
            .get(&shift_owner(shift))
            .ok_or_else(solution_contract_error)?;
        let assignment_type_id = definition
            .get("assignmentTypeId")
            .and_then(Value::as_str)
            .ok_or_else(solution_contract_error)?
            .parse::<EntityId>()
            .map_err(|_| solution_contract_error())?;
        let assignment_type = document
            .domain
            .entities
            .get(&assignment_type_id)
            .ok_or_else(solution_contract_error)?;
        let assignment_type_label = assignment_type
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(solution_contract_error)?;
        format!(
            "{assignment_type_label} — {}",
            shift.interval.starts_at.local
        )
    } else {
        "Work unavailable in current source".to_owned()
    };
    let lock = locks.get(&pair);
    Ok(ManualEditRowV1 {
        person_id: pair.person_id,
        shift_id: pair.shift_id,
        person_label: person_label.to_owned(),
        shift_label,
        selected: assignments.binary_search(&pair).is_ok(),
        lock_id: lock.map(|lock| lock.id),
        protection: lock.map_or(ManualProtectionV1::Unlocked, |lock| protection(lock.state)),
        person_available: person.is_some(),
        shift_available: shift.is_some(),
    })
}

// Keep pair mutation and its explicit protection transitions in one auditable admission pass.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn derive_intent(
    document: &ScenarioDocument,
    catalogue: &ShiftCatalogue,
    intent: &ManualEditIntentV1,
    assignments: &mut Vec<AssignmentPair>,
    current_locks: &BTreeMap<AssignmentPair, AssignmentLock>,
    budget: &mut WorkforceAdmissionBudget<'_>,
    source_hash: &str,
    occupied: &BTreeSet<uuid::Uuid>,
) -> Result<(Vec<ManualLockChangeV1>, BTreeSet<AssignmentPair>), AppError> {
    check_intent_shape(intent)?;
    let (removed, added, transitions) = match intent {
        ManualEditIntentV1::Add {
            person_id,
            shift_id,
            protection_changes,
        } => (
            None,
            Some(pair(*person_id, *shift_id)),
            protection_changes.clone(),
        ),
        ManualEditIntentV1::Remove {
            person_id,
            shift_id,
            protection_changes,
        } => (
            Some(pair(*person_id, *shift_id)),
            None,
            protection_changes.clone(),
        ),
        ManualEditIntentV1::Replace {
            incumbent_person_id,
            target_person_id,
            shift_id,
            protection_changes,
        } => {
            if incumbent_person_id == target_person_id {
                return Err(intent_error("A replacement must change the person."));
            }
            (
                Some(pair(*incumbent_person_id, *shift_id)),
                Some(pair(*target_person_id, *shift_id)),
                protection_changes.clone(),
            )
        }
        ManualEditIntentV1::Protection {
            person_id,
            shift_id,
            protection,
        } => (
            None,
            None,
            vec![ManualProtectionChangeV1 {
                person_id: *person_id,
                shift_id: *shift_id,
                protection: *protection,
            }],
        ),
    };
    let mut touched = BTreeSet::new();
    if let Some(removed) = removed {
        let index = assignments
            .binary_search(&removed)
            .map_err(|_| intent_error("The removed assignment is not selected."))?;
        assignments.remove(index);
        touched.insert(removed);
    }
    if let Some(added) = added {
        current_pair(document, catalogue, added)?;
        let index = assignments
            .binary_search(&added)
            .err()
            .ok_or_else(|| intent_error("The added assignment is already selected."))?;
        assignments.insert(index, added);
        touched.insert(added);
    }
    let mut seen = BTreeSet::new();
    let mut changes = Vec::new();
    let mut allocated = BTreeSet::new();
    for transition in transitions {
        budget.step().map_err(manual_integrity_error)?;
        let pair = pair(transition.person_id, transition.shift_id);
        if !seen.insert(pair) || (removed.is_some() || added.is_some()) && !touched.contains(&pair)
        {
            return Err(intent_error(
                "Protection transitions must be unique and belong to this edit's exact endpoints.",
            ));
        }
        let old = current_locks.get(&pair);
        if old.map_or(ManualProtectionV1::Unlocked, |lock| protection(lock.state))
            == transition.protection
            && !(transition.protection == ManualProtectionV1::Unlocked && old.is_some())
        {
            return Err(intent_error(
                "A protection transition must change the current protection.",
            ));
        }
        match transition.protection {
            ManualProtectionV1::Unlocked => changes.push(ManualLockChangeV1::Remove(
                old.ok_or_else(|| intent_error("There is no source lock to clear."))?
                    .id,
            )),
            state => {
                current_pair(document, catalogue, pair)?;
                if assignments.binary_search(&pair).is_err() {
                    return Err(intent_error(
                        "Hard or Soft protection requires prospective selected membership.",
                    ));
                }
                let state = match state {
                    ManualProtectionV1::Hard => LockState::Hard {},
                    ManualProtectionV1::Soft { stability_weight }
                        if stability_weight > 0
                            && stability_weight <= eutheto_workforce::validation::MAX_WEIGHT =>
                    {
                        LockState::Soft { stability_weight }
                    }
                    _ => {
                        return Err(intent_error(
                            "Soft stability weight must be positive and at most 1000000.",
                        ));
                    }
                };
                let id = if let Some(lock) = old {
                    lock.id
                } else {
                    ephemeral_lock_id(source_hash, pair, occupied, &mut allocated, budget)?
                };
                changes.push(ManualLockChangeV1::Upsert(AssignmentLock {
                    id,
                    person_id: pair.person_id,
                    shift_id: pair.shift_id,
                    state,
                }));
            }
        }
        touched.insert(pair);
    }
    if assignments.len() as u64 > MAX_SELECTED_PAIRS {
        return Err(intent_error(
            "The complete prospective selection exceeds its limit.",
        ));
    }
    Ok((changes, touched))
}

pub(super) fn owner_paths(
    document: &ScenarioDocument,
    catalogue: &ShiftCatalogue,
    evaluation: &RuleEvaluation,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<Vec<String>, AppError> {
    let count = evaluation.affected_entities.len() as u64 + 3;
    budget.steps(count).map_err(manual_integrity_error)?;
    budget
        .reserve(0, count, count * 160)
        .map_err(manual_integrity_error)?;
    let mut paths = BTreeSet::new();
    if document.domain.rules.contains_key(&evaluation.rule_id) {
        paths.insert(format!("/domain/rules/{}", evaluation.rule_id));
    }
    let id = EntityId::from_uuid(evaluation.rule_id.as_uuid());
    if let Some(entity) = document.domain.entities.get(&id) {
        match entity.get("kind").and_then(Value::as_str) {
            Some("person") => {
                paths.insert(format!("/domain/entities/{id}/activeRange"));
            }
            Some("availability") => {
                paths.insert(format!("/domain/entities/{id}/timeWindow"));
            }
            _ => {}
        }
    }
    let lock_id = AssignmentId::from_uuid(evaluation.rule_id.as_uuid());
    if document.domain.locked_assignments.contains_key(&lock_id) {
        paths.insert(format!("/domain/lockedAssignments/{lock_id}"));
    }
    for entity in &evaluation.affected_entities {
        if let Some(path) = referenced_owner(document, catalogue, evaluation, entity) {
            paths.insert(path);
        }
    }
    Ok(paths.into_iter().collect())
}

fn referenced_owner(
    document: &ScenarioDocument,
    catalogue: &ShiftCatalogue,
    evaluation: &RuleEvaluation,
    reference: &DomainEntityRef,
) -> Option<String> {
    // Match the producer's closed kind vocabulary before decoding its exact Workforce namespace.
    let (stored_kind, field) = match reference.kind.as_str() {
        "official.workforce.shift_template" => ("shiftTemplate", "/coverage"),
        "official.workforce.shift_instance" => ("shiftInstance", "/coverage"),
        "official.workforce.coverage_requirement" => ("coverageRequirement", "/coverage"),
        "official.workforce.availability" => ("availability", "/timeWindow"),
        "official.workforce.assignment_type" => ("assignmentType", ""),
        "official.workforce.person" => ("person", "/activeRange"),
        "official.workforce.shift" => ("shift", ""),
        _ => return None,
    };
    let id = reference
        .id
        .as_str()
        .strip_prefix("official.workforce.")?
        .parse::<EntityId>()
        .ok()?;
    if stored_kind == "shift" {
        let shift = catalogue.get(&ShiftId::try_from(id).ok()?)?;
        let owner = shift_owner(shift);
        let expected = match shift.origin {
            ResolvedShiftOrigin::Generated { .. } => "shiftTemplate",
            ResolvedShiftOrigin::Manual | ResolvedShiftOrigin::Detached { .. } => "shiftInstance",
        };
        if document
            .domain
            .entities
            .get(&owner)?
            .get("kind")?
            .as_str()?
            != expected
        {
            return None;
        }
        return Some(format!("/domain/entities/{owner}"));
    }
    if document.domain.entities.get(&id)?.get("kind")?.as_str()? != stored_kind {
        return None;
    }
    if stored_kind == "person" {
        let reason = evaluation
            .observed
            .iter()
            .find(|(key, _)| key.as_str() == "official.workforce.fact.witness_reason")?
            .1;
        if id.as_uuid() != evaluation.rule_id.as_uuid()
            || !matches!(reason, VerificationValue::Text(value) if value == "outside_active_range")
        {
            return None;
        }
    }
    Some(format!("/domain/entities/{id}{field}"))
}

// Keep the exact authenticated source and complete prospective meaning on one digest boundary.
#[allow(clippy::too_many_arguments)]
pub(super) fn approval_digest(
    source: &ScenarioDocument,
    revision: Revision,
    original: &RunBaselineOriginal,
    intent: &ManualEditIntentV1,
    assignments: &[AssignmentPair],
    source_locks: &BTreeMap<AssignmentPair, AssignmentLock>,
    prospective_locks: &BTreeMap<AssignmentPair, AssignmentLock>,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<String, AppError> {
    budget
        .reserve(
            0,
            prospective_locks.len() as u64,
            (prospective_locks.len() as u64) * 64,
        )
        .map_err(manual_integrity_error)?;
    let lock_meaning: Vec<_> = prospective_locks
        .iter()
        .map(|(pair, lock)| (*pair, source_locks.get(pair).map(|old| old.id), lock.state))
        .collect();
    let meaning = (
        "eutheto/manual-edit/v1",
        source,
        revision,
        original,
        intent,
        assignments,
        lock_meaning,
    );
    budget
        .preflight(&meaning)
        .and_then(|()| budget.reserve(1, 0, 64))
        .map_err(manual_integrity_error)?;
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, &meaning).map_err(|_| solution_contract_error())?;
    budget.check().map_err(manual_integrity_error)?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub(in crate::application) async fn controlled_wait<F: Future>(
    future: F,
    control: &OperationControl,
) -> Result<F::Output, AppError> {
    tokio::pin!(future);
    loop {
        control.check().map_err(operation_interrupted)?;
        tokio::select! {
            result = &mut future => {
                control.check().map_err(operation_interrupted)?;
                return Ok(result);
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
}

// Scratch identities are deterministic evaluation labels, never receipts or durable lock IDs.
fn ephemeral_lock_id(
    source_hash: &str,
    pair: AssignmentPair,
    occupied: &BTreeSet<uuid::Uuid>,
    allocated: &mut BTreeSet<uuid::Uuid>,
    budget: &mut WorkforceAdmissionBudget<'_>,
) -> Result<AssignmentId, AppError> {
    for salt in 0..64_u32 {
        budget.step().map_err(manual_integrity_error)?;
        let preimage = serde_json::to_vec(&(
            "eutheto/manual-edit/scratch-lock/v1",
            source_hash,
            pair,
            salt,
        ))
        .map_err(|_| solution_contract_error())?;
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&blake3::hash(&preimage).as_bytes()[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let id = uuid::Uuid::from_bytes(bytes);
        if !occupied.contains(&id) && allocated.insert(id) {
            return Ok(AssignmentId::from_uuid(id));
        }
    }
    Err(solution_contract_error())
}

fn check_intent_shape(intent: &ManualEditIntentV1) -> Result<(), AppError> {
    let changes = match intent {
        ManualEditIntentV1::Add {
            protection_changes, ..
        }
        | ManualEditIntentV1::Remove {
            protection_changes, ..
        } => protection_changes.as_slice(),
        ManualEditIntentV1::Replace {
            incumbent_person_id,
            target_person_id,
            protection_changes,
            ..
        } => {
            if incumbent_person_id == target_person_id {
                return Err(intent_error("A replacement must change the person."));
            }
            protection_changes.as_slice()
        }
        ManualEditIntentV1::Protection { protection, .. } => {
            check_protection(*protection)?;
            return Ok(());
        }
    };
    if changes.len() > 2 {
        return Err(intent_error(
            "At most two explicit protection transitions are allowed.",
        ));
    }
    let mut pairs = BTreeSet::new();
    for change in changes {
        check_protection(change.protection)?;
        if !pairs.insert(pair(change.person_id, change.shift_id)) {
            return Err(intent_error("Protection transitions must be unique."));
        }
    }
    Ok(())
}

fn check_protection(protection: ManualProtectionV1) -> Result<(), AppError> {
    if let ManualProtectionV1::Soft { stability_weight } = protection
        && (stability_weight == 0 || stability_weight > eutheto_workforce::validation::MAX_WEIGHT)
    {
        return Err(intent_error(
            "Soft stability weight must be positive and at most 1000000.",
        ));
    }
    Ok(())
}

pub(super) fn controlled_store_error(error: StoreError, control: &OperationControl) -> AppError {
    control
        .check()
        .map_or_else(operation_interrupted, |()| store_error(error))
}
