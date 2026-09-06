#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod support;

use eutheto_domain_api::{CommandDescriptor, PortableImportContext};
use eutheto_export::{
    ApplicationMetadata, BackupSections, ScenarioExportSnapshot, assemble_scenario_export,
};
use eutheto_import::{
    CollisionAction, CollisionPlan, DecodedPortableDomain, ImportOptions, InspectionPolicy,
    LocalLibrarySnapshot, MigrationFailure, MigrationRegistries, RestoreMode, build_preview,
    inspect_bundle, stage_import,
};
use eutheto_types::{Revision, ScenarioDocument, ScenarioDomain, ScenarioSnapshotV1};
use eutheto_workforce::{
    generated_workforce_pack_contract::WORKFORCE_PACK_CONTRACT_JSON,
    portable::{export_portable, import_portable},
    validation::validate_document,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
};
use support::id;

fn entity(document: &mut ScenarioDocument, index: u32) -> Result<&mut Value, Box<dyn Error>> {
    document
        .domain
        .entities
        .get_mut(&id(index).parse()?)
        .ok_or_else(|| "fixture entity missing".into())
}

fn reference_fixture() -> Result<ScenarioDocument, Box<dyn Error>> {
    let mut document = support::fixture()?;
    let contract: Value = serde_json::from_str(WORKFORCE_PACK_CONTRACT_JSON)?;
    let commands: Vec<CommandDescriptor> = serde_json::from_value(contract["commands"].clone())?;
    let additions = commands
        .iter()
        .find(|command| command.id == "official.workforce.add_entity")
        .ok_or("entity examples")?;
    for (kind, index) in [
        ("team", 20),
        ("availability", 21),
        ("coverageRequirement", 22),
        ("baseSchedule", 23),
    ] {
        let mut record = additions
            .valid_examples
            .iter()
            .find(|example| example["entity"]["kind"] == kind)
            .ok_or("missing entity kind")?["entity"]
            .clone();
        record["id"] = json!(id(index));
        document.domain.entities.insert(id(index).parse()?, record);
    }
    entity(&mut document, 1)?["teamIds"] = json!([id(20)]);
    entity(&mut document, 1)?["homeLocationId"] = json!(id(5));
    // UUID-looking external identifiers/prose are deliberately not references.
    entity(&mut document, 1)?["externalId"] = json!(id(20));
    entity(&mut document, 1)?["name"] = json!(id(7));
    let mut other = entity(&mut document, 1)?.clone();
    other["id"] = json!(id(16));
    other["externalId"] = json!("other-staff");
    document.domain.entities.insert(id(16).parse()?, other);
    document.domain.entities.insert(id(17).parse()?, json!({"kind":"location","id":id(17),"name":"South","transitions":[{"locationId":id(5),"minutes":15}]}));
    entity(&mut document, 5)?["transitions"] = json!([{"locationId":id(17),"minutes":20}]);
    let mut detached = entity(&mut document, 8)?.clone();
    detached["id"] = json!(id(18));
    detached["origin"] =
        json!({"kind":"detached","templateId":id(6),"occurrenceDate":"2026-11-08"});
    document.domain.entities.insert(id(18).parse()?, detached);
    entity(&mut document, 21)?["assignmentTypeIds"] = json!([id(4)]);
    entity(&mut document, 21)?["locationIds"] = json!([id(5)]);
    entity(&mut document, 22)?["coverage"] =
        json!({"kind":"atLeast","minimum":1,"preferredCount":2,"qualificationMinimums":[]});
    entity(&mut document, 22)?["scope"] =
        json!({"kind":"selected","shiftIds":[id(7),id(8),id(18)]});
    entity(&mut document, 23)?["assignments"] = json!([
        {"personId":id(1),"shiftId":id(7)}, {"personId":id(1),"shiftId":id(8)}, {"personId":id(16),"shiftId":id(18)}
    ]);
    for (command_id, field, first_id) in [
        ("official.workforce.add_rule", "rule", 200_u32),
        ("official.workforce.add_preference", "preference", 300_u32),
    ] {
        let descriptor = commands
            .iter()
            .find(|command| command.id == command_id)
            .ok_or("semantic examples")?;
        let mut kinds = BTreeSet::new();
        for example in &descriptor.valid_examples {
            let mut record = example[field].clone();
            let kind = record["kind"].as_str().ok_or("kind")?.to_owned();
            if !kinds.insert(kind.clone()) {
                continue;
            }
            let record_id = id(first_id + u32::try_from(kinds.len())?);
            record["id"] = json!(record_id);
            match kind.as_str() {
                "noOverlap" => record["compatibleCategoryPairs"] = json!([]),
                "mutualAssignmentRestriction" | "togetherSeparate" => {
                    record["personIds"] = json!([id(1), id(16)]);
                }
                "requestedTimeOff" => record["availabilityIds"] = json!([id(21)]),
                "adjacency" | "baseStability" => record["baseScheduleId"] = json!(id(23)),
                "preferredCoverage" => record["coverageRequirementIds"] = json!([id(22)]),
                _ => {}
            }
            if field == "rule" {
                document.domain.rules.insert(record_id.parse()?, record);
            } else {
                document
                    .domain
                    .preferences
                    .insert(record_id.parse()?, record);
            }
        }
    }
    validate_document(&document)?;
    Ok(document)
}

