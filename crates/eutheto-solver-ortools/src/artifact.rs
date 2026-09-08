use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

use eutheto_protocol::checked_in_policy;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncReadExt;

use crate::{
    ExecutableIdentityError, ORTOOLS_ADAPTER_VERSION, ORTOOLS_VERSION, VerifiedExecutable,
    VerifiedRuntimeFile, add_no_follow_flags, is_link_like, verify_path_and_digest,
};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_RUNTIME_FILES: usize = 128;
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const WORKER_IDENTITY: &str = "eutheto-ortools-worker";
const WORKER_BACKEND_ID: &str = "ortools-cp-sat";
const WORKER_VERSION: &str = "0.1.0";
const CAPABILITIES: [&str; 7] = [
    "cp-sat",
    "deterministic-time",
    "intermediate-solutions",
    "objective-bounds",
    "progress",
    "solution-projection",
    "solution-stats",
];

/// A complete manifest-authenticated worker runtime closure.
#[derive(Clone)]
pub struct VerifiedWorkerArtifact {
    executable: VerifiedExecutable,
}

impl VerifiedWorkerArtifact {
    /// Verifies the fixed installed manifest and every executable runtime payload under one
    /// artifact root. The expected manifest digest must come from trusted application build
    /// metadata.
    ///
    /// # Errors
    ///
    /// Returns a safe error when the root, manifest, target, worker identity, capability inventory,
    /// relative paths, or any payload digest differs from the approved contract.
    pub async fn verify(
        artifact_root: impl Into<PathBuf>,
        expected_manifest_sha256: [u8; 32],
    ) -> Result<Self, BundledWorkerArtifactError> {
        let artifact_root = artifact_root.into();
        let executable_path = artifact_root.join(expected_executable_relative_path());
        Self::verify_paths(
            artifact_root,
            executable_path,
            expected_manifest_sha256,
            true,
        )
        .await
    }

    /// Verifies a Tauri-packaged worker whose executable was installed beside the application
    /// binary while its manifest and runtime closure remain under one resource root.
    ///
    /// The executable path must be an absolute path with the fixed packaged worker filename. The
    /// expected manifest digest must come from trusted application build metadata.
    ///
    /// # Errors
    ///
    /// Returns a safe error under the same conditions as [`Self::verify`], or when the packaged
    /// executable does not use the fixed filename.
    pub async fn verify_packaged(
        artifact_root: impl Into<PathBuf>,
        executable_path: impl Into<PathBuf>,
        expected_manifest_sha256: [u8; 32],
    ) -> Result<Self, BundledWorkerArtifactError> {
        let executable_path = executable_path.into();
        let expected_relative = expected_executable_relative_path();
        if !executable_path.is_absolute()
            || executable_path.file_name() != expected_relative.file_name()
        {
            return Err(BundledWorkerArtifactError::UnexpectedExecutablePath);
        }
        Self::verify_paths(
            artifact_root.into(),
            executable_path,
            expected_manifest_sha256,
            false,
        )
        .await
    }

    async fn verify_paths(
        artifact_root: PathBuf,
        executable_path: PathBuf,
        expected_manifest_sha256: [u8; 32],
        executable_inside_root: bool,
    ) -> Result<Self, BundledWorkerArtifactError> {
        verify_artifact_root(&artifact_root).await?;
        let manifest_path = artifact_root.join("solver-manifest.json");
        verify_direct_regular_file(&manifest_path).await?;
        let manifest_bytes = read_bounded_manifest(&manifest_path).await?;
        let actual_manifest_sha256: [u8; 32] = Sha256::digest(&manifest_bytes).into();
        if actual_manifest_sha256 != expected_manifest_sha256 {
            return Err(BundledWorkerArtifactError::ManifestDigestMismatch);
        }
        let manifest: InstalledManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| BundledWorkerArtifactError::InvalidManifest)?;
        validate_manifest_identity(&manifest)?;

        let executable_relative = validated_relative_path(&manifest.worker.executable.path)?;
        if executable_relative != expected_executable_relative_path() {
            return Err(BundledWorkerArtifactError::UnexpectedExecutablePath);
        }
        let executable_sha256 = decode_sha256(&manifest.worker.executable.sha256)?;
        if executable_inside_root {
            verify_no_symlink_components(&artifact_root, &executable_relative).await?;
        }
        let executable_len = verify_path_and_digest(&executable_path, &executable_sha256).await?;

