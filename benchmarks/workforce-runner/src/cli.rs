//! Consumer assertions against an already assembled, manifest-bound optimizer image.
//! Nothing in this module grants imported schedules or solver history authority.

use crate::contract::{
    CLI_EVIDENCE_FORMAT, CORPUS_VERSION, CaseId, CliCheck, CliEvidence, ExpectedDisposition,
    FIXTURE_CLOCK, FORMAT_VERSION, LoadedCorpus, LoadedFixture, MAX_EXPECTED_BYTES,
    MeasurementProfile, REPAIR_PATH, RepairFixture,
};
use crate::files::{hash_file, read_bounded, sha256};
use anyhow::{Context, Result, bail, ensure};
use eutheto_domain_ir::{
    AcceptedResultRefV1, OptimizationDirection, PortableAcceptedResult, RunBaselineSelection,
    RunObjectiveOperation,
};
use eutheto_export::{
    ApplicationMetadata, BackupSections, ScenarioExportSnapshot, assemble_scenario_export,
};
use eutheto_types::{
    ActorRef, CancellationToken, CommandEnvelope, CommandSource, DomainCommandEnvelope,
    OperationControl, PersonId, ScenarioCommand, ScenarioSnapshotV1, SolveStatus,
};
use eutheto_workforce::{
    assignment_rules::decode_workforce_assignment,
    assignments_csv::decode_assignments_csv,
    ids::ShiftId,
    model::{AssignmentLock, AssignmentPair, BaseSchedule, LockState, TimeWindow, WorkforceEntity},
    temporal,
};
#[cfg(windows)]
use process_wrap::std::JobObject;
#[cfg(unix)]
use process_wrap::std::ProcessGroup;
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde::Deserialize;
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
    repository: &Path,
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
    let recipe: RepairFixture = serde_json::from_slice(&crate::generation::bound_bytes(
        repository,
        &corpus.manifest.repair,
        REPAIR_PATH,
        MAX_EXPECTED_BYTES,
    )?)?;
    for fixture in &corpus.fixtures {
        exercise_case(&runner, fixture, &mut checks)?;
        if fixture.expected.id == CaseId::PractitionerWorkflow {
            exercise_repair(&runner, fixture, &recipe, &mut checks)?;
        }
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
    // Appendix-F passes local readiness but has a cross-rule rest/absence conflict.
    if fixture.expected.disposition == ExpectedDisposition::Accepted || id == CaseId::AppendixF {
        let valid = runner.run(library, &["scenario", "validate", input_arg], 0, true)?;
        ensure!(
            valid["status"] == "valid"
                && valid["result"]["revision"] == fixture.snapshot.revision.value(),
            "CLI validation did not validate the corpus revision"
        );
        record(checks, id, "scenario.validate", 0, None, None, None);
    }
    if matches!(
        id,
        CaseId::InfeasibleCoverage | CaseId::PractitionerInfeasible
    ) {
        let code = if id == CaseId::PractitionerInfeasible {
            "official.workforce.qualification_shortage"
        } else {
            "official.workforce.candidate_shortage"
        };
        let readiness = runner.run(library, &["scenario", "validate", input_arg], 3, true)?;
        ensure!(
            readiness["status"] == "invalid"
                && readiness["result"]["revision"] == fixture.snapshot.revision.value()
                && readiness["result"]["issues"]
                    .as_array()
                    .is_some_and(|issues| issues
                        .iter()
                        .any(|issue| issue["code"] == code && issue["severity"] == "error")),
            "insufficient eligible coverage did not preserve its readiness finding"
        );
        // A readiness finding is not solver proof; the independent solve below must dispatch.
        record(
            checks,
            id,
            "scenario.validate-readiness-shortage",
            3,
            None,
            Some(code),
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
    let portable = PortableAcceptedResult::from_json(&bytes)
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
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn exercise_repair(
    runner: &Runner,
    fixture: &LoadedFixture,
    recipe: &RepairFixture,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    crate::cases::validate_repair(recipe, &fixture.snapshot)?;
    let directory = runner.root.join(fixture.expected.id.slug());
    let library = directory.join("library");
    let original_path = directory.join("accepted.json");
    let original_bytes = read_bounded(&original_path, MAX_ARTIFACT_BYTES)?;
    let original = PortableAcceptedResult::from_json(&original_bytes)?;
    let TimeWindow::Instant { starts_at, ends_at } = &recipe.time_window else {
        bail!("repair recipe omitted the exact call-out interval")
    };
    let shifts = temporal::resolve_shifts(&fixture.snapshot.document, &runner.cancellation)?;
    let affected: Vec<_> = shifts
        .iter()
        .filter(|shift| {
            shift.interval.starts_at.instant < *ends_at
                && shift.interval.ends_at.instant > *starts_at
        })
        .collect();
    ensure!(
        affected.len() == 1
            && affected[0].id == recipe.target_shift_id
            && affected[0].reporting_date.to_string() == "2027-02-10",
        "call-out must affect exactly the authored February 10 clinic"
    );
    let mut protected = BTreeSet::new();
    for value in fixture.snapshot.document.domain.locked_assignments.values() {
        let lock = AssignmentLock::deserialize(value)?;
        if matches!(lock.state, LockState::Hard {}) {
            protected.insert((lock.person_id, lock.shift_id));
        }
    }
    let original_pairs = selected_pairs(&original)?;
    let unavailable_pair = movable_incumbent(&original_pairs, &protected, recipe.target_shift_id)
        .context("accepted February 10 clinic has no movable incumbent")?;
    let minimum_change_cost =
        callout_change_lower_bound(&original_pairs, &shifts, unavailable_pair)?;
    let payload = crate::cases::repair_payload(recipe, unavailable_pair.0);
    let WorkforceEntity::Availability(callout) = &payload.entity else {
        bail!("repair payload omitted the call-out availability")
    };
    let service = crate::generation::codec()?;
    let command = CommandEnvelope {
        command_id: "0195a5e4-7c00-7000-8000-000000000202".parse()?,
        scenario_id: fixture.snapshot.document.scenario_id,
        expected_revision: fixture.snapshot.revision,
        actor: ActorRef {
            actor_id: None,
            display_name: "Workforce CLI repair corpus".to_owned(),
        },
        source: CommandSource::Cli,
        command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
            command_type: recipe.command_id.clone(),
            payload: serde_json::to_value(&payload)?,
        }),
    };
    let ScenarioCommand::ApplyDomainCommand(domain_command) = &command.command else {
        bail!("repair command omitted its domain payload")
    };
    let expected_entity = domain_command
        .payload
        .get("entity")
        .context("repair command omitted its availability entity")?;
    let applied = crate::generation::app(service.apply_scenario(
        fixture.snapshot.clone(),
        &command,
        &runner.cancellation,
    ))?;
    let changed = applied.snapshot;
    ensure!(
        changed.revision == fixture.snapshot.revision.checked_next()?
            && changed.revision == applied.result.new_revision,
        "call-out command did not create an actual new scenario revision"
    );
    let mut restored = changed.document.clone();
    ensure!(
        restored
            .domain
            .entities
            .remove(&callout.id.as_entity_id())
            .as_ref()
            == Some(expected_entity)
            && restored == fixture.snapshot.document,
        "call-out command changed more than the single availability record"
    );
    let changed_path = directory.join("callout.json");
    let changed_bytes = crate::generation::app(service.encode_scenario(
        &changed,
        &OperationControl::Cancellation(runner.cancellation.clone()),
    ))?;
    stage(&changed_path, &changed_bytes)?;
    let bundle = assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: "0195a5e4-7c00-7000-8000-000000000203".parse()?,
            created_at: FIXTURE_CLOCK.to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto Workforce CLI repair corpus".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            title: changed.document.metadata.title.clone(),
            scenario: changed.clone(),
            scenario_revisions: vec![fixture.snapshot.clone()],
            sections: BackupSections {
                results: BTreeMap::from([(
                    original.result_id.to_string(),
                    serde_json::to_value(&original)?,
                )]),
                ..BackupSections::default()
            },
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        },
        &|document| {
            eutheto_workforce::portable::export_portable(document).map_err(|_| {
                eutheto_export::ExportError::InvalidModel(
                    "Workforce repair portable encoding failed".to_owned(),
                )
            })
        },
    )?;
    let bundle_path = directory.join("callout.eutheto");
    stage(&bundle_path, &bundle)?;
    let original_id = original.result_id.to_string();
    let budget = format!("{}ms", MeasurementProfile::REVIEWED.budget_milliseconds);
    let repaired_path = directory.join("repaired-file.json");
    for (source, output, operation) in [
        (
            original_id.as_str(),
            directory.join("repaired-id.json"),
            "repair.changed-callout-id",
        ),
        (
            text(&original_path)?,
            repaired_path.clone(),
            "repair.changed-callout-file",
        ),
    ] {
        absent(&output)?;
        let solved = runner.run(
            &library,
            &[
                "solve",
                text(&bundle_path)?,
                "--repair-from",
                source,
                "--backend",
                "solver.ortools-cp-sat",
                "--mode",
                "deep",
                "--threads",
                "1",
                "--seed",
                "1",
                "--max-time",
                &budget,
                "--output",
                text(&output)?,
            ],
            0,
            true,
        )?;
        let status = solve_status(&solved["result"]["status"])?;
        ensure!(
            matches!(status, SolveStatus::Optimal | SolveStatus::Feasible)
                && solved["result"]["independentlyVerified"] == true,
            "repair did not complete genuine worker independent acceptance"
        );
        let bytes = read_bounded(&output, MAX_ARTIFACT_BYTES)?;
        let repaired = PortableAcceptedResult::from_json(&bytes)?;
        ensure!(
            solved["result"]["resultId"] == serde_json::to_value(repaired.result_id)?
                && solved["result"]["runId"] == serde_json::to_value(repaired.run_input.run_id)?,
            "repaired artifact differs from the completed CLI run"
        );
        check_repaired(
            &original,
            &repaired,
            &changed,
            unavailable_pair,
            minimum_change_cost,
            status,
        )?;
        // Standalone unchanged current input suffices: C carries its run-only captured baseline.
        let verified = runner.run(
            &library,
            &["solutions", "verify", text(&changed_path)?, text(&output)?],
            0,
            true,
        )?;
        ensure!(
            verified["result"]["resultId"] == serde_json::to_value(repaired.result_id)?
                && verified["result"]["scenarioRevision"] == changed.revision.value()
                && verified["result"]["verification"]["accepted"] == true
                && verified["result"]["verification"]["score"]
                    == serde_json::to_value(&repaired.accepted_result.verification.score)?
                && verified["result"]["historicalRunMetadata"] == "sourceProvidedUnverified",
            "fresh repair verification changed costs, source binding or imported-history authority"
        );
        record(
            checks,
            fixture.expected.id,
            operation,
            0,
            Some(status),
            None,
            Some(sha256(&bytes)),
        );
        record(
            checks,
            fixture.expected.id,
            "repair.verify-unchanged-current-input",
            0,
            None,
            None,
            None,
        );
    }
    let preserved_output = hash_file(&repaired_path, MAX_ARTIFACT_BYTES)?;
    let rejected = runner.run(
        &library,
        &[
            "solve",
            text(&changed_path)?,
            "--repair-from",
            text(&original_path)?,
            "--output",
            text(&repaired_path)?,
        ],
        3,
        false,
    )?;
    error_code(&rejected, "solve.repair_original_revision_required")?;
    ensure!(
        hash_file(&repaired_path, MAX_ARTIFACT_BYTES)? == preserved_output,
        "missing-original repair replaced the prior accepted output"
    );
    record(
        checks,
        fixture.expected.id,
        "repair.missing-original-preserves-output",
        3,
        None,
        Some("solve.repair_original_revision_required"),
        Some(preserved_output),
    );
    for (path, bytes) in [
        (directory.join("scenario.json"), fixture.bytes.as_slice()),
        (original_path, original_bytes.as_slice()),
        (changed_path, changed_bytes.as_slice()),
        (bundle_path, bundle.as_slice()),
    ] {
        ensure!(
            hash_file(&path, MAX_ARTIFACT_BYTES)? == sha256(bytes),
            "repair or reverification changed an immutable source file"
        );
    }
    ensure!(!library.exists(), "headless repair opened a stored library");
    Ok(())
}

