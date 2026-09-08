#[path = "../../../tests/support/portable_encode.rs"]
mod portable_encode;

use eutheto_export::{
    ApplicationMetadata, BackupSections, ScenarioExportSnapshot, assemble_scenario_export,
};
use eutheto_types::ScenarioSnapshotV1;
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
        .prefix("eutheto-cli-test-")
        .tempdir_in(base)?)
}

fn write_extension_fixture_bundle(path: &Path) -> Result<String, Box<dyn Error>> {
    let wrapper: Value = serde_json::from_str(include_str!(
        "../../../tests/migration/fixtures/portable_v1_scenario.json"
    ))?;
    let scenario: ScenarioSnapshotV1 = serde_json::from_value(
        wrapper
            .get("input")
            .cloned()
            .ok_or("portable fixture omitted input")?,
    )?;
    let scenario_id = scenario.document.scenario_id.to_string();
    let bytes = assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: "0195a5e4-7c00-7000-8000-000000000201".parse()?,
            created_at: "2026-08-31T12:00:00Z".to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto CLI process test".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            title: scenario.document.metadata.title.clone(),
            scenario,
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::from(["vendor.example".to_owned()]),
            manifest_extensions: BTreeMap::new(),
        },
        &portable_encode::encode_fixture_domain,
    )?;
    fs::write(path, bytes)?;
    Ok(scenario_id)
}

#[cfg(windows)]
#[test]
fn private_storage_opens_under_the_windows_profile() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(eutheto_store::SqliteScenarioStore::open(
        &directory.path().join("probe.sqlite"),
    ));
    if let Err(error) = result {
        return Err(format!("Windows private storage probe failed: {error:?}").into());
    }
    Ok(())
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

fn create_project(data_dir: &Path, title: &str) -> Result<String, Box<dyn Error>> {
    let (output, value) = run_json(
        data_dir,
        &[
            "projects",
            "create",
            "--pack",
            "official.test",
            "--title",
            title,
        ],
    )?;
    assert!(output.status.success(), "{value}");
    assert!(output.stderr.is_empty());
    Ok(value["result"]["scenarioId"]
        .as_str()
        .ok_or("create response omitted scenarioId")?
        .to_owned())
}

#[test]
fn help_exposes_the_complete_roadmap_catalog_and_global_flags() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .arg("--help")
        .output()?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout)?;
    for flag in [
        "--format",
        "--log-level",
        "--no-color",
        "--data-dir",
        "--config",
        "--offline",
    ] {
        assert!(help.contains(flag), "missing {flag}");
    }
    for command in [
        "doctor",
        "info",
        "licenses",
        "packs",
        "bundle",
        "projects",
        "backup",
        "scenario",
        "solve",
        "solutions",
        "solvers",
        "ai",
        "serve",
    ] {
        assert!(help.contains(command), "missing {command}");
    }

    let version = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .arg("--version")
        .output()?;
    assert_eq!(version.status.code(), Some(0));
    assert!(version.stderr.is_empty());
    assert!(
        String::from_utf8(version.stdout)?.starts_with("optimizer "),
        "version output was not routed to stdout"
    );

    let projects = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["projects", "--help"])
        .output()?;
    assert_eq!(projects.status.code(), Some(0));
    assert!(projects.stderr.is_empty());
    let projects_help = String::from_utf8(projects.stdout)?;
    for command in [
        "list",
        "create",
        "duplicate",
        "archive",
        "unarchive",
        "import",
        "export",
        "delete",
    ] {
        assert!(
            projects_help.contains(command),
            "missing projects {command}"
        );
    }

    let scenario = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["scenario", "--help"])
        .output()?;
    assert_eq!(scenario.status.code(), Some(0));
    assert!(scenario.stderr.is_empty());
    let scenario_help = String::from_utf8(scenario.stdout)?;
    for command in [
        "show", "migrate", "validate", "apply", "batch", "undo", "redo", "history",
    ] {
        assert!(
            scenario_help.contains(command),
            "missing scenario {command}"
        );
    }
    Ok(())
}