        if manifest.runtime_libraries.len() > MAX_RUNTIME_FILES {
            return Err(BundledWorkerArtifactError::RuntimeFileLimit);
        }
        let mut seen = BTreeSet::from([executable_relative.clone()]);
        let mut aggregate_len = executable_len;
        let mut runtime_files = Vec::with_capacity(manifest.runtime_libraries.len());
        for file in &manifest.runtime_libraries {
            let relative = validated_relative_path(&file.path)?;
            if !seen.insert(relative.clone()) {
                return Err(BundledWorkerArtifactError::DuplicateRuntimePath);
            }
            let sha256 = decode_sha256(&file.sha256)?;
            let path = artifact_root.join(&relative);
            verify_no_symlink_components(&artifact_root, &relative).await?;
            let len = verify_path_and_digest(&path, &sha256).await?;
            aggregate_len = aggregate_len
                .checked_add(len)
                .ok_or(BundledWorkerArtifactError::ArtifactTooLarge)?;
            if aggregate_len > MAX_ARTIFACT_BYTES {
                return Err(BundledWorkerArtifactError::ArtifactTooLarge);
            }
            runtime_files.push(VerifiedRuntimeFile {
                path,
                sha256,
                len,
                staged_relative_path: relative,
            });
        }

        Ok(Self {
            executable: VerifiedExecutable {
                path: executable_path,
                executable_sha256,
                executable_len,
                manifest_sha256: expected_manifest_sha256,
                staged_relative_path: executable_relative,
                runtime_files,
            },
        })
    }

    pub(crate) fn executable(&self) -> VerifiedExecutable {
        self.executable.clone()
    }
}

impl fmt::Debug for VerifiedWorkerArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedWorkerArtifact { identity: [redacted] }")
    }
}

async fn verify_artifact_root(root: &Path) -> Result<(), BundledWorkerArtifactError> {
    if !root.is_absolute() {
        return Err(BundledWorkerArtifactError::RootNotAbsolute);
    }
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .map_err(|error| BundledWorkerArtifactError::Io(error.kind()))?;
    if is_link_like(&metadata) || !metadata.is_dir() {
        return Err(BundledWorkerArtifactError::InvalidRoot);
    }
    Ok(())
}

async fn verify_direct_regular_file(path: &Path) -> Result<(), BundledWorkerArtifactError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| BundledWorkerArtifactError::Io(error.kind()))?;
    if is_link_like(&metadata) || !metadata.is_file() {
        return Err(BundledWorkerArtifactError::InvalidManifestFile);
    }
    Ok(())
}

async fn read_bounded_manifest(path: &Path) -> Result<Vec<u8>, BundledWorkerArtifactError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    add_no_follow_flags(&mut options);
    let file = options
        .open(path)
        .await
        .map_err(|error| BundledWorkerArtifactError::Io(error.kind()))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| BundledWorkerArtifactError::Io(error.kind()))?;
    if is_link_like(&metadata) || !metadata.is_file() {
        return Err(BundledWorkerArtifactError::InvalidManifestFile);
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(BundledWorkerArtifactError::ManifestTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| BundledWorkerArtifactError::Io(error.kind()))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(BundledWorkerArtifactError::ManifestTooLarge);
    }
    Ok(bytes)
}