fn movable_incumbent(
    selected: &BTreeSet<(PersonId, ShiftId)>,
    protected: &BTreeSet<(PersonId, ShiftId)>,
    target: ShiftId,
) -> Option<(PersonId, ShiftId)> {
    selected
        .iter()
        .copied()
        .find(|pair| pair.1 == target && !protected.contains(pair))
}

fn selected_pairs(portable: &PortableAcceptedResult) -> Result<BTreeSet<(PersonId, ShiftId)>> {
    let mut selected = BTreeSet::new();
    for assignment in &portable.accepted_result.solution.assignments {
        let (pair, present) = decode_workforce_assignment(assignment)
            .map_err(|_| anyhow::anyhow!("invalid retained Workforce assignment"))?;
        if present {
            ensure!(
                selected.insert((pair.person_id, pair.shift_id)),
                "retained result contains a duplicate selected pair"
            );
        }
    }
    Ok(selected)
}

// The admitted fixture requires at least one assignment per person in this reporting
// week and exactly68 selected pairs overall. Losing a person's sole weekly assignment
// requires a second addition beyond the clinic replacement, hence two removals too.
fn callout_change_lower_bound(
    selected: &BTreeSet<(PersonId, ShiftId)>,
    shifts: &[temporal::ResolvedShift],
    unavailable: (PersonId, ShiftId),
) -> Result<i64> {
    let start = "2027-02-08".parse::<jiff::civil::Date>()?;
    let end = "2027-02-15".parse::<jiff::civil::Date>()?;
    let mut assignments = 0;
    let mut target_in_week = false;
    for pair in selected {
        let shift = shifts
            .iter()
            .find(|shift| shift.id == pair.1)
            .context("accepted assignment has no native resolved shift")?;
        if pair.0 == unavailable.0 && shift.reporting_date >= start && shift.reporting_date < end {
            assignments += 1;
            target_in_week |= *pair == unavailable;
        }
    }
    ensure!(
        target_in_week,
        "call-out is not selected in its required reporting week"
    );
    Ok(if assignments == 1 { 4 } else { 2 })
}

