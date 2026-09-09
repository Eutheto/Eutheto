//! Consumer assertions against an already assembled, manifest-bound optimizer image.
//! Nothing in this module grants imported schedules or solver history authority.

use crate::contract::{
    CLI_EVIDENCE_FORMAT, CORPUS_VERSION, CaseId, CliCheck, CliEvidence, ExpectedDisposition,
    FIXTURE_CLOCK, FORMAT_VERSION, LoadedCorpus, LoadedFixture, MeasurementProfile,
};
use crate::files::{hash_file, read_bounded, sha256};
use anyhow::{Context, Result, bail, ensure};
use eutheto_domain_ir::PortableAcceptedResultV2;
use eutheto_export::{
    ApplicationMetadata, BackupSections, ScenarioExportSnapshot, assemble_scenario_export,
};
use eutheto_types::{CancellationToken, OperationControl, SolveStatus};
use eutheto_workforce::{
    assignment_rules::decode_workforce_assignment, assignments_csv::decode_assignments_csv,
    model::AssignmentPair,
};
#[cfg(windows)]
use process_wrap::std::JobObject;
#[cfg(unix)]
use process_wrap::std::ProcessGroup;
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MAX_PROCESS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const COMMAND_GUARD: Duration = Duration::from_mins(2);
const CLEANUP_GRACE: Duration = Duration::from_secs(10);

pub(crate) fn exercise(
    image: &Path,
    corpus: &LoadedCorpus,
    trusted_manifest_sha256: &str,
    cancellation: &CancellationToken,
) -> Result<CliEvidence> {
    ensure!(!cancellation.is_cancelled(), "CLI exercise cancelled");
    ensure!(
        trusted_manifest_sha256.len() == 64
            && trusted_manifest_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid trusted worker manifest digest"
    );
    let image = fs::canonicalize(image).map_err(|_| anyhow::anyhow!("CLI image is unavailable"))?;
    let executable = image.join(if cfg!(windows) {
        "optimizer.exe"
    } else {
        "optimizer"
    });
    let worker = image.join(if cfg!(windows) {
        "ortools-worker.exe"
    } else {
        "ortools-worker"
    });
    let manifest = image.join("solver/ortools/solver-manifest.json");
    let manifest_sha256 = hash_file(&manifest, MAX_PROCESS_BYTES)?;
    ensure!(
        manifest_sha256 == trusted_manifest_sha256,
        "CLI image manifest differs from trusted worker artifact"
    );
    let cli_sha256 = hash_file(&executable, MAX_EXECUTABLE_BYTES)?;
    let worker_sha256 = hash_file(&worker, MAX_EXECUTABLE_BYTES)?;
    // The installed CLI itself validates every worker/resource against its embedded digest.
    // This directory also satisfies stored-library custody, unlike global /tmp ancestry.
    let scratch = tempfile::Builder::new()
        .prefix(".workforce-cli-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .map_err(|_| anyhow::anyhow!("private CLI scratch creation failed"))?;
    let root = scratch.path();
    fs::create_dir(root.join("home"))?;
    let runner = Runner {
        executable,
        root: root.to_owned(),
        cancellation: cancellation.clone(),
    };
    let mut checks = vec![CliCheck {
        case_id: CaseId::ClinicTiny,
        operation: "image-worker".to_owned(),
        exit_code: 0,
        observed_status: None,
        observed_code: None,
        artifact_sha256: Some(worker_sha256.clone()),
    }];
    for fixture in &corpus.fixtures {
        exercise_case(&runner, fixture, &mut checks)?;
    }
    ensure!(
        hash_file(&manifest, MAX_PROCESS_BYTES)? == manifest_sha256
            && hash_file(&runner.executable, MAX_EXECUTABLE_BYTES)? == cli_sha256
            && hash_file(&worker, MAX_EXECUTABLE_BYTES)? == worker_sha256,
        "CLI image changed during corpus exercise"
    );
    ensure!(!cancellation.is_cancelled(), "CLI exercise cancelled");
    Ok(CliEvidence {
        format: CLI_EVIDENCE_FORMAT.to_owned(),
        schema_version: FORMAT_VERSION,
        corpus_version: CORPUS_VERSION,
        corpus_sha256: corpus.manifest_sha256.clone(),
        cli_sha256,
        worker_manifest_sha256: manifest_sha256,
        checks,
    })
}

struct CasePaths<'a> {
    directory: &'a Path,
    library: &'a Path,
    input: &'a Path,
    output: &'a Path,
    diagnostics: &'a Path,
}

