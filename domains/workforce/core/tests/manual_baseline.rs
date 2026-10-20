mod support;

use eutheto_domain_api::{DomainPack, DomainSetupQueryV1, DomainViewInput, SetupViewContext};
use eutheto_domain_ir::{
    AcceptedResultRefV1, RunBaselineBinding, RunBaselineOriginal, RunBaselineSelection,
};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, OperationControl, Revision, ScenarioDocument,
};
use eutheto_workforce::{
    WorkforcePack,
    assignment_rules::{
        clear_workforce_manual_after_base_change, workforce_baseline_pairs,
        workforce_baseline_reference, workforce_manual_selection_checksum,
        workforce_pair_content_hash,
    },
    commands,
    model::{AssignmentPair, ManualSelection, WorkforceEntity},
    validation::validate_document,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error};
use uuid::Uuid;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn pair(person: u32, shift: u32) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: support::id(person).parse()?,
        shift_id: support::id(shift).parse()?,
    })
}

fn remapped_pair(pair: AssignmentPair, id_remap: &BTreeMap<Uuid, Uuid>) -> Result<AssignmentPair> {
    Ok(AssignmentPair {
        person_id: id_remap
            .get(&pair.person_id.as_uuid())
            .copied()
            .unwrap_or(pair.person_id.as_uuid())
            .to_string()
            .parse()?,
        shift_id: id_remap
            .get(&pair.shift_id.as_entity_id().as_uuid())
            .copied()
            .unwrap_or(pair.shift_id.as_entity_id().as_uuid())
            .to_string()
            .parse()?,
    })
}

// These synthetic snapshot facts establish content integrity, never local accepted/admission custody.
fn snapshot() -> Result<(ScenarioDocument, ManualSelection)> {
    let mut document = support::fixture()?;
    let original_pairs = vec![pair(1, 7)?, pair(1, 8)?];
    document.domain.entities.insert(
        support::id(20).parse()?,
        json!({
            "kind":"baseSchedule", "id":support::id(20), "sourceSolutionId":support::id(21),
            "sourceRevision":0, "assignments":original_pairs,
        }),
    );
    let mut manual = ManualSelection {
        schema_version: 1,
        id: support::id(22).parse()?,
        original: RunBaselineOriginal {
            entity: workforce_baseline_reference(support::id(20).parse()?, false)?,
            pair_content_hash: workforce_pair_content_hash(&original_pairs, None)?,
            accepted_result: AcceptedResultRefV1 {
                solution_id: support::id(21).parse()?,
                result_checksum: "a".repeat(64),
            },
            source_scenario_id: document.scenario_id,
            source_revision: 0,
            source_document_hash: "b".repeat(64),
            source_run_id: support::id(23).parse()?,
            source_run_input_checksum: "c".repeat(64),
            source_run_manifest_checksum: "d".repeat(64),
        },
        admission_source_scenario_id: document.scenario_id,
        admission_source_revision: Revision::INITIAL,
        admission_source_document_hash: "e".repeat(64),
        admission_command_id: support::id(24).parse()?,
        assignments: vec![],
        checksum: String::new(),
    };
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    document.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    Ok((document, manual))
}

fn binding(manual: &ManualSelection) -> Result<RunBaselineBinding> {
    Ok(RunBaselineBinding {
        captured_original: None,
        original: manual.original.clone(),
        selection: RunBaselineSelection::ManualSelection {
            entity: workforce_baseline_reference(manual.id, true)?,
            record_checksum: manual.checksum.clone(),
            admission_source_scenario_id: manual.admission_source_scenario_id,
            admission_source_revision: manual.admission_source_revision.value(),
            admission_source_document_hash: manual.admission_source_document_hash.clone(),
        },
        selected_pair_content_hash: workforce_pair_content_hash(&manual.assignments, None)?,
    })
}

