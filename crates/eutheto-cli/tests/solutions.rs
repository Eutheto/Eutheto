use serde_json::Value;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const MISSING_SOLUTION_A: &str = "01900000-0000-7000-8000-000000000101";
const MISSING_SOLUTION_B: &str = "01900000-0000-7000-8000-000000000102";

fn private_tempdir() -> Result<tempfile::TempDir, Box<dyn Error>> {
    let base = dirs::home_dir().ok_or("platform home directory is unavailable")?;
    fs::create_dir_all(&base)?;
    Ok(tempfile::Builder::new()
        .prefix("eutheto-cli-solutions-test-")
        .tempdir_in(base)?)
}

fn run_json(data_dir: &Path, args: &[&str]) -> Result<(Output, Value), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["--format", "json", "--data-dir"])
        .arg(data_dir)
        .args(args)
        .output()?;
    let bytes = if output.status.success() {
        assert!(output.stderr.is_empty());
        &output.stdout
    } else {
        assert!(output.stdout.is_empty());
        &output.stderr
    };
    let text = std::str::from_utf8(bytes)?;
    let envelope = text
        .strip_suffix('\n')
        .ok_or("JSON output must end in exactly one newline")?;
    assert!(!envelope.contains('\n'), "JSON output must be one envelope");
    let value = serde_json::from_str(envelope)?;
    Ok((output, value))
}

fn create_project(data_dir: &Path) -> Result<String, Box<dyn Error>> {
    let (output, envelope) = run_json(
        data_dir,
        &[
            "projects",
            "create",
            "--pack",
            "official.test",
            "--title",
            "Solution adapter fixture",
        ],
    )?;
    assert!(output.status.success(), "{envelope}");
    Ok(envelope["result"]["scenarioId"]
        .as_str()
        .ok_or("project response omitted scenarioId")?
        .to_owned())
}

fn create_workforce_scenario(data_dir: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let output_path = output.to_str().ok_or("non-UTF-8 scenario path")?;
    let (command, envelope) = run_json(
        data_dir,
        &[
            "projects",
            "create",
            "--pack",
            "official.workforce",
            "--title",
            "ICS export fixture",
            "--output",
            output_path,
        ],
    )?;
    assert!(command.status.success(), "{envelope}");
    Ok(())
}

#[test]
fn solution_list_is_a_real_versioned_core_query() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = create_project(directory.path())?;
    let (output, envelope) = run_json(directory.path(), &["solutions", "list", &scenario])?;

    assert!(output.status.success(), "{envelope}");
    assert_eq!(envelope["apiVersion"], "eutheto/cli-result/v1");
    assert_eq!(envelope["command"], "solutions.list");
    assert_eq!(envelope["status"], "listed");
    assert_eq!(envelope["warnings"], serde_json::json!([]));
    assert_eq!(envelope["result"]["schemaVersion"], 1);
    assert_eq!(envelope["result"]["scenarioId"], scenario);
    assert_eq!(envelope["result"]["solutions"], serde_json::json!([]));
    Ok(())
}

#[test]
fn real_solution_queries_reject_malformed_typed_ids_before_authority_lookup()
-> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = create_project(directory.path())?;
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &["solutions", "list", "not-a-scenario"],
            "solutions.list",
            "scenario.id_invalid",
        ),
        (
            &["solutions", "verify", "not-a-scenario", MISSING_SOLUTION_A],
            "solutions.verify",
            "scenario.id_invalid",
        ),
        (
            &["solutions", "verify", &scenario, "not-a-solution"],
            "solutions.verify",
            "solution.id_invalid",
        ),
        (
            &[
                "solutions",
                "compare",
                &scenario,
                "not-a-solution",
                MISSING_SOLUTION_B,
            ],
            "solutions.compare",
            "solution.id_invalid",
        ),
        (
            &[
                "solutions",
                "explain",
                &scenario,
                "not-a-solution",
                "--assignment-id",
                "assignment.fixture",
            ],
            "solutions.explain",
            "solution.id_invalid",
        ),
    ];
    for (args, command, code) in cases {
        let (output, envelope) = run_json(directory.path(), args)?;
        assert_eq!(output.status.code(), Some(3), "{envelope}");
        assert_eq!(envelope["command"], *command);
        assert_eq!(envelope["error"]["code"], *code);
        assert!(envelope["result"].is_null());
        assert_eq!(envelope["warnings"], serde_json::json!([]));
    }
    Ok(())
}

#[test]
fn verify_compare_and_explain_dispatch_to_core_authority() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = create_project(directory.path())?;
    let cases: &[(&[&str], &str)] = &[
        (
            &["solutions", "verify", &scenario, MISSING_SOLUTION_A],
            "solutions.verify",
        ),
        (
            &[
                "solutions",
                "compare",
                &scenario,
                MISSING_SOLUTION_A,
                MISSING_SOLUTION_B,
            ],
            "solutions.compare",
        ),
        (
            &[
                "solutions",
                "explain",
                &scenario,
                MISSING_SOLUTION_A,
                "--assignment-id",
                "assignment.fixture",
            ],
            "solutions.explain",
        ),
    ];
    for (args, command) in cases {
        let (output, envelope) = run_json(directory.path(), args)?;
        assert_eq!(output.status.code(), Some(3), "{envelope}");
        assert_eq!(envelope["command"], *command);
        assert_eq!(envelope["error"]["code"], "resource.not_found");
    }
    Ok(())
}