fn minimum_change_proved(cost: i64, lower_bound: i64, status: SolveStatus) -> bool {
    cost >= lower_bound && (cost == lower_bound || status == SolveStatus::Optimal)
}

fn check_repaired(
    original: &PortableAcceptedResult,
    repaired: &PortableAcceptedResult,
    changed: &ScenarioSnapshotV1,
    unavailable_pair: (PersonId, ShiftId),
    minimum_change_cost: i64,
    status: SolveStatus,
) -> Result<()> {
    let context = repaired
        .run_input
        .objective_context
        .as_ref()
        .context("repair omitted its recorded objective context")?;
    let binding = context
        .baseline
        .as_ref()
        .context("repair omitted its original baseline")?;
    ensure!(
        context.schema_version == 2
            && context.profile_id.as_str() == "official.workforce.objective.minimal-changes"
            && context.operation == RunObjectiveOperation::Repair
            && matches!(binding.selection, RunBaselineSelection::OriginalAccepted {})
            && binding.original.accepted_result
                == AcceptedResultRefV1::from_result(&original.accepted_result)?
            && binding.original.source_scenario_id == original.scenario_id
            && binding.original.source_revision == original.scenario_revision
            && binding.original.source_document_hash == original.run_input.snapshot_document_hash
            && binding.original.source_run_id == original.run_input.run_id
            && binding.original.source_run_input_checksum == original.run_input.checksum
            && binding.original.source_run_manifest_checksum == original.run_manifest.checksum
            && binding.selected_pair_content_hash == binding.original.pair_content_hash
            && repaired.scenario_id == changed.document.scenario_id
            && repaired.scenario_revision == changed.revision.value(),
        "repair lost exact original ancestry, captured semantics or current revision"
    );
    let captured = WorkforceEntity::deserialize(
        binding
            .captured_original
            .as_ref()
            .context("repair omitted its run-only baseline")?
            .as_ref(),
    )?;
    let WorkforceEntity::BaseSchedule(BaseSchedule {
        id,
        source_solution_id,
        source_revision,
        assignments,
    }) = captured
    else {
        bail!("repair captured something other than the closed original base record")
    };
    let before = selected_pairs(original)?;
    let captured_pairs: BTreeSet<_> = assignments
        .iter()
        .map(|pair| (pair.person_id, pair.shift_id))
        .collect();
    ensure!(
        source_solution_id == original.result_id
            && source_revision.value() == original.scenario_revision
            && captured_pairs == before
            && assignments.len() == before.len()
            && binding.original.entity
                == eutheto_workforce::assignment_rules::workforce_baseline_reference(id, false)
                    .map_err(|_| anyhow::anyhow!("repair captured an invalid baseline identity"))?
            && !changed.document.domain.entities.contains_key(&id),
        "repair changed or durably adopted the complete original baseline"
    );
    let after = selected_pairs(repaired)?;
    let change_cost = i64::try_from(before.symmetric_difference(&after).count())?;
    // A feasible result attaining the source-derived lower bound proves minimum
    // pair cost, without asserting optimality of its later objective levels.
    ensure!(
        before.len() == 68
            && after.len() == before.len()
            && !after.contains(&unavailable_pair)
            && after
                .difference(&before)
                .any(|pair| pair.1 == unavailable_pair.1)
            && minimum_change_proved(change_cost, minimum_change_cost, status),
        "repair did not replace the unavailable pair with demonstrated minimum pair cost"
    );
    let score = &repaired.accepted_result.verification.score;
    let levels = &score.levels;
    // Authored and Minimal-changes have different layouts: named profiles omit
    // unmapped authored tiers. The caller freshly re-verifies the complete profile-specific score.
    ensure!(
        score.feasibility == 0
            && levels.len() >= 3
            && levels[0].level_id.as_str() == "official.workforce.objective.pair_change"
            && levels[0].value == change_cost
            && levels[1].level_id.as_str() == "official.workforce.objective.soft_lock"
            && levels[1].value == 0
            && levels.last().is_some_and(|level| {
                level.level_id.as_str() == "official.workforce.objective.assignment.rank"
                    && level.direction == OptimizationDirection::Minimize
            })
            && levels
                .iter()
                .take(2)
                .all(|level| level.direction == OptimizationDirection::Minimize),
        "repair score lost pair-change/Soft/rank ordering or exact independently counted costs"
    );
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
                    && solved["result"]["infeasibilityEvidence"]["type"] == "unavailable",
                "CLI did not report truthful backend infeasibility without accepted authority"
            );
            let unavailable_reason = solved["result"]["infeasibilityEvidence"]["reason"]
                .as_str()
                .context("CLI infeasibility evidence omitted its unavailable reason")?;
            absent(output)?;
            check_model(diagnostics, fixture)?;
            record(
                checks,
                id,
                "solve.infeasible-core-unavailable",
                exit,
                Some(SolveStatus::Infeasible),
                Some(unavailable_reason),
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
    portable: &PortableAcceptedResult,
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
            && verified["result"]["verification"]["score"]
                == serde_json::to_value(&portable.accepted_result.verification.score)?
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
    portable: &PortableAcceptedResult,
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
            let normalized = PortableAcceptedResult::from_json(&bytes)?;
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
    if matches!(id, CaseId::PractitionerWorkflow | CaseId::LargeSupported) {
        export_recipient_pdf(runner, fixture, paths, portable, checks)?;
    }
    Ok(())
}