fn remapped_manual_copy(
    original: &ScenarioDocument,
    manual: &ManualSelection,
) -> Result<(ScenarioDocument, ManualSelection, BTreeMap<Uuid, Uuid>)> {
    let id_remap = BTreeMap::from([
        (support::id(100).parse()?, support::id(31).parse()?),
        (support::id(1).parse()?, support::id(32).parse()?),
        (support::id(7).parse()?, support::id(37).parse()?),
        (support::id(8).parse()?, support::id(38).parse()?),
        (support::id(14).parse()?, support::id(34).parse()?),
        (support::id(20).parse()?, support::id(40).parse()?),
        (support::id(22).parse()?, support::id(30).parse()?),
    ]);
    let mut copied = original.clone();
    copied.scenario_id = support::id(31).parse()?;
    let mut person = copied
        .domain
        .entities
        .remove(&support::id(1).parse()?)
        .ok_or("person")?;
    person["id"] = json!(support::id(32));
    copied
        .domain
        .entities
        .insert(support::id(32).parse()?, person);
    let template = copied
        .domain
        .entities
        .get_mut(&support::id(6).parse()?)
        .ok_or("template")?;
    let occurrences = template["occurrenceIdentities"]
        .as_object_mut()
        .ok_or("occurrences")?;
    let mut occurrence = occurrences.remove(&support::id(7)).ok_or("occurrence")?;
    occurrence["id"] = json!(support::id(37));
    occurrences.insert(support::id(37), occurrence);
    let mut shift = copied
        .domain
        .entities
        .remove(&support::id(8).parse()?)
        .ok_or("shift")?;
    shift["id"] = json!(support::id(38));
    copied
        .domain
        .entities
        .insert(support::id(38).parse()?, shift);
    let mut base = copied
        .domain
        .entities
        .remove(&support::id(20).parse()?)
        .ok_or("base")?;
    base["id"] = json!(support::id(40));
    base["sourceSolutionId"] = json!(support::id(25));
    base["sourceRevision"] = json!(77);
    base["assignments"] = json!([pair(32, 37)?, pair(32, 38)?]);
    copied
        .domain
        .entities
        .insert(support::id(40).parse()?, base);
    copied.domain.entities.remove(&manual.id).ok_or("manual")?;
    let mut transformed = manual.clone();
    transformed.id = support::id(30).parse()?;
    transformed.original.source_scenario_id = copied.scenario_id;
    transformed.original.source_revision = 9;
    transformed.original.accepted_result.solution_id = support::id(25).parse()?;
    transformed.admission_source_scenario_id = copied.scenario_id;
    transformed.assignments = manual
        .assignments
        .iter()
        .copied()
        .map(|pair| remapped_pair(pair, &id_remap))
        .collect::<Result<Vec<_>>>()?;
    copied.domain.entities.insert(
        transformed.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(transformed.clone()))?,
    );
    let mut lock = copied
        .domain
        .locked_assignments
        .remove(&support::id(14).parse()?)
        .ok_or("lock")?;
    lock["id"] = json!(support::id(34));
    lock["personId"] = json!(support::id(32));
    lock["shiftId"] = json!(support::id(37));
    copied
        .domain
        .locked_assignments
        .insert(support::id(34).parse()?, lock);
    Ok((copied, transformed, id_remap))
}

#[test]
fn native_admission_hashes_and_required_checks_share_exhausted_work() -> Result {
    let (document, manual) = snapshot()?;
    let mut budget = eutheto_workforce::assignment_rules::WorkforceAdmissionBudget::new(None);
    budget.steps(eutheto_workforce::assignment_rules::MAX_WORK_STEPS - 1)?;
    assert!(matches!(
        budget.pair_content_hash(&[pair(1, 7)?]),
        Err(eutheto_domain_api::DomainPackError::ResourceLimitExceeded),
    ));
    assert!(matches!(
        budget.manual_selection_checksum(&manual),
        Err(eutheto_domain_api::DomainPackError::ResourceLimitExceeded),
    ));
    assert!(matches!(
        budget.evaluate_required_selection(&document, &[]),
        Err(eutheto_domain_api::DomainPackError::ResourceLimitExceeded),
    ));
    Ok(())
}