fn exercise_case(
    runner: &Runner,
    fixture: &LoadedFixture,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    ensure!(
        !runner.cancellation.is_cancelled(),
        "CLI exercise cancelled"
    );
    let id = fixture.expected.id;
    let directory = runner.root.join(id.slug());
    fs::create_dir(&directory)?;
    let input = directory.join("scenario.json");
    // Use the exact bytes whose hash and decoded snapshot the primary loader validated.
    stage(&input, &fixture.bytes)?;
    let library = directory.join("library");
    let output = directory.join("accepted.json");
    let diagnostics = directory.join("diagnostics.json");
    let paths = CasePaths {
        directory: &directory,
        library: &library,
        input: &input,
        output: &output,
        diagnostics: &diagnostics,
    };
    validate_input(runner, fixture, &paths, checks)?;
    let (solved, exit) = solve_input(runner, fixture, &paths)?;
    if fixture.expected.disposition == ExpectedDisposition::Accepted {
        accepted_solve(runner, fixture, &paths, &solved, exit, checks)?;
    } else {
        rejected_solve(fixture, &paths, &solved, exit, checks)?;
    }
    if matches!(id, CaseId::DstSpring | CaseId::DstFall) {
        reject_dst_policy(runner, fixture, &directory, &library, checks)?;
    }
    if id == CaseId::ClinicTiny {
        let mut newer: Value = serde_json::from_slice(&fixture.bytes)?;
        let version = newer["schemaVersion"]
            .as_u64()
            .context("portable input omitted version")?;
        newer["schemaVersion"] = json!(
            version
                .checked_add(1)
                .context("portable version overflow")?
        );
        let source = directory.join("newer-version.json");
        stage(&source, &serde_json::to_vec(&newer)?)?;
        // Standalone JSON has no outer archive checksum; this reaches the version codec directly.
        let rejected = runner.run(
            &library,
            &["scenario", "validate", text(&source)?],
            8,
            false,
        )?;
        error_code(&rejected, "portable.version_newer")?;
        record(
            checks,
            id,
            "portable.newer-version-rejected",
            8,
            None,
            Some("portable.version_newer"),
            None,
        );
    }
    ensure!(
        !library.exists(),
        "file CLI commands unexpectedly opened a stored library"
    );
    Ok(())
}

fn validate_input(
    runner: &Runner,
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let id = fixture.expected.id;
    let library = paths.library;
    let input = paths.input;
    let input_arg = text(input)?;
    let shown = runner.run(library, &["scenario", "show", input_arg], 0, true)?;
    ensure!(
        shown["result"] == serde_json::to_value(&fixture.snapshot)?,
        "CLI portable decoding changed corpus semantics"
    );
    record(
        checks,
        id,
        "scenario.show",
        0,
        None,
        None,
        Some(sha256(&fixture.bytes)),
    );
    if fixture.expected.disposition == ExpectedDisposition::Accepted {
        let valid = runner.run(library, &["scenario", "validate", input_arg], 0, true)?;
        ensure!(
            valid["status"] == "valid"
                && valid["result"]["revision"] == fixture.snapshot.revision.value(),
            "CLI validation did not validate the corpus revision"
        );
        record(checks, id, "scenario.validate", 0, None, None, None);
    }
    if fixture.expected.disposition == ExpectedDisposition::Infeasible {
        let readiness = runner.run(library, &["scenario", "validate", input_arg], 3, true)?;
        ensure!(
            readiness["status"] == "invalid"
                && readiness["result"]["revision"] == fixture.snapshot.revision.value()
                && readiness["result"]["issues"]
                    .as_array()
                    .is_some_and(|issues| issues.iter().any(|issue| issue["code"]
                        == "official.workforce.candidate_shortage"
                        && issue["severity"] == "error")),
            "insufficient eligible coverage did not preserve its readiness finding"
        );
        // A readiness finding is not solver proof; the independent solve below must dispatch.
        record(
            checks,
            id,
            "scenario.validate-readiness-shortage",
            3,
            None,
            Some("official.workforce.candidate_shortage"),
            None,
        );
    }
    Ok(())
}