fn validate_manifest_identity(
    manifest: &InstalledManifest,
) -> Result<(), BundledWorkerArtifactError> {
    let policy = checked_in_policy().map_err(|_| BundledWorkerArtifactError::ProtocolMismatch)?;
    if manifest.manifest.schema_version != 1 || manifest.manifest.generation_contract_version != 1 {
        return Err(BundledWorkerArtifactError::ManifestVersionMismatch);
    }
    if manifest.backend_source.kind != "ortools"
        || manifest.backend_source.version != ORTOOLS_VERSION
        || manifest.worker.identity != WORKER_IDENTITY
        || manifest.worker.version != WORKER_VERSION
        || manifest.worker.backend_id != WORKER_BACKEND_ID
        || manifest.worker.adapter_version != ORTOOLS_ADAPTER_VERSION
        || manifest.worker.distribution != "bundled-worker"
        || manifest.worker.stability != "beta"
    {
        return Err(BundledWorkerArtifactError::WorkerIdentityMismatch);
    }
    if current_target_triple() == "unsupported"
        || current_architecture() == "unsupported"
        || manifest.build.target_triple != current_target_triple()
        || manifest.build.architecture != current_architecture()
    {
        return Err(BundledWorkerArtifactError::TargetMismatch);
    }
    if manifest.protocol.major != policy.protocol_major()
        || manifest.protocol.minor != policy.protocol_minor()
        || manifest.protocol.wire_version != 1
    {
        return Err(BundledWorkerArtifactError::ProtocolMismatch);
    }
    if manifest.capabilities.as_slice() != CAPABILITIES {
        return Err(BundledWorkerArtifactError::CapabilityMismatch);
    }
    Ok(())
}

fn validated_relative_path(value: &str) -> Result<PathBuf, BundledWorkerArtifactError> {
    if value.is_empty() || value.len() > 2048 || value.contains('\\') || value.contains(':') {
        return Err(BundledWorkerArtifactError::InvalidRelativePath);
    }
    let path = PathBuf::from(value);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(BundledWorkerArtifactError::InvalidRelativePath);
    }
    Ok(path)
}

async fn verify_no_symlink_components(
    root: &Path,
    relative: &Path,
) -> Result<(), BundledWorkerArtifactError> {
    let mut current = root.to_owned();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(BundledWorkerArtifactError::InvalidRelativePath);
        };
        current.push(component);
        let metadata = tokio::fs::symlink_metadata(&current)
            .await
            .map_err(|error| BundledWorkerArtifactError::Io(error.kind()))?;
        if is_link_like(&metadata) {
            return Err(BundledWorkerArtifactError::SymlinkComponent);
        }
    }
    Ok(())
}

fn decode_sha256(value: &str) -> Result<[u8; 32], BundledWorkerArtifactError> {
    if value.len() != 64 {
        return Err(BundledWorkerArtifactError::InvalidDigest);
    }
    let mut digest = [0_u8; 32];
    for (index, output) in digest.iter_mut().enumerate() {
        let offset = index * 2;
        *output = u8::from_str_radix(&value[offset..offset + 2], 16)
            .map_err(|_| BundledWorkerArtifactError::InvalidDigest)?;
    }
    Ok(digest)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
const fn current_target_triple() -> &'static str {
    "x86_64-unknown-linux-gnu"
}
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const fn current_target_triple() -> &'static str {
    "x86_64-pc-windows-msvc"
}
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const fn current_target_triple() -> &'static str {
    "x86_64-apple-darwin"
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const fn current_target_triple() -> &'static str {
    "aarch64-apple-darwin"
}
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"),
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
const fn current_target_triple() -> &'static str {
    "unsupported"
}

#[cfg(target_arch = "x86_64")]
const fn current_architecture() -> &'static str {
    "x86_64"
}
#[cfg(target_arch = "aarch64")]
const fn current_architecture() -> &'static str {
    "aarch64"
}
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const fn current_architecture() -> &'static str {
    "unsupported"
}

