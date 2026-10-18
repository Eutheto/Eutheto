//! Build-time custody for the fixed, single-process PDF renderer.

use crate::solver::{
    PackageStageGuard, copy_regular_file, create_private_directory_chain, host_solver_target,
    open_regular_file_no_follow, publish_directory, sha256_file, validate_executable_architecture,
};
use anyhow::{Context, Result, ensure};
use eutheto_types::RecipientPdfRendererV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const MANIFEST_PATH_ENV: &str = "EUTHETO_PDF_MANIFEST_PATH";
pub(crate) const MANIFEST_SHA_ENV: &str = "EUTHETO_PDF_MANIFEST_SHA256";
const MANIFEST: &str = "eutheto-pdf-worker.json";
const FONT_NAMES: [&str; 5] = [
    "LibertinusSerif-Regular.otf",
    "LibertinusSerif-Bold.otf",
    "LibertinusSerif-Italic.otf",
    "LibertinusSerif-BoldItalic.otf",
    "DejaVuSansMono.ttf",
];
const NOTICES: [&str; 6] = [
    "UPSTREAM-NOTICE",
    "UPSTREAM-LICENSE",
    "COMPACT-ICC-LICENSE",
    "ICU4X-LICENSE",
    "W3C-DOCUMENT-LICENSE-2015",
    "ASSET-PROVENANCE-NOTICE",
];
// Helper, manifest, renderer/pdf directories, and each retained notice.
pub(crate) const CLI_PAYLOAD_ENTRIES: usize = NOTICES.len() + 4;
const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    worker_sha256: String,
    renderer: RecipientPdfRendererV1,
}

pub(crate) struct Artifact {
    pub(crate) root: PathBuf,
    target: &'static str,
    // Keep the publication locked until its consumer has copied the exact bytes.
    _lease: PackageStageGuard,
}

pub(crate) fn worker_name(target: &str) -> &'static str {
    if target == "x86_64-pc-windows-msvc" {
        "eutheto-pdf-worker.exe"
    } else {
        "eutheto-pdf-worker"
    }
}

fn read_asset(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = open_regular_file_no_follow(path)?;
    let size = file.metadata()?.len();
    ensure!(
        size > 0 && size <= limit,
        "PDF asset exceeds its byte limit"
    );
    let mut bytes = Vec::new();
    file.take(size + 1).read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? == size,
        "PDF asset changed while reading"
    );
    Ok(bytes)
}

fn renderer_identity(authority: &Path) -> Result<RecipientPdfRendererV1> {
    let worker = authority.join("crates/eutheto-pdf-worker");
    let template = read_asset(&worker.join("src/report.typ"), MAX_ASSET_BYTES)?;
    let mut fonts = Sha256::new();
    for name in FONT_NAMES {
        let bytes = read_asset(&worker.join("fonts").join(name), MAX_ASSET_BYTES)?;
        fonts.update(u64::try_from(bytes.len())?.to_be_bytes());
        fonts.update(&bytes);
    }
    Ok(RecipientPdfRendererV1 {
        version: 1,
        typst_version: "0.15.1".into(),
        template_sha256: format!("{:x}", Sha256::digest(template)),
        fonts_sha256: format!("{:x}", fonts.finalize()),
    })
}