#[test]
fn json_help_and_version_are_single_result_envelopes() -> Result<(), Box<dyn Error>> {
    for args in [
        vec!["--format", "json", "--help"],
        vec!["--format", "json", "--version"],
        vec!["--format", "json", "projects", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
            .args(args)
            .output()?;
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = std::str::from_utf8(&output.stdout)?;
        let envelope_text = text
            .strip_suffix('\n')
            .ok_or("JSON display output must end in one newline")?;
        assert!(
            !envelope_text.contains('\n'),
            "JSON display output must be exactly one envelope line"
        );
        let envelope: Value = serde_json::from_str(envelope_text)?;
        assert_eq!(envelope["apiVersion"], "eutheto/cli-result/v1");
        assert_eq!(envelope["ok"], true);
        assert_eq!(envelope["status"], "displayed");
        assert!(envelope["result"]["text"].as_str().is_some());
        assert_eq!(envelope["warnings"], json!([]));
    }
    Ok(())
}

#[test]
fn legacy_status_json_preflight_envelopes_parse_errors() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["status", "--json", "--not-a-real-flag"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let text = std::str::from_utf8(&output.stderr)?;
    let envelope = text
        .strip_suffix('\n')
        .ok_or("legacy JSON usage error must end in one newline")?;
    assert!(!envelope.contains('\n'));
    let value: Value = serde_json::from_str(envelope)?;
    assert_eq!(value["apiVersion"], "eutheto/cli-result/v1");
    assert_eq!(value["command"], "usage");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "cli.usage");
    Ok(())
}

#[test]
fn project_create_list_reopen_archive_and_delete_are_durable() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Durable roster")?;

    let (list_output, list) = run_json(directory.path(), &["projects", "list"])?;
    assert!(list_output.status.success());
    assert_eq!(list["result"][0]["scenarioId"], id);
    assert_eq!(list["result"][0]["title"], "Durable roster");

    let (show_output, show) = run_json(directory.path(), &["scenario", "show", &id])?;
    assert!(show_output.status.success());
    assert_eq!(show["result"]["document"]["scenarioId"], id);
    assert_eq!(show["result"]["revision"], 0);

    let (archive_output, _) = run_json(
        directory.path(),
        &["projects", "archive", &id, "--expected-revision", "0"],
    )?;
    assert!(archive_output.status.success());
    let (_, active) = run_json(directory.path(), &["projects", "list"])?;
    assert_eq!(active["result"], json!([]));
    let (_, archived) = run_json(
        directory.path(),
        &["projects", "list", "--scope", "archived"],
    )?;
    assert_eq!(archived["result"][0]["scenarioId"], id);

    let (unarchive_output, _) = run_json(
        directory.path(),
        &["projects", "unarchive", &id, "--expected-revision", "0"],
    )?;
    assert!(unarchive_output.status.success());
    let (delete_output, _) = run_json(
        directory.path(),
        &["projects", "delete", &id, "--expected-revision", "0"],
    )?;
    assert!(delete_output.status.success());
    let (_, all) = run_json(directory.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(all["result"], json!([]));
    Ok(())
}

#[test]
fn unsafe_revision_argument_is_rejected_without_rounding() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Safe revision")?;
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "projects",
            "archive",
            &id,
            "--expected-revision",
            "9007199254740992",
        ],
    )?;
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(envelope["error"]["code"], "revision.out_of_range");
    let (_, project_list) = run_json(directory.path(), &["projects", "list"])?;
    assert_eq!(project_list["result"][0]["revision"], 0);
    assert_eq!(project_list["result"][0]["archived"], false);
    Ok(())
}

#[test]
fn scenario_apply_validate_history_undo_and_redo_use_authoritative_revisions()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Journal")?;
    let commands = directory.path().join("commands.json");
    let entity_id = "018f47f2-e880-7000-8000-000000000001";
    fs::write(
        &commands,
        serde_json::to_vec(&json!({
            "type": "addEntity",
            "payload": {
                "entityId": entity_id,
                "value": {"id": entity_id, "enabled": true, "target": 3}
            }
        }))?,
    )?;

    let commands_path = commands.to_str().ok_or("non-UTF-8 test path")?;
    let (apply_output, apply) = run_json(
        directory.path(),
        &[
            "scenario",
            "apply",
            &id,
            "--commands",
            commands_path,
            "--expected-revision",
            "0",
        ],
    )?;
    assert!(apply_output.status.success(), "{apply}");
    assert_eq!(apply["result"]["newRevision"], 1);

    let (validate_output, validation) = run_json(directory.path(), &["scenario", "validate", &id])?;
    assert!(validate_output.status.success());
    assert_eq!(validation["status"], "valid");

    let (history_output, history) = run_json(directory.path(), &["scenario", "history", &id])?;
    assert!(history_output.status.success());
    assert_eq!(
        history["result"]
            .as_array()
            .ok_or("history is not an array")?
            .len(),
        1
    );
    assert_eq!(history["result"][0]["revisionAfter"], 1);

    let (undo_output, undo) = run_json(
        directory.path(),
        &["scenario", "undo", &id, "--expected-revision", "1"],
    )?;
    assert!(undo_output.status.success(), "{undo}");
    assert_eq!(undo["result"]["newRevision"], 2);
    let (redo_output, redo) = run_json(
        directory.path(),
        &["scenario", "redo", &id, "--expected-revision", "2"],
    )?;
    assert!(redo_output.status.success(), "{redo}");
    assert_eq!(redo["result"]["newRevision"], 3);
    Ok(())
}

