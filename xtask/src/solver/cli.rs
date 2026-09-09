use super::{
    Context, CopyState, MAX_SIDECAR_FILE_BYTES, MAX_SIDECAR_TOTAL_BYTES, MAX_SIDECAR_TREE_ENTRIES,
    PackageStageGuard, Path, Result, SOLVER_MANIFEST_DIGEST_ENV, SidecarTarget,
    copy_artifact_to_layout, copy_regular_file, create_private_directory_chain, ensure,
    ensure_repository_root, fs, host_solver_target, is_link_like, publish_directory,
    read_manifest_target, sha256_file, sidecar_target, validate_executable_architecture,
    validate_staged_layout,
};
use std::io::Write;
use std::process::Command;

const IMAGE_NAME: &str = "cli-package";
const MANIFEST_PATH_ENV: &str = "EUTHETO_ORTOOLS_MANIFEST_PATH";

pub(crate) fn build_cli(repository: &Path, supplied: Option<(&Path, &str)>) -> Result<()> {
    ensure_repository_root(repository)?;
    let (host_target, _) = host_solver_target()?;
    let parent = repository.join("target");
    let mut stage = PackageStageGuard::acquire_package(repository, &parent, IMAGE_NAME)?;
    stage.prepare()?;
    #[cfg(windows)]
    let (artifact, mut native_lease) = match supplied {
        Some((root, _)) => (root.to_path_buf(), None),
        None => {
            let (artifact, lease) = super::build_native_artifact(repository)?;
            (artifact, Some(lease))
        }
    };
    #[cfg(not(windows))]
    let artifact = match supplied {
        Some((root, _)) => root.to_path_buf(),
        None => super::build_pinned_nix_artifact(repository)?,
    };
    let manifest_sha256 =
        crate::solver_manifest::validate(crate::solver_manifest::ValidateOptions {
            source_contract: &repository.join("workers/ortools/source-contract.json"),
            protocol_schema: &repository.join("protocol/solver-worker.proto"),
            protocol_policy: &repository.join("protocol/version.json"),
            artifact_root: &artifact,
        })?;
    if let Some((_, expected_sha256)) = supplied {
        ensure!(
            manifest_sha256 == expected_sha256,
            "supplied CLI worker manifest differs from the trusted artifact digest"
        );
    }
    let artifact_target = read_manifest_target(&artifact)?;
    ensure!(
        artifact_target == host_target,
        "CLI worker artifact does not match the host target"
    );
    let target = cli_target(&artifact_target)?;
    let staging = stage.staging();
    let copied_payload =
        copy_artifact_to_layout(&artifact, &staging, "", "solver/ortools", target)?;
    let resources = staging.join("solver/ortools");
    let worker = staging.join(target.bundled_worker);
    ensure!(
        validate_staged_layout(repository, &resources, &worker, target)? == manifest_sha256,
        "staged CLI solver manifest differs from the approved artifact"
    );

    // Isolate the executable from ordinary source and desktop Cargo builds. The package lease
    // covers this build directory as well as staging until the completed image is published.
    let build_root = parent.join("cli-build");
    create_private_directory_chain(repository, &build_root)?;
    let status = Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--release",
            "-p",
            "eutheto-cli",
            "--target",
            host_target,
        ])
        .arg("--target-dir")
        .arg(&build_root)
        .current_dir(repository)
        .env(MANIFEST_PATH_ENV, resources.join("solver-manifest.json"))
        .env(SOLVER_MANIFEST_DIGEST_ENV, &manifest_sha256)
        .status()
        .context("failed to start the manifest-bound CLI build")?;
    ensure!(
        status.success(),
        "manifest-bound CLI build failed with {status}"
    );
    let optimizer = if cfg!(windows) {
        "optimizer.exe"
    } else {
        "optimizer"
    };
    let executable = build_root.join(host_target).join("release").join(optimizer);
    copy_cli_executable(&executable, &staging, optimizer, &copied_payload)?;
    validate_executable_architecture(&staging.join(optimizer), host_target)?;
    ensure!(
        validate_staged_layout(repository, &resources, &worker, target)? == manifest_sha256,
        "CLI solver resources changed during the build"
    );
    #[cfg(windows)]
    if let Some(lease) = &mut native_lease {
        lease.commit()?;
    }
    let current = parent.join(IMAGE_NAME);
    publish_directory(&staging, &current, &stage.previous())?;
    stage.commit();
    println!("built manifest-bound CLI image: {}", current.display());
    Ok(())
}