/// Run after stripping, RPATH changes, and platform signing; consumers must copy unchanged.
pub(crate) fn finalize(
    authority: &Path,
    executable: &Path,
    manifest: &Path,
    target: &str,
) -> Result<String> {
    validate_executable_architecture(executable, target)?;
    let value = Manifest {
        schema_version: 1,
        worker_sha256: sha256_file(executable)?,
        renderer: renderer_identity(authority)?,
    };
    let bytes = serde_json::to_vec(&value)?;
    ensure!(bytes.len() <= 4096, "PDF manifest exceeds its byte limit");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(manifest)
        .with_context(|| format!("failed to create PDF manifest {}", manifest.display()))?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn build(repository: &Path) -> Result<Artifact> {
    let (target, _) = host_solver_target()?;
    let parent = repository.join("target");
    let lease = PackageStageGuard::acquire_package(repository, &parent, "pdf-worker")?;
    lease.prepare()?;
    let build_root = parent.join("pdf-build");
    create_private_directory_chain(repository, &build_root)?;
    let status = Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--release",
            "-p",
            "eutheto-pdf-worker",
            "--target",
            target,
        ])
        .arg("--target-dir")
        .arg(&build_root)
        .current_dir(repository)
        .status()
        .context("failed to start the PDF worker build")?;
    ensure!(status.success(), "PDF worker build failed with {status}");
    let staging = lease.staging();
    create_private_directory_chain(repository, &staging.join("bin"))?;
    let name = worker_name(target);
    copy_checked(
        &build_root.join(target).join("release").join(name),
        &staging,
        &staging.join("bin").join(name),
        true,
    )?;
    for notice in NOTICES {
        copy_checked(
            &repository
                .join("crates/eutheto-pdf-worker/fonts")
                .join(notice),
            &staging,
            &staging.join(notice),
            false,
        )?;
    }
    finalize(
        repository,
        &staging.join("bin").join(name),
        &staging.join(MANIFEST),
        target,
    )?;
    let root = parent.join("pdf-worker");
    publish_directory(&staging, &root, &lease.previous())?;
    println!("built PDF worker artifact: {}", root.display());
    Ok(Artifact {
        root,
        target,
        _lease: lease,
    })
}

fn copy_checked(source: &Path, root: &Path, destination: &Path, executable: bool) -> Result<u64> {
    let metadata = fs::symlink_metadata(source)?;
    ensure!(
        metadata.is_file()
            && metadata.len() > 0
            && metadata.len()
                <= if executable {
                    MAX_EXECUTABLE_BYTES
                } else {
                    MAX_ASSET_BYTES
                },
        "PDF package input must be a bounded regular file"
    );
    let digest = sha256_file(source)?;
    copy_regular_file(source, root, destination, &metadata, executable)?;
    ensure!(
        sha256_file(destination)? == digest,
        "PDF package input changed while copying"
    );
    Ok(metadata.len())
}

/// Copy the helper and provenance documents into an already privately staged package.
pub(crate) fn copy_into(
    repository: &Path,
    artifact: &Artifact,
    staging: &Path,
    executable: &Path,
    resources: &Path,
    manifest: &Path,
) -> Result<(String, u64)> {
    let expected = read_asset(&artifact.root.join(MANIFEST), 4096)?;
    let value: Manifest = serde_json::from_slice(&expected)?;
    ensure!(
        value.schema_version == 1 && value.renderer == renderer_identity(repository)?,
        "PDF artifact renderer does not match the reviewed sources"
    );
    ensure!(
        serde_json::to_vec(&value)? == expected,
        "PDF artifact manifest is not canonical"
    );
    create_private_directory_chain(
        repository,
        executable.parent().context("missing executable parent")?,
    )?;
    create_private_directory_chain(repository, resources)?;
    let mut bytes = copy_checked(
        &artifact.root.join("bin").join(worker_name(artifact.target)),
        staging,
        executable,
        true,
    )?;
    validate_executable_architecture(executable, artifact.target)?;
    ensure!(
        sha256_file(executable)? == value.worker_sha256,
        "PDF worker differs from its manifest"
    );
    bytes += copy_checked(&artifact.root.join(MANIFEST), staging, manifest, false)?;
    ensure!(
        read_asset(manifest, 4096)? == expected,
        "PDF manifest changed while copying"
    );
    for notice in NOTICES {
        let source = artifact.root.join(notice);
        ensure!(
            read_asset(&source, MAX_ASSET_BYTES)?
                == read_asset(
                    &repository
                        .join("crates/eutheto-pdf-worker/fonts")
                        .join(notice),
                    MAX_ASSET_BYTES
                )?,
            "PDF font notice differs from its reviewed source"
        );
        bytes += copy_checked(&source, staging, &resources.join(notice), false)?;
    }
    Ok((format!("{:x}", Sha256::digest(expected)), bytes))
}

