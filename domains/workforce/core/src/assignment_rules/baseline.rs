//! Immutable baseline content checks. None of these helpers authenticates local journal custody.

use super::{
    AssignmentRuleLimit,
    authority::{contract, operation_error},
    boundary::preflight,
    budget::{MAX_SELECTED_PAIRS, OperationBudget, count, within},
};
use crate::ids::ShiftId;
use crate::model::{
    AssignmentPair, BaseSchedule, ManualSelection, WorkforceDomainV1, WorkforceEntity,
};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError};
use eutheto_domain_ir::{
    DomainEntityId, DomainEntityKindId, DomainEntityRef, MAX_CAPTURED_BASELINE_BYTES,
    RunBaselineBinding, RunBaselineSelection, RunExternalBaselineBinding, RunObjectiveContext,
};
use eutheto_types::{
    EntityId, OperationControl, PersonId, REVISION_MAX_V1, Revision, ScenarioDocument, ScenarioId,
    SolutionId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{borrow::Cow, collections::BTreeMap, str::FromStr};
use uuid::Uuid;

const ORIGINAL_KIND: &str = "official.workforce.base-schedule";
const MANUAL_KIND: &str = "official.workforce.manual-selection";
const EXTERNAL_KIND: &str = "official.workforce.external-starting-plan";
const ENTITY_PREFIX: &str = "official.workforce.";

/// Creates the pack-owned reference to an original base or immutable manual generation.
///
/// # Errors
/// Rejects an invalid stable reference; the reference alone is not admission authority.
pub fn workforce_baseline_reference(
    entity_id: EntityId,
    manual: bool,
) -> Result<DomainEntityRef, DomainPackError> {
    baseline_reference(entity_id, if manual { MANUAL_KIND } else { ORIGINAL_KIND })
}

/// Creates an external starting-plan reference without accepted-result ancestry.
///
/// # Errors
/// Rejects an invalid stable reference; this is content identity only.
pub fn workforce_external_baseline_reference(
    entity_id: EntityId,
) -> Result<DomainEntityRef, DomainPackError> {
    baseline_reference(entity_id, EXTERNAL_KIND)
}

fn baseline_reference(entity_id: EntityId, kind: &str) -> Result<DomainEntityRef, DomainPackError> {
    Ok(DomainEntityRef {
        kind: DomainEntityKindId::new(kind).map_err(|_| contract("baseline_reference"))?,
        id: DomainEntityId::new(format!("{ENTITY_PREFIX}{entity_id}"))
            .map_err(|_| contract("baseline_reference"))?,
    })
}

/// Hashes complete pair content independent of submitted ordering, rejecting duplicate pairs.
///
/// # Errors
/// Rejects duplicates, cancellation/deadline and existing finite-work/retention limits.
pub fn workforce_pair_content_hash(
    pairs: &[AssignmentPair],
    control: Option<&OperationControl>,
) -> Result<String, DomainPackError> {
    pair_content_hash(pairs, &mut OperationBudget::evaluation(control))
}

pub(super) fn pair_content_hash(
    pairs: &[AssignmentPair],
    budget: &mut OperationBudget<'_>,
) -> Result<String, DomainPackError> {
    budget.check().map_err(|error| operation_error(&error))?;
    within(
        count(pairs.len()).map_err(|error| operation_error(&error))?,
        MAX_SELECTED_PAIRS,
        AssignmentRuleLimit::SelectedPairs,
    )
    .map_err(|error| operation_error(&error))?;
    preflight(pairs, budget, ContractJsonLimits::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    if pairs.is_sorted() {
        if pairs.windows(2).any(|window| window[0] == window[1]) {
            return Err(contract("duplicate_baseline_pair"));
        }
        return hash_content(b"eutheto.workforce.baseline-pairs.v1\0", pairs, budget);
    }
    budget
        .reserve(
            1,
            count(pairs.len()).map_err(|error| operation_error(&error))?,
            count(
                pairs
                    .len()
                    .checked_mul(size_of::<&AssignmentPair>())
                    .ok_or_else(|| contract("arithmetic"))?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let mut ordered: Vec<_> = pairs.iter().collect();
    budget
        .sort_work(ordered.len())
        .map_err(|error| operation_error(&error))?;
    ordered.sort_unstable();
    if ordered.windows(2).any(|window| window[0] == window[1]) {
        return Err(contract("duplicate_baseline_pair"));
    }
    hash_content(b"eutheto.workforce.baseline-pairs.v1\0", &ordered, budget)
}

/// Computes a manual record checksum from every immutable field except the checksum itself.
///
/// # Errors
/// Rejects cancellation/deadline and existing finite-work/input bounds.
pub fn workforce_manual_selection_checksum(
    record: &ManualSelection,
    control: Option<&OperationControl>,
) -> Result<String, DomainPackError> {
    manual_checksum(record, &mut OperationBudget::evaluation(control))
}

pub(super) fn manual_checksum(
    record: &ManualSelection,
    budget: &mut OperationBudget<'_>,
) -> Result<String, DomainPackError> {
    preflight(record, budget, ContractJsonLimits::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    hash_content(
        b"eutheto.workforce.manual-selection.v1\0",
        &(
            record.schema_version,
            record.id,
            &record.original,
            record.admission_source_scenario_id,
            record.admission_source_revision,
            &record.admission_source_document_hash,
            record.admission_command_id,
            &record.assignments,
        ),
        budget,
    )
}

fn hash_content<T: Serialize + ?Sized>(
    separator: &[u8],
    content: &T,
    budget: &mut OperationBudget<'_>,
) -> Result<String, DomainPackError> {
    budget.check().map_err(|error| operation_error(&error))?;
    budget
        .reserve(1, 0, 64)
        .map_err(|error| operation_error(&error))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(separator);
    serde_json::to_writer(&mut hasher, content).map_err(|_| contract("baseline_hash"))?;
    budget.check().map_err(|error| operation_error(&error))?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn validate_manual_selection_shape(
    record: &ManualSelection,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    budget.check().map_err(|error| operation_error(&error))?;
    if record.schema_version != 1 || record.admission_source_revision.value() > REVISION_MAX_V1 {
        return Err(contract("manual_version_revision"));
    }
    record
        .original
        .validate_for(record.admission_source_scenario_id)
        .map_err(|_| contract("manual_original"))?;
    reference_id(&record.original.entity, ORIGINAL_KIND)?;
    if record.admission_source_document_hash.len() != 64
        || !record
            .admission_source_document_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(contract("manual_source_hash"));
    }
    if record
        .assignments
        .windows(2)
        .any(|window| window[0] >= window[1])
    {
        return Err(contract("manual_pair_order"));
    }
    within(
        count(record.assignments.len()).map_err(|error| operation_error(&error))?,
        MAX_SELECTED_PAIRS,
        AssignmentRuleLimit::SelectedPairs,
    )
    .map_err(|error| operation_error(&error))?;
    Ok(())
}

pub(crate) fn validate_manual_selection_record(
    record: &ManualSelection,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    validate_manual_selection_shape(record, budget)?;
    if manual_checksum(record, budget)? != record.checksum {
        return Err(contract("manual_checksum"));
    }
    Ok(())
}

/// Normalizes checksum-covered copy identities without creating local admission authority.
///
/// Call only after the complete owned-ID remap and before pack validation/persistence. Local
/// record/reference/pair identities follow the owned mapping; the reference mapping validates the
/// generic rewrite before retained historical endpoints and external provenance are restored.
/// Same-identity imports retain the exact original raw manual record and checksum.
///
/// # Errors
/// Rejects mismatched scenarios, records, original bases, mapped pair populations or bounds.
// Keep all fallible source/mapping preparation together before either protected record is mutated.
#[allow(clippy::too_many_lines)]
pub fn normalize_workforce_manual_selection_copy(
    original: &ScenarioDocument,
    copied: &mut ScenarioDocument,
    id_remap: &BTreeMap<Uuid, Uuid>,
    reference_remap: &BTreeMap<Uuid, Uuid>,
    control: Option<&OperationControl>,
) -> Result<(), DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    budget.check().map_err(|error| operation_error(&error))?;
    let is_copy = original.scenario_id != copied.scenario_id;
    if mapped_uuid(original.scenario_id.as_uuid(), id_remap, is_copy)?
        != copied.scenario_id.as_uuid()
    {
        return Err(contract("manual_copy_scenario"));
    }

    let source_manual = manual_copy_record(original, &mut budget)?;
    let mut copied_manual = manual_copy_record(copied, &mut budget)?;
    if let Some(source) = &source_manual {
        validate_manual_selection_record(source, &mut budget)?;
    }

    let source_base = base_record(original, &mut budget)?;
    let copied_base = base_record(copied, &mut budget)?;
    let source_base_id = if let Some(source) = &source_manual {
        let id = reference_id(&source.original.entity, ORIGINAL_KIND)?;
        if source.original.entity != workforce_baseline_reference(id, false)? {
            return Err(contract("baseline_reference_identity"));
        }
        Some(id)
    } else {
        source_base.map(|(id, _)| id)
    };

    let (mapped_base_id, base_update) = match (source_base, copied_base) {
        (None, None) if source_base_id.is_none() => (None, None),
        (Some((source_id, source_raw)), Some((copied_id, copied_raw))) => {
            if source_base_id != Some(source_id) {
                return Err(contract("baseline_original_identity"));
            }
            let mapped_id =
                EntityId::from_uuid(mapped_uuid(source_id.as_uuid(), id_remap, is_copy)?);
            if copied_id != mapped_id {
                return Err(contract("baseline_original_identity"));
            }
            let (source_solution, source_revision) = base_schedule_fields(source_raw, source_id)?;
            let (copied_solution, copied_revision) = base_schedule_fields(copied_raw, copied_id)?;
            let check_pair_hash = source_manual
                .as_ref()
                .is_some_and(|source| source.original.source_scenario_id == original.scenario_id);
            let source_pairs = if !is_copy && id_remap.is_empty() {
                let source_raw_pairs = base_schedule_pair_array(source_raw, &mut budget)?;
                let copied_raw_pairs = base_schedule_pair_array(copied_raw, &mut budget)?;
                if source_raw_pairs == copied_raw_pairs {
                    if check_pair_hash {
                        Some(base_schedule_pairs(source_raw, &mut budget)?)
                    } else {
                        None
                    }
                } else {
                    let source_pairs = base_schedule_pairs(source_raw, &mut budget)?;
                    let copied_pairs = base_schedule_pairs(copied_raw, &mut budget)?;
                    if source_pairs != copied_pairs {
                        return Err(contract("baseline_original_content"));
                    }
                    check_pair_hash.then_some(source_pairs)
                }
            } else {
                let source_pairs = base_schedule_pairs(source_raw, &mut budget)?;
                let expected_pairs = remap_pairs(&source_pairs, id_remap, is_copy, &mut budget)?;
                let copied_pairs = base_schedule_pairs(copied_raw, &mut budget)?;
                if expected_pairs != copied_pairs {
                    return Err(contract("baseline_original_content"));
                }
                Some(source_pairs)
            };
            if let Some(source) = &source_manual {
                if source_solution != source.original.accepted_result.solution_id
                    || source_revision.value() != source.original.source_revision
                {
                    return Err(contract("baseline_original_content"));
                }
                if check_pair_hash
                    && pair_content_hash(
                        source_pairs
                            .as_deref()
                            .ok_or_else(|| contract("baseline_original_content"))?,
                        &mut budget,
                    )? != source.original.pair_content_hash
                {
                    return Err(contract("baseline_original_content"));
                }
            }
            let update = (source_solution != copied_solution || source_revision != copied_revision)
                .then(|| {
                    (
                        copied_id,
                        source_raw["sourceSolutionId"].clone(),
                        source_raw["sourceRevision"].clone(),
                    )
                });
            (Some(mapped_id), update)
        }
        _ => return Err(contract("baseline_original_missing")),
    };

    let manual_update = match (source_manual, copied_manual.take()) {
        (None, None) => None,
        (Some(source), Some(mut destination)) => {
            let source_base_id =
                source_base_id.ok_or_else(|| contract("baseline_original_missing"))?;
            let mapped_base_id =
                mapped_base_id.ok_or_else(|| contract("baseline_original_missing"))?;
            let expected_reference = workforce_baseline_reference(mapped_base_id, false)?;
            if destination.original.entity != source.original.entity
                && destination.original.entity != expected_reference
            {
                return Err(contract("baseline_reference_identity"));
            }
            let expected_id =
                EntityId::from_uuid(mapped_uuid(source.id.as_uuid(), id_remap, is_copy)?);
            if destination.id != expected_id {
                return Err(contract("manual_copy_identity"));
            }
            let expected_assignments =
                remap_pairs(&source.assignments, id_remap, false, &mut budget)?;
            if !destination.assignments.is_sorted() {
                budget
                    .sort_work(destination.assignments.len())
                    .map_err(|error| operation_error(&error))?;
                destination.assignments.sort_unstable();
            }
            let rewritten_assignments = if id_remap == reference_remap {
                None
            } else {
                Some(remap_pairs(
                    &source.assignments,
                    reference_remap,
                    false,
                    &mut budget,
                )?)
            };
            if destination.assignments
                != *rewritten_assignments
                    .as_ref()
                    .unwrap_or(&expected_assignments)
            {
                return Err(contract("manual_copy_pairs"));
            }
            let local_remap = source.id != destination.id
                || source_base_id != mapped_base_id
                || source.assignments != expected_assignments;
            destination.assignments = expected_assignments;
            destination.original = source.original;
            destination.original.entity = expected_reference;
            destination.admission_source_scenario_id = source.admission_source_scenario_id;
            destination.admission_source_revision = source.admission_source_revision;
            destination.admission_source_document_hash = source.admission_source_document_hash;
            destination.admission_command_id = source.admission_command_id;
            if local_remap {
                destination.checksum = manual_checksum(&destination, &mut budget)?;
            } else {
                destination.checksum = source.checksum;
            }
            validate_manual_selection_shape(&destination, &mut budget)?;
            let id = destination.id;
            let raw = if local_remap {
                Some(
                    serde_json::to_value(WorkforceEntity::ManualSelection(destination))
                        .map_err(|_| contract("manual_copy_record"))?,
                )
            } else {
                let raw = original
                    .domain
                    .entities
                    .get(&id)
                    .ok_or_else(|| contract("manual_copy_record"))?;
                if copied.domain.entities.get(&id) == Some(raw) {
                    None
                } else {
                    preflight(raw, &mut budget, ContractJsonLimits::DEFAULT)
                        .map_err(|error| operation_error(&error))?;
                    Some(raw.clone())
                }
            };
            raw.map(|raw| (id, raw))
        }
        _ => return Err(contract("manual_copy_record")),
    };

    budget.check().map_err(|error| operation_error(&error))?;
    if let Some((id, source_solution, source_revision)) = base_update {
        let record = copied
            .domain
            .entities
            .get_mut(&id)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| contract("baseline_original_missing"))?;
        *record
            .get_mut("sourceSolutionId")
            .ok_or_else(|| contract("baseline_original_missing"))? = source_solution;
        *record
            .get_mut("sourceRevision")
            .ok_or_else(|| contract("baseline_original_missing"))? = source_revision;
    }
    if let Some((id, raw)) = manual_update {
        copied.domain.entities.insert(id, raw);
    }
    Ok(())
}

fn mapped_uuid(
    id: Uuid,
    id_remap: &BTreeMap<Uuid, Uuid>,
    require_mapping: bool,
) -> Result<Uuid, DomainPackError> {
    id_remap
        .get(&id)
        .copied()
        .or_else(|| (!require_mapping).then_some(id))
        .ok_or_else(|| contract("manual_copy_identity"))
}

fn base_schedule_fields(
    record: &Value,
    key: EntityId,
) -> Result<(SolutionId, Revision), DomainPackError> {
    if record.get("kind").and_then(Value::as_str) != Some("baseSchedule")
        || record
            .get("id")
            .and_then(Value::as_str)
            .and_then(|value| EntityId::from_str(value).ok())
            != Some(key)
    {
        return Err(contract("baseline_original_identity"));
    }
    let solution = record
        .get("sourceSolutionId")
        .and_then(Value::as_str)
        .and_then(|value| SolutionId::from_str(value).ok())
        .ok_or_else(|| contract("baseline_original_content"))?;
    let revision = record
        .get("sourceRevision")
        .and_then(Value::as_u64)
        .and_then(|value| Revision::try_new(value).ok())
        .ok_or_else(|| contract("baseline_original_content"))?;
    Ok((solution, revision))
}

fn base_schedule_pair_array<'a>(
    record: &'a Value,
    budget: &mut OperationBudget<'_>,
) -> Result<&'a [Value], DomainPackError> {
    let pairs = record
        .get("assignments")
        .and_then(Value::as_array)
        .ok_or_else(|| contract("baseline_original_content"))?;
    within(
        count(pairs.len()).map_err(|error| operation_error(&error))?,
        MAX_SELECTED_PAIRS,
        AssignmentRuleLimit::SelectedPairs,
    )
    .map_err(|error| operation_error(&error))?;
    budget
        .steps(count(pairs.len()).map_err(|error| operation_error(&error))?)
        .map_err(|error| operation_error(&error))?;
    Ok(pairs)
}

fn base_schedule_pairs(
    record: &Value,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentPair>, DomainPackError> {
    let raw_pairs = base_schedule_pair_array(record, budget)?;
    budget
        .reserve(
            1,
            count(raw_pairs.len()).map_err(|error| operation_error(&error))?,
            count(
                raw_pairs
                    .len()
                    .checked_mul(size_of::<AssignmentPair>())
                    .ok_or_else(|| contract("arithmetic"))?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let mut pairs = Vec::with_capacity(raw_pairs.len());
    for raw_pair in raw_pairs {
        budget.step().map_err(|error| operation_error(&error))?;
        pairs.push(
            AssignmentPair::deserialize(raw_pair)
                .map_err(|_| contract("baseline_original_content"))?,
        );
    }
    if !pairs.is_sorted() {
        budget
            .sort_work(pairs.len())
            .map_err(|error| operation_error(&error))?;
        pairs.sort_unstable();
    }
    if pairs.windows(2).any(|window| window[0] == window[1]) {
        return Err(contract("duplicate_baseline_pair"));
    }
    Ok(pairs)
}

fn remap_pairs(
    pairs: &[AssignmentPair],
    id_remap: &BTreeMap<Uuid, Uuid>,
    require_mapping: bool,
    budget: &mut OperationBudget<'_>,
) -> Result<Vec<AssignmentPair>, DomainPackError> {
    budget
        .reserve(
            1,
            count(pairs.len()).map_err(|error| operation_error(&error))?,
            count(
                pairs
                    .len()
                    .checked_mul(size_of::<AssignmentPair>())
                    .ok_or_else(|| contract("arithmetic"))?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let mut mapped = Vec::with_capacity(pairs.len());
    for pair in pairs {
        budget.step().map_err(|error| operation_error(&error))?;
        let person = mapped_uuid(pair.person_id.as_uuid(), id_remap, require_mapping)?;
        let shift = mapped_uuid(
            pair.shift_id.as_entity_id().as_uuid(),
            id_remap,
            require_mapping,
        )?;
        if person.get_version_num() != 7 {
            return Err(contract("manual_copy_pairs"));
        }
        let shift_id = ShiftId::try_from(EntityId::from_uuid(shift))
            .map_err(|_| contract("manual_copy_pairs"))?;
        mapped.push(AssignmentPair {
            person_id: PersonId::from_uuid(person),
            shift_id,
        });
    }
    if !mapped.is_sorted() {
        budget
            .sort_work(mapped.len())
            .map_err(|error| operation_error(&error))?;
        mapped.sort_unstable();
    }
    if mapped.windows(2).any(|window| window[0] == window[1]) {
        return Err(contract("duplicate_baseline_pair"));
    }
    Ok(mapped)
}

fn manual_copy_record(
    document: &ScenarioDocument,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<ManualSelection>, DomainPackError> {
    let mut found = None;
    for (id, record) in &document.domain.entities {
        budget.step().map_err(|error| operation_error(&error))?;
        if record.get("kind").and_then(Value::as_str) != Some("manualSelection") {
            continue;
        }
        if found.is_some() {
            return Err(contract("multiple_manual_selections"));
        }
        preflight(record, budget, ContractJsonLimits::DEFAULT)
            .map_err(|error| operation_error(&error))?;
        let WorkforceEntity::ManualSelection(manual) =
            WorkforceEntity::deserialize(record).map_err(|_| contract("manual_copy_record"))?
        else {
            return Err(contract("manual_copy_record"));
        };
        if manual.id != *id {
            return Err(contract("manual_copy_identity"));
        }
        found = Some(manual);
    }
    Ok(found)
}

/// Resolves exact baseline pairs from immutable objective data, never current history.
///
/// # Errors
/// Rejects any original/manual identity, source, checksum or canonical content mismatch.
pub fn workforce_baseline_pairs<'a>(
    domain: &'a WorkforceDomainV1,
    scenario_id: ScenarioId,
    binding: &RunBaselineBinding,
    control: Option<&OperationControl>,
) -> Result<Cow<'a, [AssignmentPair]>, DomainPackError> {
    standalone_baseline_pairs(
        domain,
        scenario_id,
        binding,
        &mut OperationBudget::evaluation(control),
    )
}

pub(crate) fn standalone_baseline_pairs<'a>(
    domain: &'a WorkforceDomainV1,
    scenario_id: ScenarioId,
    binding: &RunBaselineBinding,
    budget: &mut OperationBudget<'_>,
) -> Result<Cow<'a, [AssignmentPair]>, DomainPackError> {
    let captured = decode_captured_original(binding, scenario_id, budget)?;
    match captured {
        Some(base) => Ok(Cow::Owned(base.assignments)),
        None => baseline_pairs(domain, None, scenario_id, binding, budget).map(Cow::Borrowed),
    }
}

pub(crate) fn decode_captured_original(
    binding: &RunBaselineBinding,
    scenario_id: ScenarioId,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<BaseSchedule>, DomainPackError> {
    let Some(raw) = binding.captured_original.as_deref() else {
        return Ok(None);
    };
    if raw.get("kind").and_then(Value::as_str) != Some("baseSchedule") {
        return Err(contract("captured_original_payload"));
    }
    preflight(
        raw,
        budget,
        ContractJsonLimits {
            max_serialized_bytes: MAX_CAPTURED_BASELINE_BYTES,
            ..ContractJsonLimits::DEFAULT
        },
    )
    .map_err(|error| operation_error(&error))?;
    binding
        .validate_for(scenario_id)
        .map_err(|_| contract("baseline_binding"))?;
    let pairs = raw
        .get("assignments")
        .and_then(Value::as_array)
        .ok_or_else(|| contract("captured_original_payload"))?;
    within(
        count(pairs.len()).map_err(|error| operation_error(&error))?,
        MAX_SELECTED_PAIRS,
        AssignmentRuleLimit::SelectedPairs,
    )
    .map_err(|error| operation_error(&error))?;
    budget
        .reserve(
            1,
            count(pairs.len()).map_err(|error| operation_error(&error))?,
            count(
                size_of::<BaseSchedule>()
                    .checked_add(
                        pairs
                            .len()
                            .checked_mul(size_of::<AssignmentPair>())
                            .ok_or_else(|| contract("arithmetic"))?,
                    )
                    .ok_or_else(|| contract("arithmetic"))?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let WorkforceEntity::BaseSchedule(base) =
        WorkforceEntity::deserialize(raw).map_err(|_| contract("captured_original_payload"))?
    else {
        return Err(contract("captured_original_payload"));
    };
    for window in base.assignments.windows(2) {
        budget.step().map_err(|error| operation_error(&error))?;
        if window[0] >= window[1] {
            return Err(contract("captured_original_pair_order"));
        }
    }
    if base.id != EntityId::from_uuid(binding.original.accepted_result.solution_id.as_uuid())
        || binding.original.entity != workforce_baseline_reference(base.id, false)?
    {
        return Err(contract("captured_original_identity"));
    }
    validate_original(&base, binding, budget)?;
    Ok(Some(base))
}

fn validate_original(
    base: &BaseSchedule,
    binding: &RunBaselineBinding,
    budget: &mut OperationBudget<'_>,
) -> Result<(), DomainPackError> {
    if base.id != reference_id(&binding.original.entity, ORIGINAL_KIND)?
        || base.source_solution_id != binding.original.accepted_result.solution_id
        || base.source_revision.value() != binding.original.source_revision
        || pair_content_hash(&base.assignments, budget)? != binding.original.pair_content_hash
    {
        return Err(contract("baseline_original_content"));
    }
    Ok(())
}

pub(crate) fn baseline_pairs<'a>(
    domain: &'a WorkforceDomainV1,
    captured: Option<&'a BaseSchedule>,
    scenario_id: ScenarioId,
    binding: &RunBaselineBinding,
    budget: &mut OperationBudget<'_>,
) -> Result<&'a [AssignmentPair], DomainPackError> {
    baseline_sources(domain, captured, scenario_id, binding, budget).map(|(_, selected)| selected)
}

/// Resolves recorded A and the selected A/B census together without validating manual B twice.
/// These source facts do not authenticate the recorded original against native accepted custody.
pub(crate) fn baseline_sources<'a>(
    domain: &'a WorkforceDomainV1,
    captured: Option<&'a BaseSchedule>,
    scenario_id: ScenarioId,
    binding: &RunBaselineBinding,
    budget: &mut OperationBudget<'_>,
) -> Result<(&'a BaseSchedule, &'a [AssignmentPair]), DomainPackError> {
    let base = match (binding.captured_original.is_some(), captured) {
        // This operation-owned record was fully checked by decode_captured_original.
        // Do not reparse or rehash its complete census for every consumer.
        (true, Some(base)) => {
            if !matches!(binding.selection, RunBaselineSelection::OriginalAccepted {}) {
                return Err(contract("baseline_binding"));
            }
            return Ok((base, &base.assignments));
        }
        (false, None) => {
            binding
                .validate_for(scenario_id)
                .map_err(|_| contract("baseline_binding"))?;
            let original_id = reference_id(&binding.original.entity, ORIGINAL_KIND)?;
            let Some(WorkforceEntity::BaseSchedule(base)) = domain.entities.get(&original_id)
            else {
                return Err(contract("baseline_original_missing"));
            };
            validate_original(base, binding, budget)?;
            base
        }
        _ => return Err(contract("baseline_capture_presence")),
    };
    match &binding.selection {
        RunBaselineSelection::OriginalAccepted {} => Ok((base, &base.assignments)),
        RunBaselineSelection::ManualSelection {
            entity,
            record_checksum,
            admission_source_scenario_id,
            admission_source_revision,
            admission_source_document_hash,
        } => {
            let id = reference_id(entity, MANUAL_KIND)?;
            let Some(WorkforceEntity::ManualSelection(record)) = domain.entities.get(&id) else {
                return Err(contract("baseline_manual_missing"));
            };
            validate_manual_selection_record(record, budget)?;
            if record.id != id
                || record.original != binding.original
                || record.checksum != *record_checksum
                || record.admission_source_scenario_id != *admission_source_scenario_id
                || record.admission_source_revision.value() != *admission_source_revision
                || record.admission_source_document_hash != *admission_source_document_hash
                || pair_content_hash(&record.assignments, budget)?
                    != binding.selected_pair_content_hash
            {
                return Err(contract("baseline_manual_content"));
            }
            Ok((base, &record.assignments))
        }
    }
}

pub(crate) fn selected_baseline_pairs<'a>(
    domain: &'a WorkforceDomainV1,
    captured: Option<&'a BaseSchedule>,
    scenario_id: ScenarioId,
    context: Option<&RunObjectiveContext>,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<&'a [AssignmentPair]>, DomainPackError> {
    let Some(context) = context else {
        return Ok(None);
    };
    match (
        context.baseline.as_deref(),
        context.external_baseline.as_deref(),
    ) {
        (Some(binding), None) => {
            baseline_pairs(domain, captured, scenario_id, binding, budget).map(Some)
        }
        (None, Some(binding)) if captured.is_none() => {
            external_baseline_pairs(domain, binding, budget).map(Some)
        }
        (None, None) if captured.is_none() => Ok(None),
        _ => Err(contract("baseline_source")),
    }
}

fn external_baseline_pairs<'a>(
    domain: &'a WorkforceDomainV1,
    binding: &RunExternalBaselineBinding,
    budget: &mut OperationBudget<'_>,
) -> Result<&'a [AssignmentPair], DomainPackError> {
    let id = reference_id(&binding.entity, EXTERNAL_KIND)?;
    let Some(WorkforceEntity::ExternalStartingPlan(record)) = domain.entities.get(&id) else {
        return Err(contract("external_baseline_missing"));
    };
    if record.id != id
        || record.schema_version != 1
        || pair_content_hash(&record.assignments, budget)? != binding.selected_pair_content_hash
    {
        return Err(contract("external_baseline_content"));
    }
    Ok(&record.assignments)
}