#[test]
fn json_export_requires_destination_before_input_or_database_access() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let missing_scenario_path = directory.path().join("secret-scenario-path");
    let missing_solution_path = directory.path().join("secret-solution-path");
    let scenario = missing_scenario_path.to_str().ok_or("non-UTF-8 path")?;
    let solution = missing_solution_path.to_str().ok_or("non-UTF-8 path")?;
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "solutions",
            "export",
            scenario,
            solution,
            "--format",
            "json",
        ],
    )?;

    assert_eq!(output.status.code(), Some(2), "{envelope}");
    assert_eq!(envelope["command"], "solutions.export");
    assert_eq!(envelope["error"]["code"], "solution.export_output_required");
    let serialized = serde_json::to_string(&envelope)?;
    assert!(!serialized.contains(scenario));
    assert!(!serialized.contains(solution));
    assert!(!missing_scenario_path.exists());
    assert!(!missing_solution_path.exists());
    assert!(fs::read_dir(directory.path())?.next().is_none());
    Ok(())
}

#[test]
fn ics_person_filter_is_restricted_to_ics_without_touching_inputs() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = directory.path().join("not-read-scenario");
    let solution = directory.path().join("not-read-solution");
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "solutions",
            "export",
            scenario.to_str().ok_or("non-UTF-8 path")?,
            solution.to_str().ok_or("non-UTF-8 path")?,
            "--format",
            "csv",
            "--person-id",
            "not-a-person",
        ],
    )?;
    assert_eq!(output.status.code(), Some(2), "{envelope}");
    assert_eq!(envelope["error"]["code"], "solution.person_filter_invalid");
    assert!(!scenario.exists());
    assert!(!solution.exists());
    Ok(())
}

#[test]
fn ics_person_filter_rejects_non_uuidv7_without_touching_inputs() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = directory.path().join("not-read-scenario");
    let solution = directory.path().join("not-read-solution");
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "solutions",
            "export",
            scenario.to_str().ok_or("non-UTF-8 path")?,
            solution.to_str().ok_or("non-UTF-8 path")?,
            "--format",
            "ics",
            "--person-id",
            "not-a-person",
        ],
    )?;
    assert_eq!(output.status.code(), Some(3), "{envelope}");
    assert_eq!(envelope["error"]["code"], "solution.person_id_invalid");
    assert!(!scenario.exists());
    assert!(!solution.exists());
    Ok(())
}

#[test]
fn ics_export_rejects_unverified_result_before_publishing() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = directory.path().join("scenario.json");
    let solution = directory.path().join("unverified-result.json");
    let output_path = directory.path().join("schedule.ics");
    create_workforce_scenario(directory.path(), &scenario)?;
    fs::write(&solution, b"{}")?;
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "solutions",
            "export",
            scenario.to_str().ok_or("non-UTF-8 scenario path")?,
            solution.to_str().ok_or("non-UTF-8 solution path")?,
            "--format",
            "ics",
            "--output",
            output_path.to_str().ok_or("non-UTF-8 output path")?,
        ],
    )?;
    assert_eq!(output.status.code(), Some(8), "{envelope}");
    assert_eq!(envelope["error"]["code"], "solution.portable_invalid");
    assert!(!output_path.exists());
    Ok(())
}

#[test]
fn html_export_requires_a_destination_before_reading_sources() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_optimizer"))
        .args(["--data-dir"])
        .arg(directory.path())
        .args(["solutions", "export"])
        .arg(directory.path().join("missing-scenario.json"))
        .arg(directory.path().join("missing-result.json"))
        .args(["--format", "html"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(std::str::from_utf8(&output.stderr)?.contains("solution.export_output_required"));
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn report_disclosures_cannot_silently_affect_other_formats() -> Result<(), Box<dyn Error>> {
    let directory = private_tempdir()?;
    let scenario = directory.path().join("missing-scenario.json");
    let result = directory.path().join("missing-result.json");
    let destination = directory.path().join("output.json");
    for option in [
        "--include-person-names",
        "--include-authored-labels",
        "--include-staffing",
        "--include-changes",
        "--include-fairness-actuals",
    ] {
        let (output, envelope) = run_json(
            directory.path(),
            &[
                "solutions",
                "export",
                scenario.to_str().ok_or("non-UTF-8 path")?,
                result.to_str().ok_or("non-UTF-8 path")?,
                "--format",
                "json",
                "--output",
                destination.to_str().ok_or("non-UTF-8 path")?,
                option,
            ],
        )?;
        assert_eq!(output.status.code(), Some(2), "{envelope}");
        assert_eq!(envelope["error"]["code"], "solution.report_options_invalid");
    }
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn html_export_rejects_unverified_file_without_a_database_or_output() -> Result<(), Box<dyn Error>>
{
    let directory = private_tempdir()?;
    let scenario = directory.path().join("scenario.json");
    let solution = directory.path().join("unverified.json");
    let destination = directory.path().join("report.html");
    create_workforce_scenario(directory.path(), &scenario)?;
    fs::write(&solution, b"{}")?;
    let (output, envelope) = run_json(
        directory.path(),
        &[
            "solutions",
            "export",
            scenario.to_str().ok_or("non-UTF-8 path")?,
            solution.to_str().ok_or("non-UTF-8 path")?,
            "--format",
            "html",
            "--output",
            destination.to_str().ok_or("non-UTF-8 path")?,
        ],
    )?;
    assert_eq!(output.status.code(), Some(8), "{envelope}");
    assert_eq!(envelope["error"]["code"], "solution.portable_invalid");
    assert!(!destination.exists());
    let entries = fs::read_dir(directory.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    assert_eq!(
        entries,
        ["scenario.json", "unverified.json"]
            .map(std::ffi::OsString::from)
            .into()
    );
    Ok(())
}