fn solve_input(
    runner: &Runner,
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
) -> Result<(Value, i32)> {
    let library = paths.library;
    let input_arg = text(paths.input)?;
    let output = paths.output;
    let diagnostics = paths.diagnostics;
    absent(output)?;
    let profile = MeasurementProfile::REVIEWED;
    let budget = format!("{}ms", profile.budget_milliseconds);
    let seed = profile.random_seed.to_string();
    let threads = profile.worker_threads.to_string();
    let mode = match profile.mode {
        eutheto_types::SolveMode::Quick => "quick",
        eutheto_types::SolveMode::Balanced => "balanced",
        eutheto_types::SolveMode::Deep => "deep",
        eutheto_types::SolveMode::Custom => {
            anyhow::bail!("custom corpus effort mode is unsupported")
        }
    };
    let mut args = vec![
        "solve",
        input_arg,
        "--backend",
        "solver.ortools-cp-sat",
        "--mode",
        mode,
        "--threads",
        &threads,
        "--seed",
        &seed,
        "--max-time",
        &budget,
        "--output",
        text(output)?,
        "--include-diagnostics",
        text(diagnostics)?,
    ];
    if profile.stop_after_first_feasible {
        args.push("--first-feasible");
    }
    let (exit, envelope_ok) = match fixture.expected.disposition {
        ExpectedDisposition::Accepted => (0, true),
        // Infeasibility is a completed Outcome on stdout, not a SafeCliError on stderr.
        ExpectedDisposition::Infeasible => (4, true),
        ExpectedDisposition::CompileRejected => (3, false),
        ExpectedDisposition::ResourceLimit => (5, false),
    };
    let solved = runner.run(library, &args, exit, envelope_ok)?;
    Ok((solved, exit))
}

fn accepted_solve(
    runner: &Runner,
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
    solved: &Value,
    exit: i32,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let id = fixture.expected.id;
    let library = paths.library;
    let input_arg = text(paths.input)?;
    let directory = paths.directory;
    let output = paths.output;
    let diagnostics = paths.diagnostics;
    let status = solve_status(&solved["result"]["status"])?;
    ensure!(
        matches!(status, SolveStatus::Optimal | SolveStatus::Feasible),
        "CLI did not return an accepted solve status"
    );
    bound_result(&solved["result"], fixture)?;
    ensure!(
        solved["result"]["independentlyVerified"] == true
            && solved["result"]["resultId"].is_string(),
        "CLI solve omitted independent acceptance"
    );
    check_model(diagnostics, fixture)?;
    let bytes = read_bounded(output, MAX_ARTIFACT_BYTES)?;
    let portable = PortableAcceptedResultV2::from_json(&bytes)
        .context("CLI published an invalid accepted-result codec")?;
    let value = serde_json::to_value(&portable)?;
    ensure!(
        value["scenarioId"] == solved["result"]["scenarioId"]
            && value["scenarioRevision"] == solved["result"]["scenarioRevision"]
            && value["resultId"] == solved["result"]["resultId"]
            && value["runInput"]["runId"] == solved["result"]["runId"],
        "retained artifact differs from completed CLI run"
    );
    check_score(&value["acceptedResult"]["verification"]["score"], fixture)?;
    record(
        checks,
        id,
        "solve.compile-and-accept",
        exit,
        Some(status),
        None,
        Some(sha256(&bytes)),
    );
    accepted_consumers(runner, fixture, paths, &portable, checks)?;
    scenario_roundtrip(runner, fixture, directory, checks)?;
    if id == CaseId::RepairBefore {
        let refused = directory.join("repair-refused.json");
        let rejected = runner.run(
            library,
            &[
                "solve",
                input_arg,
                "--repair-from",
                text(output)?,
                "--output",
                text(&refused)?,
            ],
            6,
            false,
        )?;
        error_code(&rejected, "solve.operation_unavailable")?;
        absent(&refused)?;
        // A real accepted file is not a selected baseline, and there is no selection route.
        record(
            checks,
            id,
            "repair.unavailable-not-selected",
            6,
            None,
            Some("solve.operation_unavailable"),
            None,
        );
    }
    Ok(())
}