fn cli_target(triple: &str) -> Result<SidecarTarget> {
    let target = sidecar_target(triple)?;
    Ok(SidecarTarget {
        artifact_worker: target.artifact_worker,
        bundled_worker: if triple == "x86_64-pc-windows-msvc" {
            "ortools-worker.exe"
        } else {
            "ortools-worker"
        },
    })
}

fn copy_cli_executable(source: &Path, staging: &Path, name: &str, state: &CopyState) -> Result<()> {
    let metadata = fs::symlink_metadata(source).context("built CLI executable is missing")?;
    ensure!(
        metadata.is_file() && !is_link_like(&metadata) && metadata.len() <= MAX_SIDECAR_FILE_BYTES,
        "built CLI executable must be a bounded direct regular file"
    );
    ensure!(
        state.entries + 3 <= MAX_SIDECAR_TREE_ENTRIES
            && state.bytes + metadata.len() <= MAX_SIDECAR_TOTAL_BYTES,
        "completed CLI image exceeds the package entry or byte limit"
    );
    let digest = sha256_file(source)?;
    let destination = staging.join(name);
    copy_regular_file(source, staging, &destination, &metadata, true)?;
    ensure!(
        sha256_file(&destination)? == digest,
        "CLI executable changed while staging"
    );
    Ok(())
}

// Reuse the repository's reviewed Workforce data, without registering any debug-only pack.
#[path = "../../../domains/workforce/core/tests/support/mod.rs"]
mod workforce_fixture;

pub(crate) fn smoke_cli(repository: &Path, image: &Path) -> Result<()> {
    let image = super::canonical_packaged_resource_root(image)?;
    let target = repository.join("target");
    create_private_directory_chain(repository, &target)?;
    // Stored-library custody requires a private home/repository ancestor, not global /tmp.
    let scratch = tempfile::Builder::new()
        .prefix("eutheto-cli-smoke-")
        .tempdir_in(&target)
        .context("failed to create private CLI smoke directory")?;
    let root = scratch.path();
    let relocated = root.join("relocated");
    copy_smoke_image(&image, &relocated)?;
    let executable = relocated.join(optimizer_name());
    let commands = root.join("commands.json");
    fs::write(&commands, serde_json::to_vec(&smoke_fixture_commands()?)?)?;

    let scenario = smoke_file_mode(&executable, &commands, root)?;
    smoke_stored_mode(&executable, &commands, root)?;
    smoke_tampered_images(&image, &scenario, root)?;

    println!(
        "packaged CLI smoke passed: relocated file/stored solve, fresh verification, explanation, JSON/CSV export, worker/manifest tamper rejection"
    );
    Ok(())
}

fn smoke_file_mode(executable: &Path, commands: &Path, root: &Path) -> Result<std::path::PathBuf> {
    let blocked = root.join("not-a-directory");
    fs::write(&blocked, b"file-mode must not replace this file")?;
    let unusable_data = blocked.join("library");
    let scenario = root.join("scenario.json");
    let empty = root.join("empty.json");
    let created = run_cli(
        executable,
        &unusable_data,
        &[
            "projects",
            "create",
            "--pack",
            "official.workforce",
            "--title",
            "CLI worker smoke",
            "--time-zone",
            "America/New_York",
            "--horizon-start",
            "2026-11-01T04:00:00Z",
            "--horizon-end",
            "2026-11-02T05:00:00Z",
            "--output",
            path_text(&empty)?,
        ],
        0,
    )?;
    let initial = created["result"]["revision"]
        .as_u64()
        .context("file create omitted revision")?;
    let applied = run_cli(
        executable,
        &unusable_data,
        &[
            "scenario",
            "batch",
            path_text(&empty)?,
            "--commands",
            path_text(commands)?,
            "--expected-revision",
            &initial.to_string(),
            "--output",
            path_text(&scenario)?,
        ],
        0,
    )?;
    ensure!(
        applied["result"]["newRevision"] == initial + 1,
        "file batch did not advance one revision"
    );
    let file_id = created["result"]["scenarioId"]
        .as_str()
        .context("file create omitted identity")?;
    smoke_solve_flow(
        executable,
        &unusable_data,
        path_text(&scenario)?,
        file_id,
        initial + 1,
        root,
        "file",
    )?;
    ensure!(
        fs::read(&blocked)? == b"file-mode must not replace this file" && !unusable_data.exists(),
        "file-mode CLI touched its unusable library location"
    );
    Ok(scenario)
}