#[test]
fn committed_apply_returns_success_warning_when_output_publication_fails()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Committed warning")?;
    let commands = directory.path().join("commit-warning.json");
    let entity_id = "018f47f2-e880-7000-8000-00000000000a";
    fs::write(
        &commands,
        serde_json::to_vec(&json!({
            "type": "addEntity",
            "payload": {
                "entityId": entity_id,
                "value": {"id": entity_id, "name": "Committed"}
            }
        }))?,
    )?;
    let commands_path = commands.to_str().ok_or("non-UTF-8 command path")?;
    let unwritable_output = directory.path().to_str().ok_or("non-UTF-8 output path")?;
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "scenario",
            "apply",
            &id,
            "--commands",
            commands_path,
            "--expected-revision",
            "0",
            "--output",
            unwritable_output,
        ],
    )?;
    assert!(output.status.success(), "{envelope}");
    assert!(output.stderr.is_empty());
    assert_eq!(envelope["status"], "applied");
    assert_eq!(envelope["result"]["newRevision"], 1);
    assert_eq!(
        envelope["warnings"][0]["code"],
        "scenario.output_publication_failed"
    );
    assert_eq!(
        envelope["warnings"][0]["details"]["causeCode"],
        "portable.destination_exists"
    );

    let (_, view) = run_json(directory.path(), &["scenario", "show", &id])?;
    assert_eq!(view["result"]["revision"], 1);
    assert_eq!(
        view["result"]["document"]["domain"]["entities"][entity_id]["name"],
        "Committed"
    );
    let human_id = create_project(directory.path(), "Human committed warning")?;
    let human_entity_id = "018f47f2-e880-7000-8000-00000000000b";
    fs::write(
        &commands,
        serde_json::to_vec(&json!({
            "type": "addEntity",
            "payload": {
                "entityId": human_entity_id,
                "value": {"id": human_entity_id, "name": "Human committed"}
            }
        }))?,
    )?;
    let human = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .arg("--data-dir")
        .arg(directory.path())
        .args([
            "scenario",
            "apply",
            &human_id,
            "--commands",
            commands_path,
            "--expected-revision",
            "0",
            "--output",
            unwritable_output,
        ])
        .output()?;
    assert!(human.status.success());
    assert!(String::from_utf8(human.stdout)?.contains("Applied command; revision 1."));
    assert!(String::from_utf8(human.stderr)?.contains("scenario.output_publication_failed"));
    Ok(())
}