fn rejected_solve(
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
    solved: &Value,
    exit: i32,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let id = fixture.expected.id;
    let output = paths.output;
    let diagnostics = paths.diagnostics;
    match fixture.expected.disposition {
        ExpectedDisposition::Accepted => bail!("accepted solve reached rejection checks"),
        ExpectedDisposition::Infeasible => {
            bound_result(&solved["result"], fixture)?;
            ensure!(
                solved["status"] == "infeasible"
                    && solve_status(&solved["result"]["status"])? == SolveStatus::Infeasible
                    && solved["result"]["resultId"].is_null()
                    && solved["result"]["independentlyVerified"] == false
                    && solved["result"]["infeasibilityEvidence"]
                        == json!({"type":"unavailable","reason":"conflictNotReturned"}),
                "CLI did not report truthful backend infeasibility without accepted authority"
            );
            absent(output)?;
            check_model(diagnostics, fixture)?;
            record(
                checks,
                id,
                "solve.infeasible-core-unavailable",
                exit,
                Some(SolveStatus::Infeasible),
                Some("conflictNotReturned"),
                None,
            );
        }
        ExpectedDisposition::CompileRejected => {
            error_code(solved, "solve.compilation_failed")?;
            ensure!(
                solve_status(&solved["error"]["details"]["solveStatus"])?
                    == SolveStatus::InvalidModel,
                "unsupported document was not compilation-rejected"
            );
            absent(output)?;
            absent(diagnostics)?;
            record(
                checks,
                id,
                "solve.compile-rejected",
                exit,
                Some(SolveStatus::InvalidModel),
                Some("solve.compilation_failed"),
                None,
            );
        }
        ExpectedDisposition::ResourceLimit => {
            error_code(solved, "operation.resource_limit")?;
            ensure!(
                solve_status(&solved["error"]["details"]["solveStatus"])?
                    == SolveStatus::NoSolutionWithinLimit,
                "resource pressure did not preserve PR38 disposition"
            );
            absent(output)?;
            absent(diagnostics)?;
            record(
                checks,
                id,
                "solve.resource-limit",
                exit,
                Some(SolveStatus::NoSolutionWithinLimit),
                Some("operation.resource_limit"),
                None,
            );
        }
    }
    Ok(())
}

fn accepted_consumers(
    runner: &Runner,
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
    portable: &PortableAcceptedResultV2,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let library = paths.library;
    let input = paths.input;
    let output = paths.output;
    let id = fixture.expected.id;
    let source = text(input)?;
    let result = text(output)?;
    let verified = runner.run(library, &["solutions", "verify", source, result], 0, true)?;
    ensure!(
        verified["result"]["resultId"] == serde_json::to_value(portable.result_id)?
            && verified["result"]["scenarioRevision"] == fixture.snapshot.revision.value()
            && verified["result"]["verification"]["accepted"] == true
            && verified["result"]["historicalRunMetadata"] == "sourceProvidedUnverified",
        "fresh verification lost result binding or endorsed imported history"
    );
    check_score(&verified["result"]["verification"]["score"], fixture)?;
    record(checks, id, "solutions.verify-fresh", 0, None, None, None);
    let mut selected = Vec::new();
    let mut assignment_id = None;
    for assignment in &portable.accepted_result.solution.assignments {
        let (pair, present) = decode_workforce_assignment(assignment)
            .map_err(|_| anyhow::anyhow!("invalid retained Workforce assignment"))?;
        if present {
            selected.push(pair);
            assignment_id.get_or_insert(assignment.id.as_str());
        }
    }
    ensure!(
        Some(u32::try_from(selected.len())?) == fixture.expected.selected_assignments,
        "accepted schedule violates independent assignment-count expectation"
    );
    let assignment_id =
        assignment_id.context("supported corpus case has no assignment to explain")?;
    let explained = runner.run(
        library,
        &[
            "solutions",
            "explain",
            source,
            result,
            "--assignment-id",
            assignment_id,
        ],
        0,
        true,
    )?;
    let messages = explained["result"]["explanation"]["rendered"]["messages"]
        .as_array()
        .context("explanation omitted rendered messages")?;
    ensure!(
        explained["result"]["historicalRunMetadata"] == "sourceProvidedUnverified"
            && messages.iter().any(|message| message["messageKey"]
                == "official.workforce.assignment.recorded_decision"
                && message["assignments"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|value| value == assignment_id))),
        "explanation does not identify an actually accepted assignment"
    );
    record(
        checks,
        id,
        "solutions.explain-assignment",
        0,
        None,
        Some("official.workforce.assignment.recorded_decision"),
        None,
    );
    export_solutions(runner, fixture, paths, portable, &selected, checks)
}

