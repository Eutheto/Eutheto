#[path = "../../../tests/support/portable_encode.rs"]
mod portable_encode;

use eutheto_cli::present_solver_support_matrix;
use eutheto_core::{
    BackendSupportColumn, CapabilityMatrix, SolverSupportMatrixMetadata, SupportCell,
    SupportFeature, SupportFeatureCategory, SupportFeatureGate, SupportFeatureId,
};
use eutheto_export::{
    ApplicationMetadata, BackupSections, ScenarioExportSnapshot, assemble_scenario_export,
};
use eutheto_types::{BackendId, ScenarioSnapshotV1, SemanticCapability};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn private_tempdir() -> Result<tempfile::TempDir, Box<dyn Error>> {
    let base = dirs::home_dir().ok_or("platform home directory is unavailable")?;
    fs::create_dir_all(&base)?;
    Ok(tempfile::Builder::new()
        .prefix("eutheto-cli-phase02-test-")
        .tempdir_in(base)?)
}

fn run_json(data_dir: &Path, args: &[&str]) -> Result<(Output, Value), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["--format", "json", "--data-dir"])
        .arg(data_dir)
        .args(args)
        .output()?;
    let value = serde_json::from_slice(if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    })?;
    Ok((output, value))
}

fn write_unknown_semantic_bundle(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let wrapper: Value = serde_json::from_str(include_str!(
        "../../../tests/migration/fixtures/portable_v1_scenario.json"
    ))?;
    let mut scenario: ScenarioSnapshotV1 = serde_json::from_value(
        wrapper
            .get("input")
            .cloned()
            .ok_or("portable fixture omitted input")?,
    )?;
    scenario.required_capabilities.insert(SemanticCapability {
        id: "future.semantic".to_owned(),
        version: 1,
    });
    let bytes = assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: "0195a5e4-7c00-7000-8000-000000000202".parse()?,
            created_at: "2026-08-31T12:00:00Z".to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto CLI Phase-02 process test".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            title: "Unopened future semantics".to_owned(),
            scenario,
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        },
        &portable_encode::encode_fixture_domain,
    )?;
    fs::write(path, &bytes)?;
    Ok(bytes)
}

#[test]
fn pack_commands_expose_registry_descriptor_and_catalog_metadata() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let (list_output, list) = run_json(directory.path(), &["packs", "list"])?;
    assert!(list_output.status.success(), "{list}");
    assert!(list_output.stderr.is_empty());
    let packs = list["result"]["packs"]
        .as_array()
        .ok_or("pack list omitted packs")?;
    assert_eq!(packs.len(), 2);
    assert_eq!(packs[0]["id"], "official.test");
    assert_eq!(packs[0]["scenarioVersions"]["latest"], 1);
    assert_eq!(packs[0]["syntheticTestOnly"], true);
    assert_eq!(packs[1]["id"], "official.workforce");
    assert_eq!(packs[1]["scenarioVersions"]["latest"], 1);
    assert_eq!(packs[1]["syntheticTestOnly"], false);

    let (describe_output, describe) =
        run_json(directory.path(), &["packs", "describe", "official.test"])?;
    assert!(describe_output.status.success(), "{describe}");
    assert_eq!(describe["command"], "packs.describe");
    assert_eq!(describe["result"]["descriptor"], packs[0]);
    assert_eq!(describe["result"]["catalog"]["packId"], "official.test");

    let human = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["--data-dir"])
        .arg(directory.path())
        .args(["packs", "describe", "official.test"])
        .output()?;
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout)?;
    assert!(text.contains("official.test"));
    Ok(())
}