#[test]
fn frozen_manual_is_explicit_and_bound_to_complete_content() -> Result {
    let (mut document, mut manual) = snapshot()?;
    let captured = binding(&manual)?;
    let domain = validate_document(&document)?;
    assert_eq!(
        workforce_baseline_pairs(&domain, document.scenario_id, &captured, None)?.as_ref(),
        &[]
    );
    let original = RunBaselineBinding {
        captured_original: None,
        original: manual.original.clone(),
        selection: RunBaselineSelection::OriginalAccepted {},
        selected_pair_content_hash: manual.original.pair_content_hash.clone(),
    };
    assert_eq!(
        workforce_baseline_pairs(&domain, document.scenario_id, &original, None)?.as_ref(),
        &[pair(1, 7)?, pair(1, 8)?]
    );
    assert!(workforce_baseline_pairs(&domain, support::id(999).parse()?, &captured, None).is_err());
    manual.assignments = vec![pair(1, 8)?];
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    document.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let changed = validate_document(&document)?;
    assert!(workforce_baseline_pairs(&changed, document.scenario_id, &captured, None).is_err());
    assert_eq!(
        workforce_baseline_pairs(&changed, document.scenario_id, &binding(&manual)?, None)?
            .as_ref(),
        &[pair(1, 8)?]
    );
    document.domain.entities.remove(&manual.id);
    let absent = validate_document(&document)?;
    assert!(workforce_baseline_pairs(&absent, document.scenario_id, &captured, None).is_err());
    Ok(())
}

#[test]
fn canonical_hash_rejects_duplicates_and_manual_structural_validation_preserves_old_distance()
-> Result {
    let pairs = [pair(1, 7)?, pair(1, 8)?];
    assert_eq!(
        workforce_pair_content_hash(&pairs, None)?,
        workforce_pair_content_hash(&[pairs[1], pairs[0]], None)?
    );
    assert!(workforce_pair_content_hash(&[pairs[0], pairs[0]], None).is_err());
    let (mut document, mut manual) = snapshot()?;
    // A previously admitted pair may cease to exist after genuine source edits. Shape is not re-admission.
    manual.assignments = vec![pair(999, 999)?];
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    document.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let domain = validate_document(&document)?;
    assert_eq!(
        workforce_baseline_pairs(&domain, document.scenario_id, &binding(&manual)?, None)?.as_ref(),
        &manual.assignments
    );
    manual.assignments.push(manual.assignments[0]);
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    document.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual))?,
    );
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn copied_manual_rebinds_local_base_and_pairs_but_preserves_foreign_ancestry() -> Result {
    let (mut original, mut manual) = snapshot()?;
    manual.assignments = vec![pair(1, 7)?, pair(1, 8)?];
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    original.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let (mut copied, transformed, id_remap) = remapped_manual_copy(&original, &manual)?;
    assert!(validate_document(&copied).is_err());
    eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
        &original,
        &mut copied,
        &id_remap,
        &id_remap,
        None,
    )?;
    let domain = validate_document(&copied)?;
    let WorkforceEntity::ManualSelection(retained) = domain
        .entities
        .get(&transformed.id)
        .ok_or("copied manual")?
    else {
        return Err("manual kind".into());
    };
    let mut expected_original = manual.original.clone();
    expected_original.entity = workforce_baseline_reference(support::id(40).parse()?, false)?;
    assert_eq!(retained.original, expected_original);
    assert_eq!(retained.admission_source_scenario_id, original.scenario_id);
    assert_eq!(
        retained.admission_source_revision,
        manual.admission_source_revision
    );
    assert_eq!(
        retained.admission_source_document_hash,
        manual.admission_source_document_hash
    );
    assert_eq!(retained.admission_command_id, manual.admission_command_id);
    assert_eq!(retained.assignments, vec![pair(32, 37)?, pair(32, 38)?]);
    assert_eq!(
        retained.checksum,
        workforce_manual_selection_checksum(retained, None)?
    );
    assert!(
        workforce_baseline_pairs(&domain, copied.scenario_id, &binding(retained)?, None).is_err()
    );

    let WorkforceEntity::BaseSchedule(base) = domain
        .entities
        .get(&support::id(40).parse()?)
        .ok_or("copied base")?
    else {
        return Err("base kind".into());
    };
    assert_eq!(base.id, support::id(40).parse()?);
    assert_eq!(
        base.source_solution_id,
        manual.original.accepted_result.solution_id
    );
    assert_eq!(base.source_revision, Revision::INITIAL);
    assert_eq!(base.assignments, vec![pair(32, 37)?, pair(32, 38)?]);

    Ok(())
}