fn export_solutions(
    runner: &Runner,
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
    portable: &PortableAcceptedResultV2,
    selected: &[AssignmentPair],
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let id = fixture.expected.id;
    let library = paths.library;
    let directory = paths.directory;
    let source = text(paths.input)?;
    let result = text(paths.output)?;
    let control = OperationControl::Cancellation(runner.cancellation.clone());
    for format in ["json", "csv"] {
        let destination = directory.join(format!("normalized.{format}"));
        let exported = runner.run(
            library,
            &[
                "solutions",
                "export",
                source,
                result,
                "--format",
                format,
                "--output",
                text(&destination)?,
            ],
            0,
            true,
        )?;
        ensure!(
            exported["result"]["resultId"] == serde_json::to_value(portable.result_id)?
                && exported["result"]["scenarioRevision"] == fixture.snapshot.revision.value()
                && exported["result"]["historicalRunMetadata"] == "sourceProvidedUnverified",
            "export lost result binding or endorsed historical metadata"
        );
        let bytes = read_bounded(&destination, MAX_ARTIFACT_BYTES)?;
        if format == "json" {
            let normalized = PortableAcceptedResultV2::from_json(&bytes)?;
            ensure!(
                normalized == *portable,
                "normalized JSON changed the accepted artifact"
            );
            let fresh = runner.run(
                library,
                &["solutions", "verify", source, text(&destination)?],
                0,
                true,
            )?;
            ensure!(
                fresh["result"]["verification"]["accepted"] == true
                    && fresh["result"]["historicalRunMetadata"] == "sourceProvidedUnverified",
                "normalized export did not freshly verify"
            );
        } else {
            let pairs = decode_assignments_csv(&mut bytes.as_slice(), &control)?;
            // Both the portable solution and CSV use canonical assignment-ID ordering.
            ensure!(
                pairs.as_slice() == selected,
                "assignment CSV changed the selected person/shift pairs"
            );
        }
        record(
            checks,
            id,
            if format == "json" {
                "solutions.export-json-verified"
            } else {
                "solutions.export-csv-conformance"
            },
            0,
            None,
            None,
            Some(sha256(&bytes)),
        );
    }
    Ok(())
}

fn scenario_roundtrip(
    runner: &Runner,
    fixture: &LoadedFixture,
    directory: &Path,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    // CLI projects import accepts bundles, not standalone scenario JSON. Assemble the validated
    // snapshot through the landed export codec, with no retained results or invented baseline.
    let seed = assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: "0195a5e4-7c00-7000-8000-000000000201".parse()?,
            created_at: FIXTURE_CLOCK.to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto Workforce corpus".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            title: fixture.snapshot.document.metadata.title.clone(),
            scenario: fixture.snapshot.clone(),
            scenario_revisions: Vec::new(),
            sections: BackupSections::default(),
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        },
        &|document| {
            eutheto_workforce::portable::export_portable(document).map_err(|_| {
                eutheto_export::ExportError::InvalidModel(
                    "Workforce corpus portable encoding failed".to_owned(),
                )
            })
        },
    )?;
    let seed_path = directory.join("seed.eutheto");
    stage(&seed_path, &seed)?;
    let id = fixture.expected.id;
    let scenario_id = fixture.snapshot.document.scenario_id.to_string();
    let library = directory.join("import-library");
    let imported = runner.run(
        &library,
        &["projects", "import", text(&seed_path)?],
        0,
        true,
    )?;
    imported_identity(&imported, &scenario_id)?;
    let stored = runner.run(&library, &["scenario", "show", &scenario_id], 0, true)?;
    ensure!(
        stored["result"]["document"] == serde_json::to_value(&fixture.snapshot.document)?,
        "stored import changed scenario semantics"
    );
    let exported_path = directory.join("roundtrip.eutheto");
    runner.run(
        &library,
        &[
            "projects",
            "export",
            &scenario_id,
            "--output",
            text(&exported_path)?,
        ],
        0,
        true,
    )?;
    let exported = read_bounded(&exported_path, MAX_ARTIFACT_BYTES)?;
    let shown = runner.run(
        &library,
        &["scenario", "show", text(&exported_path)?],
        0,
        true,
    )?;
    ensure!(
        shown["result"]["document"] == stored["result"]["document"]
            && shown["result"]["revision"] == stored["result"]["revision"],
        "bundle export changed persisted scenario binding"
    );
    let second_library = directory.join("roundtrip-library");
    let imported = runner.run(
        &second_library,
        &["projects", "import", text(&exported_path)?],
        0,
        true,
    )?;
    imported_identity(&imported, &scenario_id)?;
    let roundtrip = runner.run(
        &second_library,
        &["scenario", "show", &scenario_id],
        0,
        true,
    )?;
    ensure!(
        roundtrip["result"]["document"] == stored["result"]["document"],
        "scenario import/export roundtrip changed original-domain content"
    );
    record(
        checks,
        id,
        "projects.import-export-roundtrip",
        0,
        None,
        None,
        Some(sha256(&exported)),
    );
    Ok(())
}

fn imported_identity(envelope: &Value, scenario_id: &str) -> Result<()> {
    ensure!(
        envelope["status"] == "applied"
            && envelope["result"]["scenarioIds"] == json!([scenario_id])
            && envelope["result"]["scenarioOutcomes"][0]["sourceScenarioId"] == scenario_id
            && envelope["result"]["scenarioOutcomes"][0]["scenarioId"] == scenario_id
            && envelope["result"]["scenarioOutcomes"][0]["selectedAction"] == "same-identity",
        "private import did not preserve scenario identity"
    );
    Ok(())
}