// Exercise the actual packaged helper through the public CLI. This checks byte
// publication and source binding; independent PDF text/layout inspection is a
// separate acceptance gate, not something a file signature can establish.
fn export_recipient_pdf(
    runner: &Runner,
    fixture: &LoadedFixture,
    paths: &CasePaths<'_>,
    portable: &PortableAcceptedResult,
    checks: &mut Vec<CliCheck>,
) -> Result<()> {
    let destination = paths.directory.join("recipient.pdf");
    let exported = runner.run(
        paths.library,
        &[
            "solutions",
            "export",
            text(paths.input)?,
            text(paths.output)?,
            "--format",
            "pdf",
            "--output",
            text(&destination)?,
        ],
        0,
        true,
    )?;
    let result = &exported["result"];
    ensure!(
        result["resultId"] == serde_json::to_value(portable.result_id)?
            && result["scenarioRevision"] == fixture.snapshot.revision.value()
            && result["historicalRunMetadata"] == "sourceProvidedUnverified"
            && result["format"] == "pdf",
        "recipient PDF lost verified source binding or endorsed historical metadata"
    );
    let bytes = read_bounded(
        &destination,
        u64::try_from(eutheto_types::MAX_RECIPIENT_PDF_BYTES)?,
    )?;
    let digest = sha256(&bytes);
    ensure!(
        bytes.starts_with(b"%PDF-") && result["pdfSha256"] == digest,
        "recipient PDF publication differs from its exact-byte receipt"
    );
    record(
        checks,
        fixture.expected.id,
        "solutions.export-pdf-exact-byte-publication",
        0,
        None,
        None,
        Some(digest),
    );
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
    let rank_level = levels.last().context("accepted score omitted final rank")?;
    ensure!(
        score["feasibility"].as_i64() == Some(expected.feasibility)
            && rank_level["levelId"] == "official.workforce.objective.assignment.rank"
            && rank_level["direction"] == "minimize",
        "accepted score changed its independently expected meaning"
    );
    let rank = rank_level["value"]
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
    observed_code: Option<&str>,
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

#[cfg(test)]
mod tests {
    use super::{
        BTreeSet, Context, PersonId, Result, ShiftId, SolveStatus, callout_change_lower_bound,
        minimum_change_proved, movable_incumbent,
    };

    #[test]
    fn callout_selects_only_a_movable_native_pair_in_stable_order() -> Result<()> {
        let first: PersonId = "0195a5e4-7c00-7000-8000-000000000001".parse()?;
        let second: PersonId = "0195a5e4-7c00-7000-8000-000000000002".parse()?;
        let target: ShiftId = "0195a5e4-7c00-7000-8000-000000000003".parse()?;
        let other: ShiftId = "0195a5e4-7c00-7000-8000-000000000004".parse()?;
        let selected = BTreeSet::from([(second, target), (first, other), (first, target)]);
        let mut protected = BTreeSet::new();
        assert_eq!(
            movable_incumbent(&selected, &protected, target),
            Some((first, target))
        );
        protected.insert((first, other));
        assert_eq!(
            movable_incumbent(&selected, &protected, target),
            Some((first, target))
        );
        protected.insert((first, target));
        assert_eq!(
            movable_incumbent(&selected, &protected, target),
            Some((second, target))
        );
        protected.insert((second, target));
        assert_eq!(movable_incumbent(&selected, &protected, target), None);
        assert_eq!(
            movable_incumbent(&BTreeSet::new(), &protected, target),
            None
        );
        Ok(())
    }

    #[test]
    fn callout_bound_uses_complete_native_reporting_week() -> Result<()> {
        let definitions = crate::cases::build()?;
        let fixture = definitions
            .fixtures
            .iter()
            .find(|fixture| fixture.id == crate::contract::CaseId::PractitionerWorkflow)
            .context("missing practitioner fixture")?;
        let shifts = eutheto_workforce::temporal::resolve_shifts(
            &fixture.snapshot.document,
            &eutheto_types::CancellationToken::new(),
        )?;
        let person: PersonId = "0195a5e4-7c00-7000-8000-000000000001".parse()?;
        let other: PersonId = "0195a5e4-7c00-7000-8000-000000000002".parse()?;
        let unavailable = (person, definitions.repair.target_shift_id);
        let shift_id = |day: &str, overnight: bool| -> Result<ShiftId> {
            let date = day.parse::<jiff::civil::Date>()?;
            shifts
                .iter()
                .find(|shift| {
                    shift.reporting_date == date
                        && (shift.interval.ends_at.local.as_datetime().date() > date) == overnight
                })
                .map(|shift| shift.id)
                .context("missing native boundary shift")
        };
        let start = shift_id("2027-02-08", false)?;
        let end = shift_id("2027-02-15", false)?;
        let preceding_call = shift_id("2027-02-07", true)?;
        let closing_call = shift_id("2027-02-14", true)?;
        let mut selected = BTreeSet::from([unavailable]);
        assert_eq!(
            callout_change_lower_bound(&selected, &shifts, unavailable)?,
            4
        );
        selected.extend([(person, preceding_call), (person, end), (other, start)]);
        assert_eq!(
            callout_change_lower_bound(&selected, &shifts, unavailable)?,
            4
        );
        selected.insert((person, start));
        assert_eq!(
            callout_change_lower_bound(&selected, &shifts, unavailable)?,
            2
        );
        selected.insert((person, closing_call));
        assert_eq!(
            callout_change_lower_bound(&selected, &shifts, unavailable)?,
            2
        );
        selected.remove(&(person, start));
        assert_eq!(
            callout_change_lower_bound(&selected, &shifts, unavailable)?,
            2
        );
        let unknown: ShiftId = "0195a5e4-7c00-7000-8000-000000000099".parse()?;
        selected.insert((other, unknown));
        assert!(callout_change_lower_bound(&selected, &shifts, unavailable).is_err());
        assert!(callout_change_lower_bound(&BTreeSet::new(), &shifts, unavailable).is_err());
        assert!(
            callout_change_lower_bound(&BTreeSet::from([(person, start)]), &shifts, unavailable,)
                .is_err()
        );
        assert!(
            callout_change_lower_bound(&BTreeSet::from([(person, end)]), &shifts, (person, end),)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn minimum_pair_cost_requires_a_bound_or_actual_optimality() {
        for (cost, bound, status, expected) in [
            (4, 4, SolveStatus::Feasible, true),
            (6, 4, SolveStatus::Feasible, false),
            (2, 4, SolveStatus::Optimal, false),
            (6, 4, SolveStatus::Optimal, true),
            (2, 2, SolveStatus::Feasible, true),
            (4, 2, SolveStatus::Feasible, false),
            (4, 2, SolveStatus::Optimal, true),
        ] {
            assert_eq!(minimum_change_proved(cost, bound, status), expected);
        }
    }
}
