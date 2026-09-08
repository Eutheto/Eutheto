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