#[test]
fn copied_manual_keeps_unmapped_historical_pair_identities() -> Result {
    let (mut original, mut manual) = snapshot()?;
    manual.assignments = vec![pair(1, 7)?, pair(1, 8)?, pair(50, 60)?];
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    original.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let (mut copied, transformed, id_remap) = remapped_manual_copy(&original, &manual)?;
    let mut reference_remap = id_remap.clone();
    reference_remap.insert(support::id(50).parse()?, support::id(51).parse()?);
    reference_remap.insert(support::id(60).parse()?, support::id(61).parse()?);
    let rewritten_assignments = manual
        .assignments
        .iter()
        .copied()
        .map(|pair| remapped_pair(pair, &reference_remap))
        .collect::<Result<Vec<_>>>()?;
    copied
        .domain
        .entities
        .get_mut(&transformed.id)
        .ok_or("copied manual")?["assignments"] = serde_json::to_value(rewritten_assignments)?;
    eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
        &original,
        &mut copied,
        &id_remap,
        &reference_remap,
        None,
    )?;
    let domain = validate_document(&copied)?;
    let WorkforceEntity::ManualSelection(retained) =
        domain.entities.get(&transformed.id).ok_or("manual")?
    else {
        return Err("manual kind".into());
    };
    assert_eq!(
        retained.assignments,
        vec![pair(32, 37)?, pair(32, 38)?, pair(50, 60)?]
    );
    assert_eq!(
        retained.checksum,
        workforce_manual_selection_checksum(retained, None)?
    );
    Ok(())
}

#[test]
fn copied_manual_rejects_mismatched_copy_identities_atomically() -> Result {
    let (mut original, mut manual) = snapshot()?;
    manual.assignments = vec![pair(1, 7)?, pair(1, 8)?];
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    original.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let (copied, transformed, id_remap) = remapped_manual_copy(&original, &manual)?;

    let mut wrong_scenario = copied.clone();
    wrong_scenario.scenario_id = support::id(35).parse()?;
    let before = wrong_scenario.clone();
    assert!(
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            &original,
            &mut wrong_scenario,
            &id_remap,
            &id_remap,
            None,
        )
        .is_err()
    );
    assert_eq!(wrong_scenario, before);

    let mut wrong_manual_id = copied.clone();
    let mut wrong_manual = wrong_manual_id
        .domain
        .entities
        .remove(&transformed.id)
        .ok_or("manual")?;
    wrong_manual["id"] = json!(support::id(35));
    wrong_manual_id
        .domain
        .entities
        .insert(support::id(35).parse()?, wrong_manual);
    let before = wrong_manual_id.clone();
    assert!(
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            &original,
            &mut wrong_manual_id,
            &id_remap,
            &id_remap,
            None,
        )
        .is_err()
    );
    assert_eq!(wrong_manual_id, before);

    let mut wrong_base = copied.clone();
    let mut base = wrong_base
        .domain
        .entities
        .remove(&support::id(40).parse()?)
        .ok_or("base")?;
    base["id"] = json!(support::id(41));
    wrong_base
        .domain
        .entities
        .insert(support::id(41).parse()?, base);
    let before = wrong_base.clone();
    assert!(
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            &original,
            &mut wrong_base,
            &id_remap,
            &id_remap,
            None,
        )
        .is_err()
    );
    assert_eq!(wrong_base, before);
    Ok(())
}