pub(crate) fn stage_desktop(repository: &Path) -> Result<(PathBuf, String, PackageStageGuard)> {
    let artifact = build(repository)?;
    let parent = repository.join("apps/desktop/src-tauri");
    let lease = PackageStageGuard::acquire_package(repository, &parent, "pdf-sidecar")?;
    lease.prepare()?;
    let staging = lease.staging();
    let suffix = if artifact.target.ends_with("windows-msvc") {
        ".exe"
    } else {
        ""
    };
    let executable = staging
        .join("bin")
        .join(format!("eutheto-pdf-worker-{}{suffix}", artifact.target));
    let resources = staging.join("resources");
    let (digest, _) = copy_into(
        repository,
        &artifact,
        &staging,
        &executable,
        &resources,
        &resources.join(MANIFEST),
    )?;
    let current = parent.join("pdf-sidecar");
    publish_directory(&staging, &current, &lease.previous())?;
    Ok((current.join("resources").join(MANIFEST), digest, lease))
}

pub(crate) fn stage_cli(repository: &Path, staging: &Path) -> Result<(PathBuf, String, u64)> {
    let artifact = build(repository)?;
    let manifest = staging.join(MANIFEST);
    let (digest, bytes) = copy_into(
        repository,
        &artifact,
        staging,
        &staging.join(worker_name(artifact.target)),
        &staging.join("renderer/pdf"),
        &manifest,
    )?;
    Ok((manifest, digest, bytes))
}

