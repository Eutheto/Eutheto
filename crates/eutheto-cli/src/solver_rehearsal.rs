//! Packaged diagnostic rehearsal. Build the CLI lib test with the same manifest path and
//! trusted digest as the packaged CLI. Copy its executable beside `optimizer` in each
//! disposable image and run the ignored test with `EUTHETO_SOLVER_REHEARSAL_EXPECT` set
//! to `available`, `missing` (worker removed), or `tampered` (worker bytes changed).

use super::{
    Cli, CliExitCode, Command, Outcome, OutputFormat, SafeCliError, app_dependencies,
    bundled_solver,
};
use clap::Parser;
use eutheto_core::EuthetoApp;
use eutheto_types::{CancellationToken, OperationControl};
use serde_json::Value;
use std::error::Error;

async fn inspect_installed_solver(cli: Cli) -> Result<Outcome, SafeCliError> {
    let Command::Solvers(args) = cli.command else {
        return Err(super::unexpected_result());
    };
    let cancellation = CancellationToken::new();
    let registry =
        bundled_solver::load(&OperationControl::Cancellation(cancellation.clone())).await?;
    let dependencies = app_dependencies(cli.data_dir, &cancellation)?;
    let app = EuthetoApp::open_with_solver_registry(dependencies, registry)
        .await
        .map_err(super::app_error)?;
    super::execute_solvers(&app, args)
        .await
        .map_err(|(_, error)| error)
}

#[tokio::test]
#[ignore = "requires a manifest-bound test executable relocated beside the approved worker image"]
async fn installed_worker_solver_inspection_rehearsal() -> Result<(), Box<dyn Error>> {
    assert!(
        !env!("EUTHETO_ORTOOLS_MANIFEST_SHA256").is_empty(),
        "build this rehearsal with both approved worker manifest handoffs"
    );
    let expected = std::env::var("EUTHETO_SOLVER_REHEARSAL_EXPECT")?;
    assert!(matches!(
        expected.as_str(),
        "available" | "missing" | "tampered"
    ));
    let directory = tempfile::Builder::new()
        .prefix("eutheto-solver-rehearsal-")
        .tempdir_in(dirs::home_dir().ok_or("platform home directory is unavailable")?)?;
    let cli = Cli::try_parse_from([
        "optimizer",
        "--format",
        "json",
        "--data-dir",
        directory
            .path()
            .to_str()
            .ok_or("non-UTF-8 data directory")?,
        "solvers",
        "check",
        "solver.ortools-cp-sat",
    ])?;
    let format = cli.format;
    let checked = inspect_installed_solver(cli).await;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if expected != "available" {
        let Err(error) = checked else {
            return Err("an unavailable worker image was admitted".into());
        };
        assert_eq!(error.exit, CliExitCode::Unavailable);
        assert_eq!(error.code, "backend.worker_invalid");
        super::render_error(&mut stdout, &mut stderr, format, "solvers.check", &error)?;
        assert!(stdout.is_empty());
        let envelope: Value = serde_json::from_slice(&stderr)?;
        assert_eq!(envelope["command"], "solvers.check");
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "backend.worker_invalid");
        assert!(std::fs::read_dir(directory.path())?.next().is_none());
        return Ok(());
    }
    let outcome = checked.map_err(|error| format!("solver inspection failed: {}", error.code))?;
    assert_eq!(outcome.exit, CliExitCode::Success);
    super::render_success(&mut stdout, &mut stderr, format, &outcome)?;
    assert!(stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&stdout)?;
    assert_eq!(envelope["command"], "solvers.check");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["status"], "available");
    assert_eq!(envelope["result"]["available"], true);
    assert_eq!(envelope["result"]["solver"]["id"], "solver.ortools-cp-sat");
    stdout.clear();
    super::render_success(&mut stdout, &mut stderr, OutputFormat::Human, &outcome)?;
    assert_eq!(
        stdout,
        b"solver.ortools-cp-sat is registered and available.\n"
    );
    assert!(stderr.is_empty());
    Ok(())
}