fn reject_dst_policy(
    runner: &Runner,
    fixture: &LoadedFixture,
    directory: &Path,
    library: &Path,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let mut wire: Value = serde_json::from_slice(&fixture.bytes)?;
    let key = if fixture.expected.id == CaseId::DstSpring {
        "gapPolicy"
    } else {
        "overlapPolicy"
    };
    ensure!(
        wire["settings"][key].is_string(),
        "DST portable input omitted explicit resolver policy"
    );
    wire["settings"][key] = json!("reject");
    let path = directory.join("dst-reject.json");
    stage(&path, &serde_json::to_vec(&wire)?)?;
    // No checksum rewrite: these are standalone portable bytes. Showing them first proves the
    // outer codec and domain shape succeeded; full validation then reaches temporal generation.
    runner.run(library, &["scenario", "show", text(&path)?], 0, true)?;
    let rejected = runner.run(library, &["scenario", "validate", text(&path)?], 3, true)?;
    ensure!(
        rejected["status"] == "invalid"
            && rejected["result"]["issues"]
                .as_array()
                .is_some_and(|issues| issues.iter().any(|issue| issue["code"]
                    == "official.workforce.temporal_review"
                    && issue["severity"] == "error")),
        "explicit DST rejection did not reach temporal domain validation"
    );
    record(
        checks,
        fixture.expected.id,
        "scenario.dst-policy-rejected",
        3,
        None,
        Some("official.workforce.temporal_review"),
        None,
    );
    Ok(())
}

fn check_score(score: &Value, fixture: &LoadedFixture) -> Result<()> {
    let expected = fixture
        .expected
        .score
        .context("accepted corpus case omitted independent score expectation")?;
    let levels = score["levels"]
        .as_array()
        .context("accepted score omitted levels")?;
    ensure!(
        score["feasibility"].as_i64() == Some(expected.feasibility)
            && levels.len() == 1
            && levels[0]["levelId"] == "official.workforce.objective.assignment.rank"
            && levels[0]["direction"] == "minimize",
        "accepted score changed its independently expected meaning"
    );
    let rank = levels[0]["value"]
        .as_i64()
        .context("accepted rank is not an integer")?;
    ensure!(
        (expected.minimum_stable_rank..=expected.maximum_stable_rank).contains(&rank),
        "accepted rank violates reviewed independent expectation"
    );
    Ok(())
}

fn check_model(path: &Path, fixture: &LoadedFixture) -> Result<()> {
    let bytes = read_bounded(path, MAX_PROCESS_BYTES)?;
    let diagnostics: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        diagnostics["apiVersion"] == "eutheto/cli-result/v1"
            && diagnostics["command"] == "solve.diagnostics"
            && diagnostics["ok"] == true
            && diagnostics["result"]["backendId"] == "solver.ortools-cp-sat",
        "solve omitted actual compiler diagnostics"
    );
    let model = &diagnostics["result"]["model"];
    let variables = model["variableCount"]
        .as_u64()
        .context("compilation diagnostics omitted variables")?;
    let constraints = model["constraintCount"]
        .as_u64()
        .context("compilation diagnostics omitted constraints")?;
    if let Some(envelope) = fixture.expected.model_envelope {
        ensure!(
            (u64::from(envelope.minimum_variables)..=u64::from(envelope.maximum_variables))
                .contains(&variables)
                && constraints <= u64::from(envelope.maximum_constraints),
            "CLI compilation exceeded independently reviewed model envelope"
        );
    }
    Ok(())
}

fn bound_result(result: &Value, fixture: &LoadedFixture) -> Result<()> {
    ensure!(
        result["scenarioId"] == serde_json::to_value(fixture.snapshot.document.scenario_id)?
            && result["scenarioRevision"] == fixture.snapshot.revision.value()
            && result["backendId"] == "solver.ortools-cp-sat"
            && result["runId"].is_string(),
        "CLI solve used a different input or backend"
    );
    Ok(())
}

fn solve_status(value: &Value) -> Result<SolveStatus> {
    serde_json::from_value(value.clone()).context("CLI omitted a supported solve status")
}

fn error_code(envelope: &Value, code: &str) -> Result<()> {
    ensure!(
        envelope["error"]["code"] == code && envelope["result"].is_null(),
        "CLI rejection did not match the reviewed safe code"
    );
    Ok(())
}