#[test]
fn solver_catalog_is_empty_while_ortools_matrix_and_pumpkin_gate_are_visible()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let (output, value) = run_json(directory.path(), &["solvers", "list"])?;
    assert!(output.status.success(), "{value}");
    assert!(output.stderr.is_empty());
    assert_eq!(value["result"]["solvers"], json!([]));
    assert_eq!(
        value["result"]["supportMatrix"]["productionBackendCount"],
        1
    );
    assert_eq!(
        value["result"]["supportMatrix"]["productionBackendIds"],
        json!(["solver.ortools-cp-sat"])
    );
    let columns = value["result"]["supportMatrix"]["backendColumns"]
        .as_array()
        .ok_or("solver matrix omitted backend columns")?;
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0]["backendId"], "solver.ortools-cp-sat");
    assert_eq!(columns[0]["backendVersion"], "9.15.6755");
    assert_eq!(columns[0]["adapterVersion"], "0.1.0");
    assert_eq!(
        columns[0]["cells"]
            .as_array()
            .ok_or("OR-Tools matrix column omitted support cells")?
            .len(),
        35
    );
    let features = value["result"]["supportMatrix"]["features"]
        .as_array()
        .ok_or("solver matrix omitted features")?;
    assert!(!features.is_empty());
    assert_eq!(
        value["result"]["supportMatrix"]["featureCount"],
        features.len()
    );
    let gates = value["result"]["deferredGates"]
        .as_array()
        .ok_or("solver list omitted deferred gates")?;
    assert_eq!(
        gates
            .iter()
            .map(|gate| gate["backendId"].as_str())
            .collect::<Vec<_>>(),
        vec![Some("solver.pumpkin")]
    );
    assert_eq!(gates[0]["owningPhase"], 8);

    for subcommand in ["describe", "check"] {
        let (missing_output, missing) = run_json(
            directory.path(),
            &["solvers", subcommand, "solver.ortools-cp-sat"],
        )?;
        assert_eq!(missing_output.status.code(), Some(3));
        assert!(missing_output.stdout.is_empty());
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["error"]["code"], "resource.not_found");
        assert!(missing["result"].is_null());
    }
    Ok(())
}

#[test]
fn solver_list_presentation_preserves_complete_unsupported_metadata() -> Result<(), Box<dyn Error>>
{
    let unsupported_id = SupportFeatureId::new("primitive.fixture-unsupported")?;
    let degraded_id = SupportFeatureId::new("solve.fixture-degraded")?;
    let backend_id = BackendId::new("solver.fixture")?;
    let matrix = CapabilityMatrix::new(
        1,
        2,
        vec![
            SupportFeature {
                id: degraded_id.clone(),
                category: SupportFeatureCategory::Solve,
                gate: SupportFeatureGate::Enabled("phase.fixture".to_owned()),
            },
            SupportFeature {
                id: unsupported_id.clone(),
                category: SupportFeatureCategory::Primitive,
                gate: SupportFeatureGate::Unconditional,
            },
        ],
        vec![BackendSupportColumn {
            backend_id: backend_id.clone(),
            backend_version: "0.0-fixture".to_owned(),
            adapter_version: "adapter-fixture-v2".to_owned(),
            cells: vec![
                (
                    degraded_id,
                    SupportCell::Degraded {
                        restriction_id: "restriction.fixture-cap".to_owned(),
                        reason: "Fixture degradation reason".to_owned(),
                        remediation: "Use the unrestricted fixture mode".to_owned(),
                        fixture_id: "fixture.degraded-exact".to_owned(),
                    },
                ),
                (
                    unsupported_id,
                    SupportCell::Unsupported {
                        reason: "Fixture unsupported reason".to_owned(),
                        remediation: "Choose the fixture alternative".to_owned(),
                        fixture_id: "fixture.unsupported-exact".to_owned(),
                    },
                ),
            ],
        }],
        Vec::new(),
    )?;
    let metadata = SolverSupportMatrixMetadata {
        schema_version: matrix.schema_version(),
        planning_ir_schema_version: matrix.planning_ir_schema_version(),
        features: matrix.features().cloned().collect(),
        production_backend_ids: matrix.production_backend_ids().cloned().collect(),
        backend_columns: matrix.backend_columns().collect(),
    };

    let presentation = present_solver_support_matrix(&metadata);
    assert_eq!(
        presentation.json["backendColumns"],
        json!([
            {
                "backendId": "solver.fixture",
                "backendVersion": "0.0-fixture",
                "adapterVersion": "adapter-fixture-v2",
                "cells": [
                    {
                        "featureId": "primitive.fixture-unsupported",
                        "support": "unsupported",
                        "reason": "Fixture unsupported reason",
                        "remediation": "Choose the fixture alternative",
                        "fixtureId": "fixture.unsupported-exact"
                    },
                    {
                        "featureId": "solve.fixture-degraded",
                        "support": "degraded",
                        "restrictionId": "restriction.fixture-cap",
                        "reason": "Fixture degradation reason",
                        "remediation": "Use the unrestricted fixture mode",
                        "fixtureId": "fixture.degraded-exact"
                    }
                ]
            }
        ])
    );
    assert_eq!(
        presentation.human,
        vec![
            "Support matrix: schema 1, planning IR schema 2, 2 features, 1 production backends.",
            "Warning: backend solver.fixture feature primitive.fixture-unsupported is unsupported: Fixture unsupported reason Remediation: Choose the fixture alternative Fixture: fixture.unsupported-exact.",
            "Warning: backend solver.fixture feature solve.fixture-degraded is degraded (restriction.fixture-cap): Fixture degradation reason Remediation: Use the unrestricted fixture mode Fixture: fixture.degraded-exact.",
        ]
    );
    Ok(())
}