fn stage_reference_fixture(
    original: &ScenarioDocument,
) -> Result<eutheto_import::StagedImport, Box<dyn Error>> {
    let result_id = id(50);
    let source_id = original.scenario_id;
    let result = json!({"resultId":result_id,"scenarioId":source_id,"scenarioRevision":0,
        "payload":{"assignments":[{"personId":id(1),"shiftId":id(7)},{"personId":id(16),"shiftId":id(18)}]}});
    let snapshot = ScenarioExportSnapshot {
        bundle_id: id(500).parse()?,
        created_at: "2026-09-05T00:00:00Z".to_owned(),
        application: ApplicationMetadata {
            name: "Eutheto".to_owned(),
            version: "0.1.0".to_owned(),
        },
        title: "Workforce reference copy".to_owned(),
        scenario: ScenarioSnapshotV1::current(Revision::new(0), original.clone(), BTreeSet::new()),
        scenario_revisions: vec![],
        sections: BackupSections {
            results: BTreeMap::from([(result_id, result)]),
            ..BackupSections::default()
        },
        nonsemantic_extensions: BTreeSet::new(),
        manifest_extensions: BTreeMap::new(),
    };
    let bytes = assemble_scenario_export(&snapshot, &|document| {
        export_portable(document)
            .map_err(|error| eutheto_export::ExportError::InvalidModel(error.to_string()))
    })?;
    let inspected = inspect_bundle(
        &bytes,
        &InspectionPolicy {
            supported_capabilities: BTreeMap::from([("official.workforce.portable".to_owned(), 1)]),
            ..InspectionPolicy::default()
        },
        &MigrationRegistries::current_only(),
        &|wire| {
            let context = PortableImportContext {
                scenario_shell: ScenarioDocument::new(
                    wire.scenario_id,
                    original.domain_pack.clone(),
                    wire.metadata.clone(),
                    wire.settings.clone(),
                    ScenarioDomain::default(),
                    BTreeMap::new(),
                ),
            };
            Ok(DecodedPortableDomain {
                document: import_portable(&wire.domain, &context)
                    .map_err(|error| MigrationFailure::Invalid(error.to_string()))?,
                required_capabilities: wire.domain.required_capabilities.clone(),
                applied_migrations: vec![],
            })
        },
    )?;
    assert_eq!(&inspected.scenarios[0].document, original);
    let local = LocalLibrarySnapshot {
        supplemental_identity_owners: BTreeMap::new(),
        identity_owners: BTreeMap::new(),
        revision: Revision::new(0),
        scenario_ids: BTreeSet::from([source_id]),
        scenario_revision_high_water: BTreeMap::new(),
        occupied_uuids: BTreeSet::from([source_id.to_string()]),
        scenarios: Vec::new(),
        supplemental_identities: BTreeSet::new(),
        settings: BTreeMap::new(),
    };
    let options = ImportOptions {
        restore_mode: RestoreMode::ImportScenario,
        include_results: true,
        include_assets: false,
    };
    let preview = build_preview(&inspected, &options, &local)?;
    let staged = stage_import(
        &inspected,
        &preview,
        &options,
        &local,
        &CollisionPlan {
            scenarios: BTreeMap::from([(source_id, CollisionAction::CreateCopy)]),
            supplemental: BTreeMap::new(),
        },
    )?;
    Ok(staged)
}