#[test]
fn copied_manual_rejects_foreign_endpoints_and_pair_mismatches_atomically() -> Result {
    let (mut original, mut manual) = snapshot()?;
    manual.assignments = vec![pair(1, 7)?, pair(1, 8)?];
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    original.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    let (mut copied, transformed, id_remap) = remapped_manual_copy(&original, &manual)?;
    eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
        &original,
        &mut copied,
        &id_remap,
        &id_remap,
        None,
    )?;

    let mut wrong_endpoint = copied.clone();
    wrong_endpoint
        .domain
        .entities
        .get_mut(&transformed.id)
        .ok_or("manual")?["original"]["entity"]["id"] =
        json!(format!("official.workforce.{}", support::id(45)));
    let before = wrong_endpoint.clone();
    assert!(
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            &original,
            &mut wrong_endpoint,
            &id_remap,
            &id_remap,
            None,
        )
        .is_err()
    );
    assert_eq!(wrong_endpoint, before);

    let mut wrong_pairs = copied.clone();
    wrong_pairs
        .domain
        .entities
        .get_mut(&support::id(40).parse()?)
        .ok_or("base")?["assignments"][0]["shiftId"] = json!(support::id(39));
    let before = wrong_pairs.clone();
    assert!(
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            &original,
            &mut wrong_pairs,
            &id_remap,
            &id_remap,
            None,
        )
        .is_err()
    );
    assert_eq!(wrong_pairs, before);

    let before = copied.clone();
    original
        .domain
        .entities
        .get_mut(&manual.id)
        .ok_or("source manual")?["checksum"] = json!("0".repeat(64));
    assert!(
        eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
            &original,
            &mut copied,
            &id_remap,
            &id_remap,
            None,
        )
        .is_err()
    );
    assert_eq!(copied, before);
    Ok(())
}

#[test]
fn same_identity_import_restores_foreign_provenance_without_changing_checksum() -> Result {
    let (mut original, mut manual) = snapshot()?;
    manual.original.source_scenario_id = support::id(99).parse()?;
    manual.admission_source_scenario_id = manual.original.source_scenario_id;
    manual.original.pair_content_hash = workforce_pair_content_hash(&[pair(50, 60)?], None)?;
    manual.checksum = workforce_manual_selection_checksum(&manual, None)?;
    original.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(manual.clone()))?,
    );
    original
        .domain
        .entities
        .get_mut(&manual.id)
        .ok_or("manual")?["admissionCommandId"] =
        json!(manual.admission_command_id.to_string().to_uppercase());
    let mut imported = original.clone();
    let mut transformed = manual.clone();
    transformed.original.source_scenario_id = support::id(31).parse()?;
    transformed.original.source_revision = 9;
    transformed.original.accepted_result.solution_id = support::id(25).parse()?;
    transformed.original.source_document_hash = "f".repeat(64);
    transformed.original.source_run_id = support::id(26).parse()?;
    transformed.original.source_run_input_checksum = "a".repeat(64);
    transformed.original.source_run_manifest_checksum = "b".repeat(64);
    transformed.admission_source_scenario_id = support::id(31).parse()?;
    transformed.admission_source_revision = Revision::new(9);
    transformed.admission_source_document_hash = "c".repeat(64);
    transformed.admission_command_id = support::id(27).parse()?;
    imported.domain.entities.insert(
        manual.id,
        serde_json::to_value(WorkforceEntity::ManualSelection(transformed))?,
    );
    imported
        .domain
        .entities
        .get_mut(&support::id(20).parse()?)
        .ok_or("base")?["sourceSolutionId"] = json!(support::id(25));
    imported
        .domain
        .entities
        .get_mut(&support::id(20).parse()?)
        .ok_or("base")?["sourceRevision"] = json!(77);

    eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
        &original,
        &mut imported,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
    )?;
    assert_eq!(
        imported.domain.entities.get(&manual.id),
        original.domain.entities.get(&manual.id),
    );
    let domain = validate_document(&imported)?;
    let WorkforceEntity::ManualSelection(retained) =
        domain.entities.get(&manual.id).ok_or("manual")?
    else {
        return Err("manual kind".into());
    };
    assert_eq!(retained.original, manual.original);
    assert_eq!(
        retained.admission_source_scenario_id,
        manual.admission_source_scenario_id
    );
    assert_eq!(
        retained.admission_source_revision,
        manual.admission_source_revision
    );
    assert_eq!(
        retained.admission_source_document_hash,
        manual.admission_source_document_hash
    );
    assert_eq!(retained.admission_command_id, manual.admission_command_id);
    assert_eq!(retained.checksum, manual.checksum);
    assert_eq!(
        retained.checksum,
        workforce_manual_selection_checksum(retained, None)?
    );
    let WorkforceEntity::BaseSchedule(base) = domain
        .entities
        .get(&support::id(20).parse()?)
        .ok_or("base")?
    else {
        return Err("base kind".into());
    };
    assert_eq!(
        base.source_solution_id,
        manual.original.accepted_result.solution_id
    );
    assert_eq!(base.source_revision, Revision::INITIAL);
    Ok(())
}