#[test]
fn closed_warning_stderr_does_not_reverse_a_committed_command_disposition()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Closed diagnostic sink")?;
    let commands = directory.path().join("closed-stderr-command.json");
    let entity_id = "018f47f2-e880-7000-8000-00000000000b";
    fs::write(
        &commands,
        serde_json::to_vec(&json!({
            "type": "addEntity",
            "payload": {"entityId": entity_id, "value": {"id": entity_id, "name": "Retained"}}
        }))?,
    )?;
    let (reader, writer) = std::io::pipe()?;
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .arg("--data-dir")
        .arg(directory.path())
        .args(["scenario", "apply", &id, "--commands"])
        .arg(&commands)
        .args(["--expected-revision", "0", "--output"])
        .arg(directory.path())
        .stderr(writer)
        .output()?;
    assert!(output.status.success(), "{output:?}");
    let (_, view) = run_json(directory.path(), &["scenario", "show", &id])?;
    assert_eq!(view["result"]["revision"], 1);
    assert_eq!(
        view["result"]["document"]["domain"]["entities"][entity_id]["name"],
        "Retained"
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn scenario_export_inspect_import_and_backup_restore_round_trip() -> Result<(), Box<dyn Error>> {
    let source = private_tempdir()?;
    let imported = private_tempdir()?;
    let restored = private_tempdir()?;
    let seed_bundle = source.path().join("extension-seed.eutheto");
    let id = write_extension_fixture_bundle(&seed_bundle)?;
    let seed_path = seed_bundle.to_str().ok_or("non-UTF-8 seed path")?;
    let (seed_output, seed_import) = run_json(source.path(), &["projects", "import", seed_path])?;
    assert!(seed_output.status.success(), "{seed_import}");
    assert_eq!(seed_import["result"]["scenarioIds"][0], id);
    let scenario_bundle = source.path().join("scenario.eutheto");
    let backup_bundle = source.path().join("backup.eutheto");
    let (setting_output, _) = run_json(
        source.path(),
        &[
            "settings",
            "set",
            "appearance",
            "{\"theme\":\"dark\",\"reducedMotion\":false}",
        ],
    )?;
    assert!(setting_output.status.success());
    let scenario_path = scenario_bundle.to_str().ok_or("non-UTF-8 scenario path")?;
    let backup_path = backup_bundle.to_str().ok_or("non-UTF-8 backup path")?;

    let (export_output, _) = run_json(
        source.path(),
        &["projects", "export", &id, "--output", scenario_path],
    )?;
    assert!(export_output.status.success());
    assert!(scenario_bundle.is_file());

    let (import_output, import) =
        run_json(imported.path(), &["projects", "import", scenario_path])?;
    assert!(import_output.status.success(), "{import}");
    assert_eq!(import["result"]["scenarioIds"][0], id);
    assert_eq!(
        import["result"]["scenarioOutcomes"][0]["sourceScenarioId"],
        id
    );
    assert_eq!(import["result"]["scenarioOutcomes"][0]["scenarioId"], id);
    assert_eq!(
        import["result"]["scenarioOutcomes"][0]["selectedAction"],
        "same-identity"
    );
    assert!(
        import["result"]["preview"]["preservedExtensions"]
            .as_array()
            .is_some_and(|extensions| extensions.contains(&json!("vendor.example")))
    );
    let (imported_show_output, imported_show) =
        run_json(imported.path(), &["scenario", "show", &id])?;
    assert!(imported_show_output.status.success(), "{imported_show}");
    assert_eq!(
        imported_show["result"]["document"]["extensions"]["vendor.example"],
        json!({"opaque": [1, "two", true]})
    );
    let (review_output, review) =
        run_json(imported.path(), &["projects", "import", scenario_path])?;
    assert!(review_output.status.success(), "{review}");
    assert_eq!(review["status"], "review-required");
    assert_eq!(
        review["result"]["requiredCollisionPlan"]["scenarios"][id.as_str()],
        "create-copy"
    );
    assert_eq!(
        review["result"]["collisionActionChoices"]["scenarios"],
        json!(["create-copy", "replace", "skip"])
    );
    let (_, unchanged_after_review) = run_json(imported.path(), &["projects", "list"])?;
    assert_eq!(
        unchanged_after_review["result"].as_array().map(Vec::len),
        Some(1)
    );
    let plan = review["result"]["requiredCollisionPlan"].to_string();
    let (apply_output, applied) = run_json(
        imported.path(),
        &[
            "projects",
            "import",
            scenario_path,
            "--collision-plan",
            &plan,
        ],
    )?;
    assert!(apply_output.status.success(), "{applied}");
    assert_eq!(
        applied["result"]["scenarioIds"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        applied["result"]["scenarioOutcomes"][0]["sourceScenarioId"],
        id
    );
    assert_eq!(
        applied["result"]["scenarioOutcomes"][0]["scenarioId"],
        applied["result"]["scenarioIds"][0]
    );
    assert_ne!(applied["result"]["scenarioIds"][0], id);
    assert_eq!(
        applied["result"]["scenarioOutcomes"][0]["selectedAction"],
        "create-copy"
    );
    assert_eq!(
        applied["result"]["scenarioOutcomes"][0]["revision"],
        applied["result"]["preview"]["scenarios"][0]["sourceRevision"]
    );
    assert!(applied["result"]["scenarioOutcomes"][0]["warning"].is_null());
    let mut skip_plan = review["result"]["requiredCollisionPlan"].clone();
    skip_plan["scenarios"][id.as_str()] = json!("skip");
    let skip_plan = skip_plan.to_string();
    let (skip_output, skipped) = run_json(
        imported.path(),
        &[
            "projects",
            "import",
            scenario_path,
            "--collision-plan",
            &skip_plan,
        ],
    )?;
    assert!(skip_output.status.success(), "{skipped}");
    assert_eq!(skipped["result"]["scenarioIds"], json!([]));
    assert_eq!(
        skipped["result"]["scenarioOutcomes"][0]["sourceScenarioId"],
        id
    );
    assert!(skipped["result"]["scenarioOutcomes"][0]["scenarioId"].is_null());
    assert_eq!(
        skipped["result"]["scenarioOutcomes"][0]["selectedAction"],
        "skip"
    );

    let same_name_id = create_project(imported.path(), "Deterministic fixture")?;
    assert_ne!(same_name_id, id);
    let target_only_entity = "018f47f2-e880-7000-8000-000000000099";
    let target_mutation = imported.path().join("target-only-command.json");
    fs::write(
        &target_mutation,
        serde_json::to_vec(&json!({
            "type": "addEntity",
            "payload": {
                "entityId": target_only_entity,
                "value": {"id": target_only_entity, "name": "Target-only"}
            }
        }))?,
    )?;
    let target_mutation_path = target_mutation
        .to_str()
        .ok_or("non-UTF-8 target mutation path")?;
    let (mutation_output, mutation) = run_json(
        imported.path(),
        &[
            "scenario",
            "apply",
            &id,
            "--commands",
            target_mutation_path,
            "--expected-revision",
            "7",
        ],
    )?;
    assert!(mutation_output.status.success(), "{mutation}");
    assert_eq!(mutation["result"]["newRevision"], 8);
    let (_, before_replace) = run_json(imported.path(), &["projects", "list"])?;
    assert_eq!(before_replace["result"].as_array().map(Vec::len), Some(3));

    let mut replace_plan = review["result"]["requiredCollisionPlan"].clone();
    replace_plan["scenarios"][id.as_str()] = json!("replace");
    let replace_plan = replace_plan.to_string();
    let (replace_output, replaced) = run_json(
        imported.path(),
        &[
            "projects",
            "import",
            scenario_path,
            "--collision-plan",
            &replace_plan,
        ],
    )?;
    assert!(replace_output.status.success(), "{replaced}");
    assert_eq!(replaced["result"]["scenarioIds"], json!([id]));
    assert_eq!(
        replaced["result"]["scenarioOutcomes"][0]["sourceScenarioId"],
        id
    );
    assert_eq!(replaced["result"]["scenarioOutcomes"][0]["scenarioId"], id);
    assert_eq!(
        replaced["result"]["scenarioOutcomes"][0]["selectedAction"],
        "replace"
    );
    assert_eq!(
        replaced["result"]["scenarioOutcomes"][0]["revision"],
        replaced["result"]["preview"]["scenarios"][0]["sameIdentityRevision"]
    );

    let (replaced_show_output, replaced_show) =
        run_json(imported.path(), &["scenario", "show", &id])?;
    assert!(replaced_show_output.status.success(), "{replaced_show}");
    assert_eq!(
        replaced_show["result"]["revision"],
        replaced["result"]["scenarioOutcomes"][0]["revision"]
    );
    assert!(
        replaced_show["result"]["document"]["domain"]["entities"][target_only_entity].is_null()
    );
    assert_eq!(
        replaced_show["result"]["document"]["metadata"]["title"],
        "Deterministic fixture"
    );
    assert_eq!(
        replaced_show["result"]["document"]["extensions"]["vendor.example"],
        json!({"opaque": [1, "two", true]})
    );
    let (same_name_show_output, same_name_show) =
        run_json(imported.path(), &["scenario", "show", &same_name_id])?;
    assert!(same_name_show_output.status.success(), "{same_name_show}");
    assert_eq!(
        same_name_show["result"]["document"]["metadata"]["title"],
        "Deterministic fixture"
    );
    assert_eq!(same_name_show["result"]["revision"], 0);
    let (_, after_replace) = run_json(imported.path(), &["projects", "list"])?;
    let after_replace = after_replace["result"]
        .as_array()
        .ok_or("project list result is not an array")?;
    assert_eq!(after_replace.len(), 3);
    assert_eq!(
        after_replace
            .iter()
            .filter(|project| project["scenarioId"].as_str() == Some(id.as_str()))
            .count(),
        1
    );
    assert!(
        after_replace
            .iter()
            .any(|project| project["scenarioId"].as_str() == Some(same_name_id.as_str()))
    );

    let included = import["result"]["preview"]["includedSections"]
        .as_array()
        .ok_or("import preview omitted included sections")?;
    assert!(included.contains(&json!("results")));
    assert!(included.contains(&json!("assets")));
    let excluded_import = private_tempdir()?;
    let (excluded_output, excluded) = run_json(
        excluded_import.path(),
        &[
            "projects",
            "import",
            scenario_path,
            "--exclude-results",
            "--exclude-assets",
        ],
    )?;
    assert!(excluded_output.status.success(), "{excluded}");
    let excluded_sections = excluded["result"]["preview"]["excludedSections"]
        .as_array()
        .ok_or("excluded import preview omitted excluded sections")?;
    assert!(excluded_sections.contains(&json!("results")));
    assert!(excluded_sections.contains(&json!("assets")));
    let (backup_output, _) = run_json(
        source.path(),
        &["backup", "create", "--output", backup_path],
    )?;
    assert!(backup_output.status.success());
    let (inspect_output, inspect) = run_json(source.path(), &["backup", "inspect", backup_path])?;
    assert!(inspect_output.status.success(), "{inspect}");
    assert_eq!(inspect["status"], "valid");
    assert_eq!(inspect["result"]["scenarios"][0]["scenarioId"], id);
    assert!(inspect["result"]["scenarios"][0]["sourceRevision"].is_number());
    assert!(inspect["result"]["scenarios"][0]["sameIdentityRevision"].is_number());
    assert!(inspect["result"]["scenarios"][0]["sameIdentityRevisionWarning"].is_string());

    let (restore_output, restore) = run_json(
        restored.path(),
        &["backup", "restore", backup_path, "--mode", "add"],
    )?;
    assert!(restore_output.status.success(), "{restore}");
    assert!(
        restore["result"]["preview"]["settingsChanged"]
            .as_array()
            .is_some_and(|keys| keys.contains(&json!("appearance")))
    );
    let (_, list) = run_json(restored.path(), &["projects", "list"])?;
    assert_eq!(list["result"][0]["scenarioId"], id);
    Ok(())
}

#[test]

fn large_asset_exclusion_keeps_the_assets_section_and_reports_scope() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    create_project(directory.path(), "Large asset selection")?;
    let fixed_exclusions = json!([
        "local-undo-and-audit-history",
        "sqlite-and-database-internals",
        "credentials-tokens-and-keychain-references",
        "device-local-paths-and-window-state",
        "logs-caches-and-temporary-data",
        "redistribution-prohibited-provider-data",
        "executable-content"
    ]);
    let backup = directory.path().join("threshold-backup.eutheto");
    let backup_path = backup.to_str().ok_or("non-UTF-8 backup path")?;
    let (create_output, created) = run_json(
        directory.path(),
        &[
            "backup",
            "create",
            "--output",
            backup_path,
            "--exclude-large-assets",
            "--exclude-results",
        ],
    )?;
    assert!(create_output.status.success(), "{created}");
    assert_eq!(created["result"]["includeResults"], false);
    assert_eq!(created["result"]["assetSelection"], "v1-threshold");
    assert_eq!(created["result"]["excludedAssetCount"], 0);
    assert_eq!(created["result"]["excludedAssetIds"], json!([]));
    assert_eq!(created["result"]["exclusionScope"], Value::Null);
    assert_eq!(created["result"]["fixedExclusions"], fixed_exclusions);
    assert!(
        created["result"]["largeAssetThresholdBytes"]
            .as_u64()
            .is_some_and(|threshold| threshold > 0)
    );

    let (inspect_output, inspected) =
        run_json(directory.path(), &["backup", "inspect", backup_path])?;
    assert!(inspect_output.status.success(), "{inspected}");
    let included = inspected["result"]["includedSections"]
        .as_array()
        .ok_or("backup preview omitted included sections")?;
    assert!(included.contains(&json!("assets")));
    assert!(
        !inspected["result"]["excludedSections"]
            .as_array()
            .ok_or("backup preview omitted excluded sections")?
            .contains(&json!("assets"))
    );
    assert_eq!(
        inspected["result"]["sourceBackupSelection"]["includeResults"],
        false
    );
    assert_eq!(
        inspected["result"]["sourceBackupSelection"]["assetSelection"],
        "v1-threshold"
    );
    assert_eq!(
        inspected["result"]["sourceBackupSelection"]["excludedAssetCount"],
        0
    );
    assert_eq!(
        inspected["result"]["sourceBackupSelection"]["fixedExclusions"],
        fixed_exclusions
    );
    assert_eq!(inspected["result"]["omittedAssets"], json!([]));
    Ok(())
}

#[test]
fn replace_restore_requires_review_and_rejects_a_forged_failure_receipt()
-> Result<(), Box<dyn Error>> {
    let source = private_tempdir()?;
    let target = private_tempdir()?;
    let source_id = create_project(source.path(), "Backup source")?;
    let target_id = create_project(target.path(), "Keep target")?;
    let backup = source.path().join("replace-token.eutheto");
    let backup_path = backup.to_str().ok_or("non-UTF-8 backup path")?;
    let (create_output, created) = run_json(
        source.path(),
        &["backup", "create", "--output", backup_path],
    )?;
    assert!(create_output.status.success(), "{created}");

    let invalid_plan =
        format!(r#"{{"scenarios":{{"{source_id}":"replace"}},"supplementalChoices":[]}}"#);
    let (invalid_output, invalid) = run_json(
        target.path(),
        &[
            "backup",
            "restore",
            backup_path,
            "--mode",
            "replace",
            "--collision-plan",
            &invalid_plan,
        ],
    )?;
    assert_eq!(invalid_output.status.code(), Some(3));
    assert_eq!(invalid["error"]["code"], "collision_plan.invalid");
    assert!(invalid["result"].is_null());
    assert!(!serde_json::to_string(&invalid)?.contains("reviewToken"));
    let (_, unchanged_after_invalid) =
        run_json(target.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(
        unchanged_after_invalid["result"][0]["scenarioId"],
        target_id
    );

    let (review_output, review) = run_json(
        target.path(),
        &["backup", "restore", backup_path, "--mode", "replace"],
    )?;
    assert!(review_output.status.success(), "{review}");
    assert_eq!(review["status"], "review-required");
    assert_eq!(review["result"]["localLibraryRevision"], 1);
    assert_eq!(
        review["result"]["removedScenarios"][0]["scenarioId"],
        target_id
    );
    let review_token = review["result"]["reviewToken"]
        .as_str()
        .ok_or("review response omitted token")?;
    let (_, unchanged) = run_json(target.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(unchanged["result"][0]["scenarioId"], target_id);

    let (forged_output, forged) = run_json(
        target.path(),
        &[
            "backup",
            "restore",
            backup_path,
            "--mode",
            "replace",
            "--confirm-replace",
            "--review-token",
            review_token,
            "--without-backup-token",
            "01900000-0000-7000-8000-000000000099",
        ],
    )?;
    assert_eq!(forged_output.status.code(), Some(3));
    assert_eq!(
        forged["error"]["code"],
        "restore.safety_backup_receipt_rejected"
    );
    let (_, still_unchanged) = run_json(target.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(still_unchanged["result"][0]["scenarioId"], target_id);

    let (apply_output, applied) = run_json(
        target.path(),
        &[
            "backup",
            "restore",
            backup_path,
            "--mode",
            "replace",
            "--confirm-replace",
            "--review-token",
            review_token,
        ],
    )?;
    assert!(apply_output.status.success(), "{applied}");
    let (_, projects) = run_json(target.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(projects["result"].as_array().map(Vec::len), Some(1));
    assert_eq!(projects["result"][0]["scenarioId"], source_id);
    Ok(())
}

#[test]
fn replace_restore_returns_and_accepts_only_an_actual_failure_receipt() -> Result<(), Box<dyn Error>>
{
    let source = private_tempdir()?;
    let target = private_tempdir()?;
    let source_id = create_project(source.path(), "Receipt source")?;
    let target_id = create_project(target.path(), "Receipt target")?;
    let backup = source.path().join("receipt-backup.eutheto");
    let backup_path = backup.to_str().ok_or("non-UTF-8 backup path")?;
    let (create_output, created) = run_json(
        source.path(),
        &["backup", "create", "--output", backup_path],
    )?;
    assert!(create_output.status.success(), "{created}");
    let (review_output, review) = run_json(
        target.path(),
        &["backup", "restore", backup_path, "--mode", "replace"],
    )?;
    assert!(review_output.status.success(), "{review}");
    let review_token = review["result"]["reviewToken"]
        .as_str()
        .ok_or("review response omitted token")?;

    let safety_backups = target.path().join("safety-backups");
    if safety_backups.is_dir() {
        fs::remove_dir_all(&safety_backups)?;
    }
    fs::write(&safety_backups, b"block safety backup directory creation")?;
    let (failure_output, failure) = run_json(
        target.path(),
        &[
            "backup",
            "restore",
            backup_path,
            "--mode",
            "replace",
            "--confirm-replace",
            "--review-token",
            review_token,
        ],
    )?;
    assert_eq!(failure_output.status.code(), Some(8));
    assert_eq!(failure["error"]["code"], "restore.safety_backup_failed");
    let receipt = failure["error"]["details"]["failureReceiptToken"]
        .as_str()
        .ok_or("safety-backup failure omitted its receipt")?;
    assert_eq!(failure["error"]["details"]["reviewToken"], review_token);
    let (_, unchanged) = run_json(target.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(unchanged["result"][0]["scenarioId"], target_id);

    fs::remove_file(&safety_backups)?;
    let (apply_output, applied) = run_json(
        target.path(),
        &[
            "backup",
            "restore",
            backup_path,
            "--mode",
            "replace",
            "--confirm-replace",
            "--review-token",
            review_token,
            "--without-backup-token",
            receipt,
        ],
    )?;
    assert!(apply_output.status.success(), "{applied}");
    let (_, projects) = run_json(target.path(), &["projects", "list", "--scope", "all"])?;
    assert_eq!(projects["result"].as_array().map(Vec::len), Some(1));
    assert_eq!(projects["result"][0]["scenarioId"], source_id);
    Ok(())
}

#[test]
fn json_is_exactly_one_envelope_and_human_diagnostics_stay_on_stderr() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let (output, value) = run_json(directory.path(), &["projects", "list"])?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = std::str::from_utf8(&output.stdout)?;
    let Some(envelope) = stdout.strip_suffix('\n') else {
        return Err("JSON output must end with one newline".into());
    };
    assert!(
        !envelope.contains('\n'),
        "JSON output must contain exactly one envelope line"
    );
    assert_eq!(value["apiVersion"], "eutheto/cli-result/v1");
    assert_eq!(value["ok"], true);
    assert!(value.get("warnings").is_some());
    assert!(value.get("diagnosticId").is_some());

    let human = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["--data-dir"])
        .arg(directory.path())
        .args(["scenario", "show", "not-an-id"])
        .output()?;
    assert_eq!(human.status.code(), Some(3));
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8(human.stderr)?.contains("scenario.id_invalid"));
    Ok(())
}

#[test]
fn usage_validation_storage_conflict_and_unavailable_have_stable_exit_codes()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Exit codes")?;

    let usage = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .arg("unknown-command")
        .output()?;
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty());
    assert!(
        String::from_utf8(usage.stderr)?.contains("cli.usage"),
        "usage diagnostic was not routed to stderr"
    );

    let (validation_output, validation) =
        run_json(directory.path(), &["scenario", "show", "invalid"])?;
    assert_eq!(validation_output.status.code(), Some(3));
    assert_eq!(validation["error"]["code"], "scenario.id_invalid");

    let secret_path = directory.path().join("token-super-secret-missing.eutheto");
    let secret_path_text = secret_path.to_str().ok_or("non-UTF-8 secret path")?;
    let (storage_output, storage) =
        run_json(directory.path(), &["projects", "import", secret_path_text])?;
    assert_eq!(storage_output.status.code(), Some(8));
    let storage_text = String::from_utf8(storage_output.stderr.clone())?;
    assert_eq!(storage["error"]["code"], "storage.read_failed");
    assert!(!storage_text.contains(secret_path_text));
    assert!(!storage_text.contains("token-super-secret"));

    let commands = directory.path().join("conflict.json");
    let entity_id = "018f47f2-e880-7000-8000-000000000002";
    fs::write(
        &commands,
        serde_json::to_vec(&json!({
            "type": "addEntity",
            "payload": {"entityId": entity_id, "value": {"id": entity_id, "name": "Grace"}}
        }))?,
    )?;
    let commands_path = commands.to_str().ok_or("non-UTF-8 command path")?;
    let (first_output, _) = run_json(
        directory.path(),
        &[
            "scenario",
            "apply",
            &id,
            "--commands",
            commands_path,
            "--expected-revision",
            "0",
        ],
    )?;
    assert!(first_output.status.success());
    let (conflict_output, conflict) = run_json(
        directory.path(),
        &[
            "scenario",
            "apply",
            &id,
            "--commands",
            commands_path,
            "--expected-revision",
            "0",
        ],
    )?;
    assert_eq!(conflict_output.status.code(), Some(10));
    assert_eq!(conflict["error"]["code"], "state.revision_conflict");

    let (unavailable_output, unavailable) = run_json(
        directory.path(),
        &[
            "solutions",
            "export",
            "scenario",
            "solution",
            "--format",
            "ics",
        ],
    )?;
    assert_eq!(unavailable_output.status.code(), Some(6));
    assert_eq!(
        unavailable["error"]["code"],
        "solution.export_format_unavailable"
    );
    Ok(())
}

#[test]
fn strict_json_rejects_duplicate_command_and_collision_plan_keys() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let id = create_project(directory.path(), "Strict JSON")?;
    let commands = directory.path().join("duplicates.json");
    fs::write(
        &commands,
        br#"{"type":"addEntity","type":"removeEntity","payload":{}}"#,
    )?;
    let commands_path = commands.to_str().ok_or("non-UTF-8 command path")?;
    let (command_output, command_error) = run_json(
        directory.path(),
        &[
            "scenario",
            "apply",
            &id,
            "--commands",
            commands_path,
            "--expected-revision",
            "0",
        ],
    )?;
    assert_eq!(command_output.status.code(), Some(3));
    assert_eq!(command_error["error"]["code"], "json.invalid");

    let scenario_bundle = directory.path().join("strict.eutheto");
    let scenario_path = scenario_bundle.to_str().ok_or("non-UTF-8 scenario path")?;
    let (export_output, _) = run_json(
        directory.path(),
        &["projects", "export", &id, "--output", scenario_path],
    )?;
    assert!(export_output.status.success());
    let other = private_tempdir()?;
    let collision = r#"{"scenarios":{},"scenarios":{}}"#;
    let (collision_output, collision_error) = run_json(
        other.path(),
        &[
            "projects",
            "import",
            scenario_path,
            "--collision-plan",
            collision,
        ],
    )?;
    assert_eq!(collision_output.status.code(), Some(3));
    assert_eq!(collision_error["error"]["code"], "json.invalid");
    Ok(())
}