#[test]
fn workforce_archive_copy_remaps_owned_definitions_references_and_inert_results()
-> Result<(), Box<dyn Error>> {
    let original = reference_fixture()?;
    let source_id = original.scenario_id;
    let staged = stage_reference_fixture(&original)?;
    let copy = &staged.scenarios[0];
    let document = &copy.scenario.document;
    validate_document(document)?;
    let source_owned = eutheto_types::collect_document_owned_uuids(&original);
    let copy_owned = eutheto_types::collect_document_owned_uuids(document);
    assert_eq!(source_owned.len(), copy_owned.len());
    assert!(source_owned.is_disjoint(&copy_owned));
    let copied_result: Value = serde_json::from_slice(
        staged
            .results
            .values()
            .next()
            .ok_or("missing inert result")?,
    )?;
    let mapped = |index| -> Result<String, Box<dyn Error>> {
        Ok(copy
            .id_remap
            .get(&id(index).parse()?)
            .ok_or("missing identity mapping")?
            .to_string())
    };
    let domain = serde_json::to_value(&document.domain)?;
    assert_ne!(document.scenario_id, source_id);
    assert_eq!(domain["entities"][mapped(1)?]["externalId"], id(20));
    assert_eq!(domain["entities"][mapped(1)?]["name"], id(7));
    assert_eq!(
        domain["entities"][mapped(1)?]["teamIds"],
        json!([mapped(20)?])
    );
    assert_eq!(
        domain["entities"][mapped(5)?]["transitions"][0]["locationId"],
        mapped(17)?
    );
    assert_eq!(
        domain["entities"][mapped(6)?]["occurrenceIdentities"][mapped(7)?]["id"],
        mapped(7)?
    );
    assert_eq!(
        domain["entities"][mapped(18)?]["origin"]["templateId"],
        mapped(6)?
    );
    assert_ne!(copied_result["resultId"], id(50));
    assert_eq!(
        domain["entities"][mapped(23)?]["sourceSolutionId"],
        copied_result["resultId"]
    );
    assert_eq!(
        domain["lockedAssignments"][mapped(14)?]["shiftId"],
        mapped(7)?
    );
    assert_eq!(document.extensions, original.extensions);
    assert_eq!(document.metadata, original.metadata);
    assert_eq!(document.settings, original.settings);
    let kinds = |map: &BTreeMap<_, Value>| -> BTreeSet<String> {
        map.values()
            .filter_map(|record| record["kind"].as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(
        kinds(&original.domain.entities),
        kinds(&document.domain.entities)
    );
    assert_eq!(document.domain.rules.len(), 12);
    assert_eq!(document.domain.preferences.len(), 11);
    assert_eq!(
        copied_result["scenarioId"],
        document.scenario_id.to_string()
    );
    assert_eq!(
        copied_result["payload"]["assignments"][0]["shiftId"],
        mapped(7)?
    );
    assert_eq!(
        copied_result["payload"]["assignments"][1]["personId"],
        mapped(16)?
    );
    assert_eq!(
        copied_result["payload"]["assignments"][1]["shiftId"],
        mapped(18)?
    );
    let roundtrip = import_portable(
        &export_portable(document)?,
        &PortableImportContext {
            scenario_shell: document.clone(),
        },
    )?;
    assert_eq!(roundtrip, *document);
    Ok(())
}