fn reference_id(reference: &DomainEntityRef, kind: &str) -> Result<EntityId, DomainPackError> {
    if reference.kind.as_str() != kind {
        return Err(contract("baseline_reference_kind"));
    }
    let value = reference
        .id
        .as_str()
        .strip_prefix(ENTITY_PREFIX)
        .ok_or_else(|| contract("baseline_reference_identity"))?;
    EntityId::from_str(value).map_err(|_| contract("baseline_reference_identity"))
}

/// Clears retained manual data only after a whole operation effectively changes its original base.
///
/// The returned identity/raw record is the exact removed protected side for private journaling.
/// Calling code owns final combined changes, validation, bounds and complete history inverse.
/// Trusted history restoration must not call this automatic-clear path.
///
/// # Errors
/// Rejects malformed/multiple base or manual records and existing controlled work/input limits.
pub fn clear_workforce_manual_after_base_change(
    original: &ScenarioDocument,
    prospective: &mut ScenarioDocument,
    control: Option<&OperationControl>,
) -> Result<Option<(EntityId, Value)>, DomainPackError> {
    let mut budget = OperationBudget::evaluation(control);
    let mut manual_id = None;
    for (id, record) in &prospective.domain.entities {
        budget.step().map_err(|error| operation_error(&error))?;
        if record.get("kind").and_then(Value::as_str) == Some("manualSelection") {
            preflight(record, &mut budget, ContractJsonLimits::DEFAULT)
                .map_err(|error| operation_error(&error))?;
            if manual_id.replace(*id).is_some() {
                return Err(contract("multiple_manual_selections"));
            }
        }
    }
    let Some(manual_id) = manual_id else {
        return Ok(None);
    };
    if !effective_base_changed(original, prospective, &mut budget)? {
        return Ok(None);
    }
    budget.check().map_err(|error| operation_error(&error))?;
    let removed = prospective
        .domain
        .entities
        .remove(&manual_id)
        .ok_or_else(|| contract("manual_selection_missing"))?;
    Ok(Some((manual_id, removed)))
}