pub(crate) fn verify_staged(executable: &Path, manifest: &Path, expected: &str) -> Result<()> {
    let bytes = read_asset(manifest, 4096)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "staged PDF manifest changed during the application build"
    );
    let value: Manifest = serde_json::from_slice(&bytes)?;
    ensure!(
        sha256_file(executable)? == value.worker_sha256,
        "staged PDF executable changed during the application build"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn verify_macos_bundle(repository: &Path, expected: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct TargetDirectory {
        target_directory: PathBuf,
    }
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(repository)
        .output()?;
    ensure!(
        output.status.success(),
        "failed to resolve the desktop bundle target directory"
    );
    let target: TargetDirectory = serde_json::from_slice(&output.stdout)?;
    let directory = target.target_directory.join("release/bundle/macos");
    let mut apps = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            ensure!(
                entry.file_type()?.is_dir(),
                "desktop app must be a direct directory"
            );
            apps.push(entry.path());
        }
    }
    ensure!(
        apps.len() == 1,
        "expected exactly one final macOS application bundle"
    );
    verify_staged(
        &apps[0].join("Contents/MacOS/eutheto-pdf-worker"),
        &apps[0]
            .join("Contents/Resources/renderer/pdf")
            .join(MANIFEST),
        expected,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<tempfile::TempDir> {
        let root = tempfile::tempdir()?;
        let worker = root.path().join("crates/eutheto-pdf-worker");
        fs::create_dir_all(worker.join("src"))?;
        fs::create_dir_all(worker.join("fonts"))?;
        fs::write(worker.join("src/report.typ"), b"fixed template")?;
        for name in FONT_NAMES.into_iter().chain(NOTICES) {
            fs::write(worker.join("fonts").join(name), name)?;
        }
        Ok(root)
    }

    #[test]
    fn finalization_binds_final_executable_template_and_ordered_fonts() -> Result<()> {
        let root = fixture()?;
        let exe = root.path().join("worker");
        let mut header = [0u8; 64];
        header[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        header[18] = 0x3e;
        fs::write(&exe, header)?;
        let path = root.path().join(MANIFEST);
        let digest = finalize(root.path(), &exe, &path, "x86_64-unknown-linux-gnu")?;
        assert_eq!(sha256_file(&path)?, digest);
        let value: Manifest = serde_json::from_slice(&fs::read(&path)?)?;
        assert_eq!(value.worker_sha256, sha256_file(&exe)?);
        let first = value.renderer;
        fs::write(
            root.path()
                .join("crates/eutheto-pdf-worker/fonts/LibertinusSerif-Regular.otf"),
            b"changed",
        )?;
        assert_ne!(
            first.fonts_sha256,
            renderer_identity(root.path())?.fonts_sha256
        );
        assert_eq!(
            first.template_sha256,
            renderer_identity(root.path())?.template_sha256
        );
        fs::write(
            root.path().join("crates/eutheto-pdf-worker/src/report.typ"),
            b"changed",
        )?;
        assert_ne!(
            first.template_sha256,
            renderer_identity(root.path())?.template_sha256
        );
        assert!(finalize(root.path(), &exe, &path, "x86_64-unknown-linux-gnu").is_err());
        assert!(
            finalize(
                root.path(),
                &exe,
                &root.path().join("wrong.json"),
                "aarch64-apple-darwin"
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn staging_rejects_changed_worker_and_preserves_existing_publication() -> Result<()> {
        let root = fixture()?;
        let parent = root.path().join("target");
        let lease = PackageStageGuard::acquire_package(root.path(), &parent, "pdf-worker")?;
        let artifact_root = parent.join("pdf-worker");
        fs::create_dir_all(artifact_root.join("bin"))?;
        let executable = artifact_root.join("bin/eutheto-pdf-worker");
        let mut header = [0u8; 64];
        header[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        header[18] = 0x3e;
        fs::write(&executable, header)?;
        finalize(
            root.path(),
            &executable,
            &artifact_root.join(MANIFEST),
            "x86_64-unknown-linux-gnu",
        )?;
        for notice in NOTICES {
            fs::write(artifact_root.join(notice), notice)?;
        }
        let artifact = Artifact {
            root: artifact_root,
            target: "x86_64-unknown-linux-gnu",
            _lease: lease,
        };
        let stage = PackageStageGuard::acquire_package(root.path(), &parent, "image")?;
        stage.prepare()?;
        let current = parent.join("image");
        fs::create_dir(&current)?;
        fs::write(current.join("retained"), b"old image")?;
        let staging = stage.staging();
        copy_into(
            root.path(),
            &artifact,
            &staging,
            &staging.join("worker"),
            &staging.join("resources"),
            &staging.join(MANIFEST),
        )?;
        assert_eq!(
            fs::read(staging.join("resources/UPSTREAM-LICENSE"))?,
            b"UPSTREAM-LICENSE"
        );
        let manifest = staging.join(MANIFEST);
        let digest = sha256_file(&manifest)?;
        verify_staged(&staging.join("worker"), &manifest, &digest)?;
        OpenOptions::new()
            .append(true)
            .open(staging.join("worker"))?
            .write_all(b"tamper")?;
        assert!(verify_staged(&staging.join("worker"), &manifest, &digest).is_err());
        assert!(PackageStageGuard::acquire_package(root.path(), &parent, "image").is_err());
        drop(stage);
        assert!(!staging.exists());
        assert_eq!(fs::read(current.join("retained"))?, b"old image");
        let stage = PackageStageGuard::acquire_package(root.path(), &parent, "image")?;
        stage.prepare()?;
        OpenOptions::new()
            .append(true)
            .open(executable)?
            .write_all(b"tamper")?;
        assert!(
            copy_into(
                root.path(),
                &artifact,
                &staging,
                &staging.join("worker"),
                &staging.join("resources"),
                &staging.join(MANIFEST)
            )
            .is_err()
        );
        drop(stage);
        assert_eq!(fs::read(current.join("retained"))?, b"old image");
        Ok(())
    }
}