#[cfg(windows)]
fn expected_executable_relative_path() -> PathBuf {
    PathBuf::from("bin/ortools-worker.exe")
}
#[cfg(not(windows))]
fn expected_executable_relative_path() -> PathBuf {
    PathBuf::from("bin/ortools-worker")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledManifest {
    #[serde(rename = "approval")]
    _approval: serde_json::Value,
    backend_source: BackendSource,
    build: BuildIdentity,
    capabilities: Vec<String>,
    #[serde(rename = "licenses")]
    _licenses: serde_json::Value,
    manifest: ManifestIdentity,
    #[serde(rename = "protobuf")]
    _protobuf: serde_json::Value,
    protocol: ProtocolIdentity,
    runtime_libraries: Vec<FileDigest>,
    #[serde(rename = "sbom")]
    _sbom: serde_json::Value,
    worker: WorkerManifest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendSource {
    kind: String,
    #[serde(rename = "sha256")]
    _sha256: String,
    #[serde(rename = "source_url")]
    _source_url: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildIdentity {
    architecture: String,
    #[serde(rename = "cmake")]
    _cmake: serde_json::Value,
    #[serde(rename = "compiler")]
    _compiler: serde_json::Value,
    #[serde(rename = "linkage")]
    _linkage: String,
    target_triple: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestIdentity {
    generation_contract_version: u32,
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolIdentity {
    major: u32,
    minor: u32,
    #[serde(rename = "schema_sha256")]
    _schema_sha256: String,
    wire_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDigest {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerManifest {
    adapter_version: String,
    backend_id: String,
    distribution: String,
    executable: FileDigest,
    identity: String,
    stability: String,
    version: String,
}

/// Safe bundled-worker artifact verification failures. Paths and digests are never displayed.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum BundledWorkerArtifactError {
    #[error("worker artifact root is not absolute")]
    RootNotAbsolute,
    #[error("worker artifact root is not a direct directory")]
    InvalidRoot,
    #[error("worker manifest is not a direct regular file")]
    InvalidManifestFile,
    #[error("worker manifest exceeds the bounded input limit")]
    ManifestTooLarge,
    #[error("worker artifact I/O failed: {0:?}")]
    Io(io::ErrorKind),
    #[error("worker manifest digest does not match trusted application metadata")]
    ManifestDigestMismatch,
    #[error("worker manifest is not the strict installed-manifest shape")]
    InvalidManifest,
    #[error("worker manifest version does not match the runtime contract")]
    ManifestVersionMismatch,
    #[error("worker identity or version does not match the runtime contract")]
    WorkerIdentityMismatch,
    #[error("worker target does not match this application binary")]
    TargetMismatch,
    #[error("worker protocol does not match this application binary")]
    ProtocolMismatch,
    #[error("worker capability inventory does not match the runtime contract")]
    CapabilityMismatch,
    #[error("worker executable path does not match the installed layout")]
    UnexpectedExecutablePath,
    #[error("worker artifact contains an invalid relative path")]
    InvalidRelativePath,
    #[error("worker artifact contains a symbolic-link path component")]
    SymlinkComponent,
    #[error("worker artifact contains an invalid SHA-256 digest")]
    InvalidDigest,
    #[error("worker artifact contains too many runtime files")]
    RuntimeFileLimit,
    #[error("worker artifact exceeds the aggregate staging byte limit")]
    ArtifactTooLarge,
    #[error("worker artifact contains a duplicate runtime path")]
    DuplicateRuntimePath,
    #[error(transparent)]
    Executable(#[from] ExecutableIdentityError),
}

#[cfg(all(test, unix))]
mod read_custody_tests {
    use super::{BundledWorkerArtifactError, read_bounded_manifest, verify_direct_regular_file};
    use std::os::unix::fs::{OpenOptionsExt, symlink};
    use std::time::Duration;

    #[test]
    fn checked_manifest_replaced_by_fifo_rejects_without_waiting_for_a_writer()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("manifest.json");
        std::fs::write(&path, b"{}")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(verify_direct_regular_file(&path))?;
        std::fs::remove_file(&path)?;
        let status = std::process::Command::new("mkfifo").arg(&path).status()?;
        assert!(status.success());
        // Exercise the actual read after the successful pathname precheck, not a second
        // metadata rejection that would conceal the check/open race.
        let result = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), read_bounded_manifest(&path)).await
        });
        if result.is_err() {
            // A regressed blocking open must not strand the test process during shutdown.
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path);
        }
        runtime.shutdown_timeout(Duration::from_secs(2));
        assert!(matches!(
            result,
            Ok(Err(BundledWorkerArtifactError::InvalidManifestFile))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn manifest_handle_reads_regular_bytes_and_rejects_symlink_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("manifest.json");
        let other = directory.path().join("other.json");
        std::fs::write(&path, b"{\"checked\":true}")?;
        assert_eq!(read_bounded_manifest(&path).await?, b"{\"checked\":true}");
        verify_direct_regular_file(&path).await?;
        std::fs::write(&other, b"{\"substituted\":true}")?;
        std::fs::remove_file(&path)?;
        symlink(&other, &path)?;
        assert!(matches!(
            read_bounded_manifest(&path).await,
            Err(BundledWorkerArtifactError::Io(_))
        ));
        Ok(())
    }
}