pub(super) fn effective_base_changed(
    original: &ScenarioDocument,
    prospective: &ScenarioDocument,
    budget: &mut OperationBudget<'_>,
) -> Result<bool, DomainPackError> {
    let before = base_record(original, budget)?;
    let after = base_record(prospective, budget)?;
    Ok(before != after && effective_base(before, budget)? != effective_base(after, budget)?)
}

fn base_record<'a>(
    document: &'a ScenarioDocument,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<(EntityId, &'a Value)>, DomainPackError> {
    let mut found = None;
    for (id, record) in &document.domain.entities {
        budget.step().map_err(|error| operation_error(&error))?;
        if record.get("kind").and_then(Value::as_str) == Some("baseSchedule") {
            preflight(record, budget, ContractJsonLimits::DEFAULT)
                .map_err(|error| operation_error(&error))?;
            if found.replace((*id, record)).is_some() {
                return Err(contract("multiple_base_schedules"));
            }
        }
    }
    Ok(found)
}

fn effective_base(
    record: Option<(EntityId, &Value)>,
    budget: &mut OperationBudget<'_>,
) -> Result<Option<(EntityId, SolutionId, Revision, String)>, DomainPackError> {
    let Some((id, raw)) = record else {
        return Ok(None);
    };
    let pairs = raw
        .get("assignments")
        .and_then(Value::as_array)
        .ok_or_else(|| contract("baseline_original_content"))?;
    budget
        .reserve(
            1,
            count(pairs.len()).map_err(|error| operation_error(&error))?,
            count(
                pairs
                    .len()
                    .checked_mul(size_of::<AssignmentPair>())
                    .ok_or_else(|| contract("arithmetic"))?,
            )
            .map_err(|error| operation_error(&error))?,
        )
        .map_err(|error| operation_error(&error))?;
    let WorkforceEntity::BaseSchedule(base) =
        WorkforceEntity::deserialize(raw).map_err(|_| contract("baseline_original_content"))?
    else {
        return Err(contract("baseline_original_content"));
    };
    if base.id != id {
        return Err(contract("baseline_original_identity"));
    }
    Ok(Some((
        base.id,
        base.source_solution_id,
        base.source_revision,
        pair_content_hash(&base.assignments, budget)?,
    )))
}