#[test]
fn unopened_inspection_and_exact_reexport_preserve_bytes_without_import_fallback()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let source = directory.path().join("future.eutheto");
    let original = write_unknown_semantic_bundle(&source)?;
    let source_arg = source.to_str().ok_or("bundle path is not UTF-8")?;

    let (import_output, import) = run_json(directory.path(), &["projects", "import", source_arg])?;
    assert!(!import_output.status.success());
    assert_eq!(import["error"]["code"], "portable.capability_unsupported");

    let (inspect_output, inspect) = run_json(directory.path(), &["bundle", "inspect", source_arg])?;
    assert!(inspect_output.status.success(), "{inspect}");
    assert!(inspect_output.stderr.is_empty());
    assert_eq!(inspect["command"], "bundle.inspect");
    assert_eq!(inspect["status"], "inspected");
    assert_eq!(inspect["result"]["reexported"], false);
    assert!(
        inspect["result"]["metadata"]["requiredCapabilities"]
            .as_array()
            .ok_or("missing required capability metadata")?
            .contains(&json!({"id": "future.semantic", "version": 1}))
    );
    assert_eq!(
        inspect["result"]["metadata"]["title"],
        "Unopened future semantics"
    );
    assert!(inspect["result"].get("previewId").is_none());
    assert!(inspect["result"]["metadata"].get("bytes").is_none());
    assert!(inspect["result"]["metadata"].get("document").is_none());

    let destination = directory.path().join("preserved.eutheto");
    let destination_arg = destination
        .to_str()
        .ok_or("destination path is not UTF-8")?;
    let (reexport_output, reexport) = run_json(
        directory.path(),
        &["bundle", "exact-reexport", source_arg, destination_arg],
    )?;
    assert!(reexport_output.status.success(), "{reexport}");
    assert_eq!(reexport["command"], "bundle.exact-reexport");
    assert_eq!(reexport["result"]["reexported"], true);
    assert_eq!(fs::read(&destination)?, original);

    let occupied = directory.path().join("occupied.eutheto");
    fs::write(&occupied, b"existing")?;
    let occupied_arg = occupied.to_str().ok_or("occupied path is not UTF-8")?;
    let (occupied_output, occupied_error) = run_json(
        directory.path(),
        &["bundle", "exact-reexport", source_arg, occupied_arg],
    )?;
    assert!(!occupied_output.status.success());
    assert_eq!(occupied_error["ok"], false);
    assert_eq!(fs::read(&occupied)?, b"existing");

    let invalid = directory.path().join("invalid.eutheto");
    fs::write(&invalid, b"not a bundle")?;
    let invalid_arg = invalid.to_str().ok_or("invalid path is not UTF-8")?;
    let absent = directory.path().join("must-not-exist.eutheto");
    let absent_arg = absent.to_str().ok_or("absent path is not UTF-8")?;
    let (invalid_output, invalid_error) = run_json(
        directory.path(),
        &["bundle", "exact-reexport", invalid_arg, absent_arg],
    )?;
    assert!(!invalid_output.status.success());
    assert_eq!(invalid_error["ok"], false);
    assert!(!absent.exists());
    Ok(())
}

#[test]
fn help_catalog_includes_unopened_bundle_commands() -> Result<(), Box<dyn Error>> {
    let root = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .arg("--help")
        .output()?;
    assert!(root.status.success());
    assert!(String::from_utf8(root.stdout)?.contains("bundle"));

    let bundle = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["bundle", "--help"])
        .output()?;
    assert!(bundle.status.success());
    assert!(bundle.stderr.is_empty());
    let help = String::from_utf8(bundle.stdout)?;
    assert!(help.contains("inspect"));
    assert!(help.contains("exact-reexport"));
    Ok(())
}