fn record(
    checks: &mut Vec<CliCheck>,
    case_id: CaseId,
    operation: &'static str,
    exit_code: i32,
    observed_status: Option<SolveStatus>,
    observed_code: Option<&'static str>,
    artifact_sha256: Option<String>,
) {
    checks.push(CliCheck {
        case_id,
        operation: operation.to_owned(),
        exit_code,
        observed_status,
        observed_code: observed_code.map(str::to_owned),
        artifact_sha256,
    });
}

fn stage(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        u64::try_from(bytes.len())? <= MAX_ARTIFACT_BYTES,
        "CLI staged input exceeds its bound"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| anyhow::anyhow!("private CLI staging failed"))?;
    file.write_all(bytes)
        .map_err(|_| anyhow::anyhow!("private CLI staging write failed"))?;
    Ok(())
}

fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => bail!("negative CLI solve unexpectedly published an artifact"),
    }
}

fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("private CLI path cannot be represented as an argument")
}

struct Runner {
    cancellation: CancellationToken,
    executable: PathBuf,
    root: PathBuf,
}

impl Runner {
    fn run(
        &self,
        library: &Path,
        args: &[&str],
        expected_exit: i32,
        envelope_ok: bool,
    ) -> Result<Value> {
        ensure!(!self.cancellation.is_cancelled(), "CLI command cancelled");
        let mut command = Command::new(&self.executable);
        command
            .args(["--format", "json", "--offline", "--data-dir"])
            .arg(library)
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("USERPROFILE", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("home"))
            .env("TMPDIR", &self.root)
            .env("TMP", &self.root)
            .env("TEMP", &self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Windows system DLL loading may require this OS-owned setting; never retain it.
        #[cfg(windows)]
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = ChildGuard::spawn(command)?;
        let result = capture(&mut child, &self.cancellation)
            .and_then(|captured| captured.envelope(expected_exit, envelope_ok));
        match result {
            Ok(envelope) => {
                child.finish()?;
                Ok(envelope)
            }
            Err(error) => {
                child.cleanup()?;
                Err(error)
            }
        }
    }
}

struct Captured {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Captured {
    fn envelope(self, expected_exit: i32, envelope_ok: bool) -> Result<Value> {
        ensure!(
            self.status.code() == Some(expected_exit),
            "CLI command returned an unexpected process disposition"
        );
        let bytes = if envelope_ok {
            ensure!(
                self.stderr.is_empty(),
                "completed CLI command emitted unexpected stderr"
            );
            self.stdout
        } else {
            ensure!(
                self.stdout.is_empty(),
                "rejected CLI command emitted accepted stdout"
            );
            self.stderr
        };
        let envelope: Value = serde_json::from_slice(&bytes)
            .context("CLI output is not one bounded JSON envelope")?;
        ensure!(
            envelope["apiVersion"] == "eutheto/cli-result/v1" && envelope["ok"] == envelope_ok,
            "CLI envelope disagrees with its observed disposition"
        );
        // No raw response, warnings, prose, diagnostic IDs, arguments, paths or environment enter evidence.
        Ok(envelope)
    }
}

fn capture(child: &mut ChildGuard, cancellation: &CancellationToken) -> Result<Captured> {
    let stdout = child
        .child_mut()?
        .stdout()
        .take()
        .context("CLI stdout pipe missing")?;
    let stderr = child
        .child_mut()?
        .stderr()
        .take()
        .context("CLI stderr pipe missing")?;
    let (sender, receiver) = mpsc::channel();
    for (index, stream) in [
        (0, Box::new(stdout) as Box<dyn Read + Send>),
        (1, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let sender = sender.clone();
        // Never join a pipe potentially inherited by an uncontained Unix descendant.
        thread::Builder::new()
            .spawn(move || {
                let mut bytes = Vec::new();
                let result = stream
                    .take(MAX_PROCESS_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes);
                let _ = sender.send((index, result));
            })
            .map_err(|_| anyhow::anyhow!("CLI capture reader could not be started"))?;
    }
    drop(sender);
    let deadline = Instant::now() + COMMAND_GUARD;
    let mut streams: [Option<Vec<u8>>; 2] = [None, None];
    loop {
        ensure!(!cancellation.is_cancelled(), "CLI command cancelled");
        while let Ok((index, result)) = receiver.try_recv() {
            let bytes = result.map_err(|_| anyhow::anyhow!("CLI bounded capture failed"))?;
            ensure!(
                u64::try_from(bytes.len())? <= MAX_PROCESS_BYTES,
                "CLI output exceeded its capture bound"
            );
            streams[index] = Some(bytes);
        }
        if let Some(status) = child.poll()? {
            ensure!(
                controlled_exit(status),
                "CLI wrapper did not exit through its controlled completion path"
            );
            if streams.iter().all(Option::is_some) {
                let [stdout, stderr] = streams;
                return Ok(Captured {
                    status,
                    stdout: stdout.context("CLI stdout capture missing")?,
                    stderr: stderr.context("CLI stderr capture missing")?,
                });
            }
        }
        ensure!(
            Instant::now() < deadline,
            "CLI command exceeded finite supervisory guard"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn controlled_exit(status: ExitStatus) -> bool {
    // Stable CliExitCode contract: ordinary results 0..=10 and cancellation 130.
    // Rust's panic exit (101) is not evidence of completed worker cleanup.
    matches!(status.code(), Some(0..=10 | 130))
}

struct ChildGuard {
    child: Option<Box<dyn ChildWrapper>>,
    status: Option<ExitStatus>,
    settled: bool,
}

impl ChildGuard {
    fn spawn(command: Command) -> Result<Self> {
        let mut command = CommandWrap::from(command);
        #[cfg(unix)]
        command.wrap(ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(JobObject);
        let child = command
            .spawn()
            .map_err(|_| anyhow::anyhow!("supplied optimizer could not be started"))?;
        Ok(Self {
            child: Some(child),
            status: None,
            settled: false,
        })
    }

    fn child_mut(&mut self) -> Result<&mut Box<dyn ChildWrapper>> {
        self.child
            .as_mut()
            .context("CLI child custody was already transferred")
    }

    fn poll(&mut self) -> Result<Option<ExitStatus>> {
        if self.status.is_none() {
            // Windows root polling must leave job-completion events for the cleanup wait.
            #[cfg(windows)]
            let child = self.child_mut()?.inner_mut();
            #[cfg(not(windows))]
            let child = self.child_mut()?.as_mut();
            self.status = child
                .try_wait()
                .map_err(|_| anyhow::anyhow!("CLI supervisory wait failed"))?;
        }
        Ok(self.status)
    }

    fn finish(&mut self) -> Result<()> {
        ensure!(
            self.status.is_some_and(controlled_exit),
            "CLI wrapper completion missing; worker cleanup unconfirmed"
        );
        // The std job is not kill-on-close. Even a normally exited root may leave descendants.
        #[cfg(windows)]
        self.cleanup()?;
        self.settled = true;
        Ok(())
    }

    #[cfg(unix)]
    fn cleanup(&mut self) -> Result<()> {
        // The production worker has its own PGID. SIGINT asks the wrapper to cancel and
        // await its worker; killing our process group alone cannot establish worker cleanup.
        if self.status.is_none() {
            let _ = self.child_mut()?.signal(2); // POSIX SIGINT, through the safe wrapper API.
            let deadline = Instant::now() + CLEANUP_GRACE;
            loop {
                if let Ok(Some(status)) = self.poll() {
                    self.settled = true;
                    ensure!(
                        controlled_exit(status),
                        "CLI wrapper aborted; worker cleanup unconfirmed"
                    );
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    self.force_wrapper_cleanup();
                    bail!("CLI cancellation grace expired; worker cleanup unconfirmed");
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        self.settled = true;
        ensure!(
            self.status.is_some_and(controlled_exit),
            "CLI wrapper aborted; worker cleanup unconfirmed"
        );
        Ok(())
    }

    #[cfg(windows)]
    fn cleanup(&mut self) -> Result<()> {
        // Terminate the owned job even after root exit. The pinned std wrapper's
        // wait has no deadline, so retain its handles in a waiter with bounded observation.
        let mut child = self
            .child
            .take()
            .context("CLI job custody already transferred")?;
        if child.start_kill().is_err() {
            self.child = Some(child);
            bail!("CLI owned-job termination failed; worker cleanup unconfirmed");
        }
        self.settled = true;
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .spawn(move || {
                let _ = sender.send(child.wait());
            })
            .map_err(|_| anyhow::anyhow!("CLI job wait unavailable; worker cleanup unconfirmed"))?;
        ensure!(
            matches!(receiver.recv_timeout(CLEANUP_GRACE), Ok(Ok(_))),
            "CLI owned-job wait failed or expired; worker cleanup unconfirmed"
        );
        Ok(())
    }

    #[cfg(unix)]
    fn force_wrapper_cleanup(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        // A reaped wrapper's PGID may be reused; do not signal it after completion.
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.start_kill();
            let deadline = Instant::now() + CLEANUP_GRACE;
            while !matches!(child.try_wait(), Ok(Some(_))) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        }
        self.settled = true;
        // Best effort only: independently grouped workers may survive a crashed wrapper.
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.cleanup();
            #[cfg(unix)]
            if !self.settled {
                self.force_wrapper_cleanup();
            }
        }
    }
}