#[test]
fn baseline_source_ancestry_is_restored_without_a_manual_record() -> Result {
    let (mut original, manual) = snapshot()?;
    original
        .domain
        .entities
        .remove(&manual.id)
        .ok_or("manual")?;
    let mut imported = original.clone();
    imported
        .domain
        .entities
        .get_mut(&support::id(20).parse()?)
        .ok_or("base")?["sourceSolutionId"] = json!(support::id(25));
    imported
        .domain
        .entities
        .get_mut(&support::id(20).parse()?)
        .ok_or("base")?["sourceRevision"] = json!(77);
    eutheto_workforce::assignment_rules::normalize_workforce_manual_selection_copy(
        &original,
        &mut imported,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
    )?;
    let domain = validate_document(&imported)?;
    let WorkforceEntity::BaseSchedule(base) = domain
        .entities
        .get(&support::id(20).parse()?)
        .ok_or("base")?
    else {
        return Err("base kind".into());
    };
    assert_eq!(base.source_solution_id, support::id(21).parse()?);
    assert_eq!(base.source_revision, Revision::INITIAL);
    Ok(())
}

#[test]
fn whole_operation_base_effect_preserves_atomic_reverts_but_not_separate_commits() -> Result {
    let (original, manual) = snapshot()?;
    let base_id = support::id(20).parse()?;
    let mut reordered = original.clone();
    reordered.domain.entities.get_mut(&base_id).ok_or("base")?["assignments"] =
        json!([pair(1, 8)?, pair(1, 7)?]);
    assert!(clear_workforce_manual_after_base_change(&original, &mut reordered, None)?.is_none());
    assert_eq!(
        reordered.domain.entities.get(&manual.id),
        original.domain.entities.get(&manual.id)
    );
    let mut prospective = original.clone();
    prospective
        .domain
        .entities
        .get_mut(&base_id)
        .ok_or("base")?["sourceSolutionId"] = json!(support::id(25));
    let reverted = prospective
        .domain
        .entities
        .insert(base_id, original.domain.entities[&base_id].clone());
    assert!(reverted.is_some());
    assert!(clear_workforce_manual_after_base_change(&original, &mut prospective, None)?.is_none());
    prospective
        .domain
        .entities
        .get_mut(&base_id)
        .ok_or("base")?["assignments"] = json!([]);
    let removed = clear_workforce_manual_after_base_change(&original, &mut prospective, None)?
        .ok_or("protected removal")?;
    assert_eq!(
        removed,
        (manual.id, original.domain.entities[&manual.id].clone())
    );
    let committed = prospective.clone();
    prospective
        .domain
        .entities
        .insert(base_id, original.domain.entities[&base_id].clone());
    assert!(
        clear_workforce_manual_after_base_change(&committed, &mut prospective, None)?.is_none()
    );
    assert!(!prospective.domain.entities.contains_key(&manual.id));
    Ok(())
}