fn smoke_stored_mode(executable: &Path, commands: &Path, root: &Path) -> Result<()> {
    // Fixed entity identities belong to exactly one project in this fresh library.
    let library = root.join("library");
    let created = run_cli(
        executable,
        &library,
        &[
            "projects",
            "create",
            "--pack",
            "official.workforce",
            "--title",
            "Stored CLI worker smoke",
            "--time-zone",
            "America/New_York",
            "--horizon-start",
            "2026-11-01T04:00:00Z",
            "--horizon-end",
            "2026-11-02T05:00:00Z",
        ],
        0,
    )?;
    let stored_id = created["result"]["scenarioId"]
        .as_str()
        .context("stored create omitted identity")?;
    let initial = created["result"]["revision"]
        .as_u64()
        .context("stored create omitted revision")?;
    let applied = run_cli(
        executable,
        &library,
        &[
            "scenario",
            "batch",
            stored_id,
            "--commands",
            path_text(commands)?,
            "--expected-revision",
            &initial.to_string(),
        ],
        0,
    )?;
    ensure!(
        applied["result"]["newRevision"] == initial + 1,
        "stored batch did not advance one revision"
    );
    smoke_solve_flow(
        executable,
        &library,
        stored_id,
        stored_id,
        initial + 1,
        root,
        "stored",
    )?;
    Ok(())
}

fn smoke_tampered_images(image: &Path, scenario: &Path, root: &Path) -> Result<()> {
    let unusable_data = root.join("not-a-directory/library");
    for (label, relative) in [
        (
            "worker-tamper",
            if cfg!(windows) {
                "ortools-worker.exe"
            } else {
                "ortools-worker"
            },
        ),
        ("manifest-tamper", "solver/ortools/solver-manifest.json"),
    ] {
        let tampered = root.join(label);
        copy_smoke_image(image, &tampered)?;
        super::OpenOptions::new()
            .append(true)
            .open(tampered.join(relative))?
            .write_all(b" ")?;
        let output = root.join(format!("{label}.json"));
        run_cli(
            &tampered.join(optimizer_name()),
            &unusable_data,
            &[
                "solve",
                path_text(scenario)?,
                "--backend",
                "solver.ortools-cp-sat",
                "--threads",
                "1",
                "--seed",
                "1",
                "--max-time",
                "10s",
                "--output",
                path_text(&output)?,
            ],
            6,
        )?;
        ensure!(
            !output.exists(),
            "tampered CLI image published an accepted result"
        );
    }
    Ok(())
}

fn optimizer_name() -> &'static str {
    if cfg!(windows) {
        "optimizer.exe"
    } else {
        "optimizer"
    }
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str().context("CLI smoke path is not UTF-8")
}