fn setup_result(document: &ScenarioDocument, query: &DomainSetupQueryV1) -> Result<Value> {
    let output = WorkforcePack.build_view(
        DomainViewInput::StoredSetup {
            document,
            query,
            context: SetupViewContext {
                revision: Revision::INITIAL,
                query_fingerprint: [7; 32],
            },
        },
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    let decoded: eutheto_workforce::setup::contracts::WorkforceSetupResultV1 =
        serde_json::from_value(output.view.data)?;
    Ok(serde_json::to_value(decoded.result)?)
}

#[test]
fn ordinary_queries_work_and_generic_manual_writes_are_closed() -> Result {
    let (document, manual) = snapshot()?;
    let mut without_manual = document.clone();
    without_manual.domain.entities.remove(&manual.id);
    for (view_id, parameters) in [
        ("official.workforce.setup.overview", json!({})),
        (
            "eutheto.setup.entity_page",
            json!({"entityKind":"person", "search":"", "limit":50}),
        ),
        (
            "eutheto.setup.entity_page",
            json!({"entityKind":"baseSchedule", "search":"", "limit":50}),
        ),
        (
            "official.workforce.setup.people_page",
            json!({"search":"", "limit":50}),
        ),
        (
            "official.workforce.setup.availability_records",
            json!({"personId":support::id(1), "limit":50}),
        ),
        (
            "official.workforce.setup.availability_window",
            json!({
                "personId":support::id(1), "limit":50,
                "dates":{"startDate":"2026-11-01", "endDateExclusive":"2026-11-02"}
            }),
        ),
    ] {
        let query = DomainSetupQueryV1 {
            schema_version: 1,
            view_id: view_id.to_owned(),
            parameters,
            continuation: None,
        };
        let result = setup_result(&document, &query)?;
        assert_eq!(result, setup_result(&without_manual, &query)?);
        // Protected snapshot data must never be exposed as an editable ordinary record.
        assert!(!serde_json::to_string(&result)?.contains("admissionCommandId"));
    }
    let protected = document.domain.entities[&manual.id].clone();
    let mut replacement = document.domain.entities[&support::id(1).parse()?].clone();
    replacement["id"] = json!(manual.id);
    for (command_type, payload) in [
        (commands::ADD_ENTITY, json!({"entity":protected})),
        (commands::UPDATE_ENTITY, json!({"entity":protected})),
        (commands::UPDATE_ENTITY, json!({"entity":replacement})),
        (commands::REMOVE_ENTITY, json!({"entityId":manual.id})),
    ] {
        let batch = eutheto_domain_api::DomainBatchCommand {
            schema_version: 1,
            pack_id: document.domain_pack.id.clone(),
            scenario_schema_version: 1,
            label: None,
            commands: vec![DomainCommandEnvelope {
                command_type: command_type.to_owned(),
                payload,
            }],
        };
        assert!(commands::apply_batch(&document, &batch).is_err());
    }
    assert_eq!(document.domain.entities.get(&manual.id), Some(&protected));
    Ok(())
}

fn captured_original(
    assignments: &[AssignmentPair],
) -> Result<(ScenarioDocument, RunBaselineBinding)> {
    let (mut document, manual) = snapshot()?;
    document.domain.entities.remove(&support::id(20).parse()?);
    document.domain.entities.remove(&manual.id);
    let mut original = manual.original;
    let id = eutheto_types::EntityId::from_uuid(original.accepted_result.solution_id.as_uuid());
    original.entity = workforce_baseline_reference(id, false)?;
    original.pair_content_hash = workforce_pair_content_hash(assignments, None)?;
    let captured = json!({
        "kind":"baseSchedule", "id":id,
        "sourceSolutionId":original.accepted_result.solution_id,
        "sourceRevision":original.source_revision, "assignments":assignments,
    });
    Ok((
        document,
        RunBaselineBinding {
            selected_pair_content_hash: original.pair_content_hash.clone(),
            original,
            selection: RunBaselineSelection::OriginalAccepted {},
            captured_original: Some(std::sync::Arc::new(captured)),
        },
    ))
}

#[test]
fn captured_original_is_owned_exact_data_without_snapshot_or_endpoint_authority() -> Result {
    let pairs = [pair(999, 998)?, pair(999, 999)?];
    let (document, binding) = captured_original(&pairs)?;
    let before = serde_json::to_vec(&document)?;
    let domain = validate_document(&document)?;
    let resolved = workforce_baseline_pairs(&domain, document.scenario_id, &binding, None)?;
    assert!(matches!(resolved, std::borrow::Cow::Owned(_)));
    assert_eq!(resolved.as_ref(), &pairs);
    assert_eq!(serde_json::to_vec(&document)?, before);
    let mut missing = binding.clone();
    missing.captured_original = None;
    assert!(workforce_baseline_pairs(&domain, document.scenario_id, &missing, None).is_err());
    let (empty_doc, empty) = captured_original(&[])?;
    assert!(
        workforce_baseline_pairs(
            &validate_document(&empty_doc)?,
            empty_doc.scenario_id,
            &empty,
            None,
        )?
        .is_empty()
    );
    Ok(())
}

#[test]
fn captured_original_rejects_closed_payload_and_exact_binding_mismatches() -> Result {
    let (mut document, binding) = captured_original(&[pair(1, 7)?, pair(1, 8)?])?;
    // An otherwise valid embedded original must not rescue a corrupt explicit capture.
    let raw = binding.captured_original.as_deref().ok_or("capture")?;
    document.domain.entities.insert(
        eutheto_types::EntityId::from_uuid(binding.original.accepted_result.solution_id.as_uuid()),
        raw.clone(),
    );
    let domain = validate_document(&document)?;
    for (field, value) in [
        ("kind", json!("manualSelection")),
        ("extra", json!(true)),
        ("id", json!(support::id(900))),
        ("sourceSolutionId", json!(support::id(900))),
        ("sourceRevision", json!(1)),
        ("assignments", json!([pair(1, 8)?, pair(1, 7)?])),
        ("assignments", json!([pair(1, 7)?, pair(1, 7)?])),
        (
            "assignments",
            json!([{"personId":support::id(1),"shiftId":support::id(7),"extra":true}]),
        ),
    ] {
        let mut malformed = binding.clone();
        std::sync::Arc::make_mut(malformed.captured_original.as_mut().ok_or("capture")?)[field] =
            value;
        assert!(
            workforce_baseline_pairs(&domain, document.scenario_id, &malformed, None).is_err(),
            "{field}"
        );
    }
    for original_hash in [false, true] {
        let mut malformed = binding.clone();
        malformed.selected_pair_content_hash = "f".repeat(64);
        if original_hash {
            malformed.original.pair_content_hash = malformed.selected_pair_content_hash.clone();
        }
        assert!(workforce_baseline_pairs(&domain, document.scenario_id, &malformed, None).is_err());
    }
    let mut wrong_id = binding.clone();
    wrong_id.original.entity = workforce_baseline_reference(support::id(900).parse()?, false)?;
    assert!(workforce_baseline_pairs(&domain, document.scenario_id, &wrong_id, None).is_err());
    let (_, manual) = snapshot()?;
    let mut wrong_selection = binding.clone();
    wrong_selection.selection = self::binding(&manual)?.selection;
    assert!(
        workforce_baseline_pairs(&domain, document.scenario_id, &wrong_selection, None).is_err()
    );
    Ok(())
}

#[test]
fn captured_original_ceiling_and_storage_use_the_inherited_admission_budget() -> Result {
    let (document, empty) = captured_original(&[])?;
    let overhead = serde_json::to_vec(empty.captured_original.as_deref().ok_or("capture")?)?.len();
    let pair_bytes = serde_json::to_vec(&pair(999, 1)?)?.len() + 1;
    let ceiling = eutheto_domain_ir::MAX_CAPTURED_BASELINE_BYTES;
    let count = (ceiling - overhead + 1) / pair_bytes;
    let pairs = (1..=u32::try_from(count)?)
        .map(|index| pair(999, index))
        .collect::<Result<Vec<_>>>()?;
    let (_, near) = captured_original(&pairs)?;
    let bytes = serde_json::to_vec(near.captured_original.as_deref().ok_or("capture")?)?.len();
    assert!(bytes <= ceiling && ceiling - bytes < pair_bytes);
    let domain = validate_document(&document)?;
    assert_eq!(
        workforce_baseline_pairs(&domain, document.scenario_id, &near, None)?.as_ref(),
        pairs
    );
    let mut over_pairs = pairs;
    over_pairs.push(pair(999, u32::try_from(count + 1)?)?);
    let (_, over) = captured_original(&over_pairs)?;
    assert!(matches!(
        workforce_baseline_pairs(&domain, document.scenario_id, &over, None),
        Err(eutheto_domain_api::DomainPackError::ResourceLimitExceeded)
    ));
    let mut budget = eutheto_workforce::assignment_rules::WorkforceAdmissionBudget::new(None);
    budget.reserve(0, 0, 16 * 1024 * 1024)?;
    assert!(matches!(
        budget.baseline_pairs(&domain, document.scenario_id, &near),
        Err(eutheto_domain_api::DomainPackError::ResourceLimitExceeded)
    ));
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        workforce_baseline_pairs(
            &domain,
            document.scenario_id,
            &near,
            Some(&OperationControl::Cancellation(token))
        ),
        Err(eutheto_domain_api::DomainPackError::Cancelled)
    ));
    Ok(())
}