fn copy_smoke_image(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    super::materialize_resource_tree(source, destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["optimizer", "ortools-worker"] {
            fs::set_permissions(destination.join(name), fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

fn smoke_fixture_commands() -> Result<serde_json::Value> {
    use serde_json::json;
    use workforce_fixture::id;
    let fixture = workforce_fixture::fixture().map_err(|error| anyhow::anyhow!("{error}"))?;
    let value = serde_json::to_value(fixture)?;
    let entities = &value["domain"]["entities"];
    // One real two-hour shift and one qualified person. Omit the overlapping recurring
    // template and its lock; retain the fixture's workload/score policy and qualifications.
    let mut commands = Vec::new();
    for index in [11, 2, 3, 4, 5, 1, 8, 9] {
        let entity = entities
            .get(id(index))
            .context("owned Workforce fixture entity missing")?;
        commands.push(json!({"type":"applyDomainCommand","payload":{
            "commandType":"official.workforce.add_entity","payload":{"entity":entity}
        }}));
    }
    for (index, kind) in [(30, "eligibility"), (32, "coverage")] {
        commands.push(json!({"type":"applyDomainCommand","payload":{
            "commandType":"official.workforce.add_rule","payload":{"rule":{
                "id":id(index),"kind":kind,"active":true,"strength":"required","scope":{"people":{"kind":"all"}}
            }}
        }}));
    }
    Ok(serde_json::Value::Array(commands))
}

fn smoke_solve_flow(
    executable: &Path,
    data: &Path,
    scenario: &str,
    scenario_id: &str,
    revision: u64,
    root: &Path,
    label: &str,
) -> Result<()> {
    let validated = run_cli(executable, data, &["scenario", "validate", scenario], 0)?;
    ensure!(
        validated["result"]["revision"] == revision,
        "validation used a different revision"
    );
    let output = root.join(format!("{label}-result.json"));
    let solved = run_cli(
        executable,
        data,
        &[
            "solve",
            scenario,
            "--backend",
            "solver.ortools-cp-sat",
            "--mode",
            "balanced",
            "--threads",
            "1",
            "--seed",
            "1",
            "--max-time",
            "10s",
            "--output",
            path_text(&output)?,
        ],
        0,
    )?;
    let result = &solved["result"];
    ensure!(
        result["scenarioId"] == scenario_id
            && result["scenarioRevision"] == revision
            && result["independentlyVerified"] == true
            && result["backendId"] == "solver.ortools-cp-sat",
        "CLI solve did not accept the requested scenario/revision through OR-Tools"
    );
    let result_id = result["resultId"]
        .as_str()
        .context("accepted solve omitted result identity")?;
    let portable = read_smoke_json(&output)?;
    ensure!(
        portable["scenarioId"] == scenario_id
            && portable["scenarioRevision"] == revision
            && portable["resultId"] == result_id
            && portable["runInput"]["runId"] == result["runId"]
            && portable["acceptedResult"]["verification"]["accepted"] == true,
        "published accepted result differs from the completed run"
    );
    let assignment = format!(
        "official.workforce.assignment.{}.{}",
        workforce_fixture::id(1),
        workforce_fixture::id(8)
    );
    let assignments = portable["acceptedResult"]["solution"]["assignments"]
        .as_array()
        .context("accepted result omitted assignments")?;
    ensure!(
        assignments.len() == 1
            && assignments[0]["id"] == assignment
            && assignments[0]["value"] == serde_json::json!({"type":"boolean","value":true}),
        "CLI worker did not produce the fixture's exact required assignment"
    );
    let solution = if label == "stored" {
        result_id
    } else {
        path_text(&output)?
    };
    smoke_verify_result(
        executable,
        data,
        scenario,
        solution,
        result,
        label == "stored",
    )?;
    smoke_explain_assignment(executable, data, scenario, solution, &assignment)?;
    smoke_export_result(executable, data, scenario, solution, &portable, root, label)?;
    Ok(())
}

fn smoke_verify_result(
    executable: &Path,
    data: &Path,
    scenario: &str,
    solution: &str,
    result: &serde_json::Value,
    stored: bool,
) -> Result<()> {
    let revision = result["scenarioRevision"]
        .as_u64()
        .context("accepted solve omitted revision")?;
    let result_id = result["resultId"]
        .as_str()
        .context("accepted solve omitted result identity")?;
    let verified = run_cli(
        executable,
        data,
        &["solutions", "verify", scenario, solution],
        0,
    )?;
    ensure!(
        verified["result"]["scenarioRevision"] == revision
            && verified["result"]["verification"]["accepted"] == true,
        "fresh verification did not accept the solved revision"
    );
    if stored {
        ensure!(
            result["currentRevision"] == revision
                && result["stale"] == false
                && verified["result"]["result"]["solutionId"] == result_id,
            "stored solve or verification lost result identity/freshness"
        );
        let listed = run_cli(executable, data, &["solutions", "list", scenario], 0)?;
        let solutions = listed["result"]["solutions"]
            .as_array()
            .context("stored list omitted solutions")?;
        ensure!(
            solutions.len() == 1
                && solutions[0]["result"]["solutionId"] == result_id
                && solutions[0]["scenarioRevision"] == revision
                && solutions[0]["stale"] == false,
            "accepted stored run was not retained at the solved revision"
        );
    } else {
        ensure!(
            verified["result"]["resultId"] == result_id
                && verified["result"]["historicalRunMetadata"] == "sourceProvidedUnverified",
            "file verification endorsed historical metadata or lost result identity"
        );
    }
    Ok(())
}

fn smoke_explain_assignment(
    executable: &Path,
    data: &Path,
    scenario: &str,
    solution: &str,
    assignment: &str,
) -> Result<()> {
    let explained = run_cli(
        executable,
        data,
        &[
            "solutions",
            "explain",
            scenario,
            solution,
            "--assignment-id",
            assignment,
        ],
        0,
    )?;
    let messages = explained["result"]["explanation"]["rendered"]["messages"]
        .as_array()
        .context("assignment explanation omitted rendered evidence")?;
    ensure!(
        messages.iter().any(|message| {
            message["assignments"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == assignment))
        }),
        "explanation does not identify the accepted assignment"
    );
    Ok(())
}

fn smoke_export_result(
    executable: &Path,
    data: &Path,
    scenario: &str,
    solution: &str,
    portable: &serde_json::Value,
    root: &Path,
    label: &str,
) -> Result<()> {
    let result_id = portable["resultId"]
        .as_str()
        .context("accepted export source omitted result identity")?;
    let revision = portable["scenarioRevision"]
        .as_u64()
        .context("accepted export source omitted revision")?;
    let assignment = portable["acceptedResult"]["solution"]["assignments"][0]["id"]
        .as_str()
        .context("accepted export source omitted assignment identity")?;
    for format in ["json", "csv"] {
        let export = root.join(format!("{label}-export.{format}"));
        let exported = run_cli(
            executable,
            data,
            &[
                "solutions",
                "export",
                scenario,
                solution,
                "--format",
                format,
                "--output",
                path_text(&export)?,
            ],
            0,
        )?;
        ensure!(
            exported["result"]["resultId"] == result_id
                && exported["result"]["scenarioRevision"] == revision,
            "export lost accepted result/revision binding"
        );
        if format == "json" {
            ensure!(
                read_smoke_json(&export)? == *portable,
                "JSON export changed the accepted artifact"
            );
        } else {
            let expected = format!(
                "eutheto/assignments,1\nassignment_id,person_id,shift_id\n{assignment},{},{}\n",
                workforce_fixture::id(1),
                workforce_fixture::id(8),
            );
            ensure!(
                super::read_bounded_file(&export, 16 * 1024 * 1024, "assignment CSV export")?
                    == expected.as_bytes(),
                "CSV export did not preserve the exact selected schedule and v1 framing"
            );
        }
    }
    Ok(())
}

fn read_smoke_json(path: &Path) -> Result<serde_json::Value> {
    let bytes = super::read_bounded_file(path, 16 * 1024 * 1024, "CLI smoke JSON")?;
    serde_json::from_slice(&bytes).context("CLI smoke artifact is not valid JSON")
}

fn read_cli_output(stream: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    stream.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn run_cli(
    executable: &Path,
    data: &Path,
    args: &[&str],
    expected_exit: i32,
) -> Result<serde_json::Value> {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let manifest = executable
        .parent()
        .context("CLI executable has no parent")?
        .join("solver/ortools/solver-manifest.json");
    // Even the tampered manifest's matching digest must not override embedded build authority.
    let mut child = Command::new(executable)
        .args(["--format", "json", "--offline", "--data-dir"])
        .arg(data)
        .args(args)
        .env(MANIFEST_PATH_ENV, &manifest)
        .env(SOLVER_MANIFEST_DIGEST_ENV, sha256_file(&manifest)?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start relocated optimizer")?;
    let stdout = child.stdout.take().context("CLI stdout was not piped")?;
    let stderr = child.stderr.take().context("CLI stderr was not piped")?;
    let stdout = thread::spawn(move || read_cli_output(stdout));
    let stderr = thread::spawn(move || read_cli_output(stderr));
    let deadline = Instant::now() + Duration::from_mins(1);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("relocated CLI command exceeded its smoke deadline");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout
        .join()
        .map_err(|_| anyhow::anyhow!("CLI stdout reader panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_| anyhow::anyhow!("CLI stderr reader panicked"))??;
    ensure!(
        stdout.len() <= 1024 * 1024 && stderr.len() <= 1024 * 1024,
        "CLI smoke output exceeds its bound"
    );
    ensure!(
        status.code() == Some(expected_exit),
        "CLI smoke command {args:?} returned {status}, expected {expected_exit}; stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    let bytes = if expected_exit == 0 {
        ensure!(
            stderr.is_empty(),
            "successful CLI command emitted unexpected diagnostics"
        );
        &stdout
    } else {
        ensure!(
            stdout.is_empty(),
            "rejected CLI command emitted accepted output"
        );
        &stderr
    };
    let envelope: serde_json::Value =
        serde_json::from_slice(bytes).context("CLI did not emit one JSON envelope")?;
    ensure!(
        envelope["apiVersion"] == "eutheto/cli-result/v1" && envelope["ok"] == (expected_exit == 0),
        "CLI result envelope disagrees with its process disposition"
    );
    Ok(envelope)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{IMAGE_NAME, PackageStageGuard, cli_target, copy_artifact_to_layout, fs};

    #[test]
    fn cli_layout_keeps_native_names_and_nested_resources() {
        let root = tempfile::TempDir::new().unwrap();
        let artifact = root.path().join("artifact");
        fs::create_dir_all(artifact.join("bin")).unwrap();
        fs::create_dir_all(artifact.join("lib")).unwrap();
        fs::write(artifact.join("bin/ortools-worker.exe"), b"worker").unwrap();
        fs::write(artifact.join("lib/runtime.dll"), b"runtime").unwrap();
        fs::write(artifact.join("solver-manifest.json"), b"manifest").unwrap();
        let staging = root.path().join("image");
        copy_artifact_to_layout(
            &artifact,
            &staging,
            "",
            "solver/ortools",
            cli_target("x86_64-pc-windows-msvc").unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::read(staging.join("ortools-worker.exe")).unwrap(),
            b"worker"
        );
        assert_eq!(
            fs::read(staging.join("solver/ortools/lib/runtime.dll")).unwrap(),
            b"runtime"
        );
        assert_eq!(
            fs::read(staging.join("solver/ortools/solver-manifest.json")).unwrap(),
            b"manifest"
        );
        assert!(
            !staging
                .join("solver/ortools/bin/ortools-worker.exe")
                .exists()
        );
    }

    #[test]
    fn failed_cli_stage_preserves_complete_image_and_excludes_other_builders() {
        let repository = tempfile::TempDir::new().unwrap();
        let parent = repository.path().join("target");
        let stage =
            PackageStageGuard::acquire_package(repository.path(), &parent, IMAGE_NAME).unwrap();
        stage.prepare().unwrap();
        fs::create_dir(parent.join(IMAGE_NAME)).unwrap();
        fs::write(parent.join(IMAGE_NAME).join("optimizer"), b"complete").unwrap();
        fs::create_dir(stage.staging()).unwrap();
        fs::write(stage.staging().join("optimizer"), b"partial").unwrap();
        assert!(
            PackageStageGuard::acquire_package(repository.path(), &parent, IMAGE_NAME).is_err()
        );
        drop(stage);
        assert_eq!(
            fs::read(parent.join(IMAGE_NAME).join("optimizer")).unwrap(),
            b"complete"
        );
        assert!(!parent.join("cli-package.staging").exists());
    }
}
