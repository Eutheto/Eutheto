use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const MAX_INPUT_BYTES: usize = 65_536;
const MAX_SBOM_BYTES: usize = 1_048_576;
const MAX_DEPTH: usize = 8;
const MAX_OBJECT_MEMBERS: usize = 128;
const MAX_ARRAY_ENTRIES: usize = 256;
const MAX_STRING_BYTES: usize = 2_048;
const MAX_CAPABILITIES: usize = 64;
const MAX_CMAKE_ENTRIES: usize = 128;
const MAX_RUNTIME_LIBRARIES: usize = 128;
const MAX_TREE_ENTRIES: usize = 256;
const SCHEMA_VERSION: u32 = 1;
const GENERATION_CONTRACT_VERSION: u32 = 1;
const BACKEND_ID: &str = "ortools-cp-sat";
const BACKEND_KIND: &str = "ortools";
const ADAPTER_VERSION: &str = "0.1.0";
const DISTRIBUTION: &str = "bundled-worker";
const STABILITY: &str = "beta";
const LINKAGE: &str = "static-ortools";
const CP_MODEL_PROTO_SHA256: &str =
    "c967180600fab5db4fc8b7477fef56c3c6d3c0714b1f0355697f96d147f77d96";
const SAT_PARAMETERS_PROTO_SHA256: &str =
    "9a5e08486a63414191870bd9953f9e561af221e165654913a33662bf1b674308";
const CAPABILITIES: [&str; 8] = [
    "cp-sat",
    "deterministic-time",
    "intermediate-solutions",
    "objective-bounds",
    "progress",
    "solution-projection",
    "solution-stats",
    "sufficient-assumptions",
];
const PATH_TOKENS: [&str; 7] = [
    "@artifact-root@",
    "@build-root@",
    "@loader_path",
    "@rpath",
    "@source-root@",
    "@target-root@",
    "@toolchain-root@",
];
const ORTOOLS_CMAKE_KEYS: [&str; 13] = [
    "CMAKE_C_COMPILER",
    "CMAKE_CXX_COMPILER",
    "CMAKE_CXX_FLAGS",
    "CMAKE_GENERATOR",
    "CMAKE_INSTALL_PREFIX",
    "CMAKE_POLICY_DEFAULT_CMP0077",
    "FETCHCONTENT_FULLY_DISCONNECTED",
    "FETCHCONTENT_SOURCE_DIR_ABSL",
    "FETCHCONTENT_SOURCE_DIR_BZIP2",
    "FETCHCONTENT_SOURCE_DIR_EIGEN3",
    "FETCHCONTENT_SOURCE_DIR_PROTOBUF",
    "FETCHCONTENT_SOURCE_DIR_RE2",
    "FETCHCONTENT_SOURCE_DIR_ZLIB",
];
const WORKER_CMAKE_KEYS: [&str; 14] = [
    "CMAKE_CXX_COMPILER",
    "CMAKE_FIND_USE_CMAKE_ENVIRONMENT_PATH",
    "CMAKE_FIND_USE_PACKAGE_REGISTRY",
    "CMAKE_FIND_USE_PACKAGE_ROOT_PATH",
    "CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY",
    "CMAKE_GENERATOR",
    "CMAKE_INSTALL_PREFIX",
    "CMAKE_PREFIX_PATH",
    "EUTHETO_ORTOOLS_BUILD_CANDIDATE_BENCHMARKS",
    "EUTHETO_ORTOOLS_BUILD_TESTS",
    "EUTHETO_ORTOOLS_DEVELOPMENT_BUILD",
    "EUTHETO_ORTOOLS_PHASE3_CONTRACT",
    "Protobuf_DIR",
    "ortools_DIR",
];
const DARWIN_CMAKE_KEYS: [&str; 3] = [
    "CMAKE_INSTALL_NAME_DIR",
    "CMAKE_INSTALL_RPATH",
    "CMAKE_MACOSX_RPATH",
];
const WINDOWS_CMAKE_KEYS: [&str; 1] = ["CMAKE_MSVC_RUNTIME_LIBRARY"];
pub(crate) const NOTICE_PATH: &str = "licenses/NOTICE.txt";
pub(crate) const SBOM_PATH: &str = "sbom/solver.spdx.json";
pub(crate) const LICENSE_PROFILE: [(&str, &str); 8] = [
    ("licenses/abseil-Apache-2.0.txt", "Apache-2.0"),
    ("licenses/bzip2-bzip2-1.0.6.txt", "bzip2-1.0.6"),
    ("licenses/eutheto-Apache-2.0.txt", "Apache-2.0"),
    ("licenses/or-tools-Apache-2.0.txt", "Apache-2.0"),
    ("licenses/protobuf-BSD-3-Clause.txt", "BSD-3-Clause"),
    ("licenses/re2-BSD-3-Clause.txt", "BSD-3-Clause"),
    ("licenses/utf8-range-MIT.txt", "MIT"),
    ("licenses/zlib-Zlib.txt", "Zlib"),
];
const ABSEIL_VERSION: &str = "20250814.1";
const ABSEIL_URL: &str = "https://github.com/abseil/abseil-cpp/archive/refs/tags/20250814.1.tar.gz";
const ABSEIL_SHA256: &str = "1692f77d1739bacf3f94337188b78583cf09bab7e420d2dc6c5605a4f86785a1";
const BZIP2_VERSION: &str = "66c46b8c9436613fd81bc5d03f63a61933a4dcc3";
const BZIP2_URL: &str = "https://gitlab.com/bzip2/bzip2/-/archive/66c46b8c9436613fd81bc5d03f63a61933a4dcc3/bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz";
const BZIP2_SHA256: &str = "3a4cff5f9d197e9e6c6138660afa6b1f9370df0bed135bd949243f6dfc83b3e1";
const RE2_VERSION: &str = "2025-08-12";
const RE2_URL: &str = "https://github.com/google/re2/archive/refs/tags/2025-08-12.tar.gz";
const RE2_SHA256: &str = "2f3bec634c3e51ea1faf0d441e0a8718b73ef758d7020175ed7e352df3f6ae12";
const ZLIB_VERSION: &str = "1.3.1";
const ZLIB_URL: &str = "https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz";
const ZLIB_SHA256: &str = "17e88863f3600672ab49182f217281b6fc4d3c762bde361935e436a95214d05c";
const ORTOOLS_VERSION: &str = "9.15.6755";
const ORTOOLS_URL: &str = "https://github.com/google/or-tools/archive/refs/tags/v9.15.tar.gz";
const ORTOOLS_SHA256: &str = "6395a00a97ff30af878ee8d7fd5ad0ab1c7844f7219182c6d71acbee1b5f3026";
const PROTOBUF_SOURCE_VERSION: &str = "33.1";
const PROTOBUF_RUNTIME_VERSION: &str = "33.1.0";
const PROTOBUF_URL: &str =
    "https://github.com/protocolbuffers/protobuf/releases/download/v33.1/protobuf-33.1.tar.gz";
const PROTOBUF_SHA256: &str = "fda132cb0c86400381c0af1fe98bd0f775cb566cb247cdcc105e344e00acc30e";

#[derive(Debug, Clone, Copy)]
pub(crate) struct AssembleOptions<'a> {
    pub source_contract: &'a Path,
    pub protocol_schema: &'a Path,
    pub protocol_policy: &'a Path,
    pub build_evidence: &'a Path,
    pub payload_evidence: &'a Path,
    pub artifact_root: &'a Path,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ValidateOptions<'a> {
    pub source_contract: &'a Path,
    pub protocol_schema: &'a Path,
    pub protocol_policy: &'a Path,
    pub artifact_root: &'a Path,
}

pub(crate) fn validate_authority_inputs(
    source_contract: &Path,
    protocol_schema: &Path,
    protocol_policy: &Path,
) -> Result<()> {
    let source_bytes = read_bounded(source_contract, "source contract")?;
    require_canonical_json(&source_bytes, "source contract")?;
    let source: SourceContract = parse_bounded_json(&source_bytes, "source contract")?;
    validate_source_contract(&source)?;
    let protocol_schema_bytes = read_bounded(protocol_schema, "protocol schema")?;
    ensure!(
        sha256_bytes(&protocol_schema_bytes) == source.protocol.schema_sha256,
        "protocol schema SHA-256 does not match the approved source contract"
    );
    let policy_bytes = read_bounded(protocol_policy, "protocol policy")?;
    require_canonical_json(&policy_bytes, "protocol policy")?;
    let policy: ProtocolPolicy = parse_bounded_json(&policy_bytes, "protocol policy")?;
    validate_protocol_policy(&policy, source.protocol.wire_version)
}

pub(crate) fn assemble(options: AssembleOptions<'_>) -> Result<String> {
    let artifact_root = canonical_artifact_root(options.artifact_root)?;
    let source_bytes = read_bounded(options.source_contract, "source contract")?;
    require_canonical_json(&source_bytes, "source contract")?;
    let source: SourceContract = parse_bounded_json(&source_bytes, "source contract")?;
    validate_source_contract(&source)?;

    let protocol_schema_bytes = read_bounded(options.protocol_schema, "protocol schema")?;
    ensure!(
        sha256_bytes(&protocol_schema_bytes) == source.protocol.schema_sha256,
        "protocol schema SHA-256 does not match the approved source contract"
    );

    let policy_bytes = read_bounded(options.protocol_policy, "protocol policy")?;
    require_canonical_json(&policy_bytes, "protocol policy")?;
    let policy: ProtocolPolicy = parse_bounded_json(&policy_bytes, "protocol policy")?;
    validate_protocol_policy(&policy, source.protocol.wire_version)?;

    let build_bytes = read_bounded(options.build_evidence, "build evidence")?;
    let build: BuildEvidence = parse_bounded_json(&build_bytes, "build evidence")?;
    validate_build_evidence(&build, &source)?;

    let payload_bytes = read_bounded(options.payload_evidence, "payload evidence")?;
    let payload: PayloadEvidence = parse_bounded_json(&payload_bytes, "payload evidence")?;
    validate_payload_evidence(&payload)?;

    let manifest = derive_manifest(
        &source,
        sha256_bytes(&source_bytes),
        &policy,
        &build,
        &payload,
        &artifact_root,
    )?;
    validate_manifest_semantics(&manifest)?;
    validate_pre_manifest_artifacts(&manifest, &artifact_root)?;
    let bytes = canonical_bytes(&manifest)?;
    ensure!(
        bytes.len() <= MAX_INPUT_BYTES,
        "assembled manifest exceeds {MAX_INPUT_BYTES} bytes"
    );

    let output = artifact_root.join("solver-manifest.json");
    ensure!(
        !output.exists(),
        "refusing to replace existing manifest {}",
        output.display()
    );
    let mut temporary = NamedTempFile::new_in(&artifact_root).with_context(|| {
        format!(
            "failed to create temporary manifest in {}",
            artifact_root.display()
        )
    })?;
    std::io::Write::write_all(&mut temporary, &bytes)
        .context("failed to write temporary solver manifest")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = temporary
            .as_file()
            .metadata()
            .context("failed to inspect temporary solver manifest")?
            .permissions();
        permissions.set_mode(0o644);
        temporary
            .as_file()
            .set_permissions(permissions)
            .context("failed to set solver manifest permissions")?;
    }
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync temporary solver manifest")?;
    temporary.persist_noclobber(&output).map_err(|error| {
        anyhow::anyhow!(
            "failed to atomically install solver manifest {}: {}",
            output.display(),
            error.error
        )
    })?;

    let digest = sha256_bytes(&bytes);
    println!("{digest}");
    Ok(digest)
}

pub(crate) fn validate(options: ValidateOptions<'_>) -> Result<String> {
    let artifact_root = canonical_artifact_root(options.artifact_root)?;
    let source_bytes = read_bounded(options.source_contract, "source contract")?;
    require_canonical_json(&source_bytes, "source contract")?;
    let source: SourceContract = parse_bounded_json(&source_bytes, "source contract")?;
    validate_source_contract(&source)?;

    let protocol_schema_bytes = read_bounded(options.protocol_schema, "protocol schema")?;
    ensure!(
        sha256_bytes(&protocol_schema_bytes) == source.protocol.schema_sha256,
        "protocol schema SHA-256 does not match the approved source contract"
    );

    let policy_bytes = read_bounded(options.protocol_policy, "protocol policy")?;
    require_canonical_json(&policy_bytes, "protocol policy")?;
    let policy: ProtocolPolicy = parse_bounded_json(&policy_bytes, "protocol policy")?;
    validate_protocol_policy(&policy, source.protocol.wire_version)?;

    let manifest_path = artifact_root.join("solver-manifest.json");
    let manifest_metadata = fs::symlink_metadata(&manifest_path)
        .context("solver manifest is missing or inaccessible")?;
    ensure!(
        !manifest_metadata.file_type().is_symlink(),
        "solver manifest must not be a symlink"
    );
    let bytes = read_bounded(&manifest_path, "solver manifest")?;
    let manifest: SolverManifest = parse_bounded_json(&bytes, "solver manifest")?;
    validate_manifest_semantics(&manifest)?;
    validate_manifest_authority(&manifest, &source, &sha256_bytes(&source_bytes), &policy)?;
    let canonical = canonical_bytes(&manifest)?;
    ensure!(
        bytes == canonical,
        "solver manifest is not byte-for-byte canonical JSON"
    );
    validate_artifacts(&manifest, &artifact_root)?;
    let digest = sha256_bytes(&bytes);
    println!("{digest}");
    Ok(digest)
}

fn derive_manifest(
    source: &SourceContract,
    source_contract_sha256: String,
    policy: &ProtocolPolicy,
    build: &BuildEvidence,
    payload: &PayloadEvidence,
    artifact_root: &Path,
) -> Result<SolverManifest> {
    let mut seen = HashSet::new();
    let executable_sha256 = hash_artifact(&build.worker.executable_path, artifact_root, &mut seen)?;
    let mut runtime_libraries = Vec::with_capacity(build.runtime_library_paths.len());
    for path in &build.runtime_library_paths {
        runtime_libraries.push(FileDigest {
            path: path.clone(),
            sha256: hash_artifact(path, artifact_root, &mut seen)?,
        });
    }
    let mut licenses = Vec::with_capacity(payload.licenses.len());
    for license in &payload.licenses {
        licenses.push(LicenseDigest {
            path: license.path.clone(),
            sha256: hash_artifact(&license.path, artifact_root, &mut seen)?,
            spdx: license.spdx.clone(),
        });
    }
    let sbom_sha256 = hash_artifact(&payload.sbom_path, artifact_root, &mut seen)?;

    Ok(SolverManifest {
        approval: Approval {
            phase: source.approval.phase,
            record: source.approval.record.clone(),
            source_contract_sha256,
        },
        backend_source: BackendSource {
            kind: BACKEND_KIND.to_owned(),
            sha256: source.ortools.sha256.clone(),
            source_url: source.ortools.source_url.clone(),
            version: source.ortools.version.clone(),
        },
        build: Build {
            architecture: architecture_for_target(&build.target_triple)?.to_owned(),
            cmake: build.cmake.clone(),
            compiler: build.compiler.clone(),
            linkage: LINKAGE.to_owned(),
            target_triple: build.target_triple.clone(),
        },
        capabilities: CAPABILITIES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        licenses,
        manifest: ManifestVersion {
            generation_contract_version: GENERATION_CONTRACT_VERSION,
            schema_version: SCHEMA_VERSION,
        },
        protobuf: Protobuf {
            approved_proto_checksums: ApprovedProtoChecksums {
                cp_model_proto: CP_MODEL_PROTO_SHA256.to_owned(),
                sat_parameters_proto: SAT_PARAMETERS_PROTO_SHA256.to_owned(),
            },
            cpp_runtime_version: source.protobuf.cpp_runtime_version.clone(),
            protoc_version: source.protobuf.protoc_version.clone(),
            sha256: source.protobuf.sha256.clone(),
            source_url: source.protobuf.source_url.clone(),
            source_version: source.protobuf.source_version.clone(),
        },
        protocol: Protocol {
            major: policy.version.major,
            minor: policy.version.minor,
            schema_sha256: source.protocol.schema_sha256.clone(),
            wire_version: source.protocol.wire_version,
        },
        runtime_libraries,
        sbom: FileDigest {
            path: payload.sbom_path.clone(),
            sha256: sbom_sha256,
        },
        worker: Worker {
            adapter_version: ADAPTER_VERSION.to_owned(),
            backend_id: BACKEND_ID.to_owned(),
            distribution: DISTRIBUTION.to_owned(),
            executable: FileDigest {
                path: build.worker.executable_path.clone(),
                sha256: executable_sha256,
            },
            identity: source.worker.identity.clone(),
            stability: STABILITY.to_owned(),
            version: source.worker.version.clone(),
        },
    })
}

fn validate_source_contract(source: &SourceContract) -> Result<()> {
    ensure!(
        source.schema_version == 1,
        "unsupported source-contract schema_version {}",
        source.schema_version
    );
    ensure!(
        source.approval.status == "approved",
        "source contract is not approved"
    );
    ensure!(
        source.approval.phase == 3,
        "source contract approval phase must be 3"
    );
    validate_approval_record(&source.approval.record)?;
    ensure!(
        source.worker.identity == "eutheto-ortools-worker",
        "source contract has unexpected worker identity"
    );
    ensure!(
        source.ortools.version == ORTOOLS_VERSION
            && source.ortools.source_url == ORTOOLS_URL
            && source.ortools.sha256 == ORTOOLS_SHA256,
        "source contract selects an unsupported OR-Tools source"
    );
    ensure!(
        source.protobuf.source_version == PROTOBUF_SOURCE_VERSION
            && source.protobuf.protoc_version == PROTOBUF_SOURCE_VERSION
            && source.protobuf.cpp_runtime_version == PROTOBUF_RUNTIME_VERSION
            && source.protobuf.source_url == PROTOBUF_URL
            && source.protobuf.sha256 == PROTOBUF_SHA256,
        "source contract selects an unsupported protobuf source"
    );
    ensure!(
        source.worker.version == "0.1.0",
        "source contract selects an unsupported worker version"
    );
    validate_hash(&source.protocol.schema_sha256, "protocol schema")?;
    ensure!(
        source.cmake.cache_entries.len() == 26,
        "source contract must contain exactly 26 approved CMake entries"
    );
    ensure!(
        source.ortools.patch_path == "workers/ortools/patches/9.15-candidate-fixes.patch",
        "source contract has an unexpected OR-Tools patch path"
    );
    validate_hash(&source.ortools.patch_sha256, "source-contract patch")?;
    validate_cmake_map(&source.cmake.cache_entries, "source-contract CMake")?;
    Ok(())
}

fn validate_approval_record(record: &str) -> Result<()> {
    ensure!(
        record.len() <= 256,
        "source contract approval record is too long"
    );
    let revision = record
        .strip_prefix("docs/roadmap/assumptions.md@")
        .context("source contract approval record has an unexpected path")?;
    ensure!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "source contract approval record must end with a 40-character lowercase Git revision"
    );
    Ok(())
}

fn validate_protocol_policy(policy: &ProtocolPolicy, wire_version: u32) -> Result<()> {
    ensure!(
        policy.version.major == wire_version,
        "source-contract wire version does not match protocol-policy major"
    );
    ensure!(
        policy.version.major == 1,
        "unsupported protocol major {}",
        policy.version.major
    );
    ensure!(
        policy.package == "eutheto.worker.v1",
        "unexpected protocol package"
    );
    ensure!(
        policy.protocol == "eutheto.solver-worker",
        "unexpected protocol identity"
    );
    Ok(())
}

fn validate_build_evidence(build: &BuildEvidence, source: &SourceContract) -> Result<()> {
    ensure!(
        build.schema_version == 1,
        "unsupported build-evidence schema_version {}",
        build.schema_version
    );
    let architecture = architecture_for_target(&build.target_triple)?;
    ensure!(
        build.architecture == architecture,
        "architecture does not match target triple"
    );
    ensure!(
        build.linkage == LINKAGE,
        "build evidence linkage must be {LINKAGE}"
    );
    ensure!(
        build.worker.backend_id == BACKEND_ID,
        "build evidence backend_id mismatch"
    );
    ensure!(
        build.worker.adapter_version == ADAPTER_VERSION,
        "build evidence adapter_version mismatch"
    );
    validate_capabilities(&build.worker.capabilities)?;
    ensure!(
        build
            .worker
            .capabilities
            .iter()
            .map(String::as_str)
            .eq(CAPABILITIES),
        "build evidence capabilities do not match the reviewed capability set"
    );
    validate_exact_cmake_keys(
        &build.cmake.ortools,
        &source.cmake.cache_entries,
        &ORTOOLS_CMAKE_KEYS,
        &build.target_triple,
        "OR-Tools CMake",
    )?;
    validate_exact_cmake_keys(
        &build.cmake.worker,
        &source.cmake.cache_entries,
        &WORKER_CMAKE_KEYS,
        &build.target_triple,
        "worker CMake",
    )?;
    validate_compiler(&build.compiler)?;
    validate_relative_path(&build.worker.executable_path, "worker executable path")?;
    validate_sorted_unique_paths(
        &build.runtime_library_paths,
        "runtime library paths",
        MAX_RUNTIME_LIBRARIES,
    )?;
    ensure!(
        build.worker.executable_path == expected_worker_path(&build.target_triple)?,
        "worker executable path does not match the target contract"
    );
    for path in &build.runtime_library_paths {
        validate_runtime_library_path(path, &build.target_triple)?;
    }
    validate_msvc_runtime_closure(
        build.runtime_library_paths.iter().map(String::as_str),
        &build.target_triple,
    )?;
    validate_reviewed_cmake_values(
        &build.cmake.ortools,
        &build.target_triple,
        CmakeScope::Ortools,
        "OR-Tools CMake",
    )?;
    validate_reviewed_cmake_values(
        &build.cmake.worker,
        &build.target_triple,
        CmakeScope::Worker,
        "worker CMake",
    )?;
    for (key, expected) in &source.cmake.cache_entries {
        ensure!(
            build.cmake.ortools.get(key) == Some(expected),
            "OR-Tools CMake entry {key} differs from the approved source contract"
        );
        ensure!(
            build.cmake.worker.get(key) == Some(expected),
            "worker CMake entry {key} differs from the approved source contract"
        );
    }
    Ok(())
}

fn validate_payload_evidence(payload: &PayloadEvidence) -> Result<()> {
    ensure!(
        payload.schema_version == 1,
        "unsupported payload-evidence schema_version {}",
        payload.schema_version
    );
    ensure!(
        payload
            .licenses
            .iter()
            .map(|license| (license.path.as_str(), license.spdx.as_str()))
            .eq(LICENSE_PROFILE),
        "payload license inventory does not match the exact reviewed path/SPDX profile"
    );
    ensure!(
        payload.sbom_path == SBOM_PATH,
        "payload SBOM path must be {SBOM_PATH}"
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_manifest_semantics(manifest: &SolverManifest) -> Result<()> {
    ensure!(
        manifest.manifest.schema_version == SCHEMA_VERSION,
        "unsupported solver manifest schema_version {}",
        manifest.manifest.schema_version
    );
    ensure!(
        manifest.manifest.generation_contract_version == GENERATION_CONTRACT_VERSION,
        "unsupported generation_contract_version {}",
        manifest.manifest.generation_contract_version
    );
    ensure!(
        manifest.approval.phase == 3,
        "manifest approval phase must be 3"
    );
    validate_nonempty(&manifest.approval.record, "manifest approval record")?;
    validate_hash(&manifest.approval.source_contract_sha256, "source contract")?;
    ensure!(
        manifest.backend_source.kind == BACKEND_KIND,
        "manifest backend source kind mismatch"
    );
    validate_hash(&manifest.backend_source.sha256, "OR-Tools source")?;
    ensure!(
        manifest.build.architecture == architecture_for_target(&manifest.build.target_triple)?,
        "manifest architecture does not match target triple"
    );
    ensure!(
        manifest.build.linkage == LINKAGE,
        "manifest linkage must be {LINKAGE}"
    );
    validate_nonempty(&manifest.backend_source.source_url, "OR-Tools source URL")?;
    ensure!(
        manifest.backend_source.source_url.starts_with("https://"),
        "OR-Tools source URL must use HTTPS"
    );
    validate_nonempty(&manifest.backend_source.version, "OR-Tools version")?;
    validate_compiler(&manifest.build.compiler)?;
    validate_cmake_map(&manifest.build.cmake.ortools, "manifest OR-Tools CMake")?;
    validate_cmake_map(&manifest.build.cmake.worker, "manifest worker CMake")?;
    ensure!(
        manifest.build.cmake.ortools.len() <= MAX_CMAKE_ENTRIES,
        "manifest OR-Tools CMake map is too large"
    );
    ensure!(
        manifest.build.cmake.worker.len() <= MAX_CMAKE_ENTRIES,
        "manifest worker CMake map is too large"
    );
    validate_capabilities(&manifest.capabilities)?;
    ensure!(
        manifest
            .capabilities
            .iter()
            .map(String::as_str)
            .eq(CAPABILITIES),
        "manifest capabilities do not match reviewed constants"
    );
    ensure!(
        manifest.worker.backend_id == BACKEND_ID,
        "manifest backend_id mismatch"
    );
    ensure!(
        manifest.worker.adapter_version == ADAPTER_VERSION,
        "manifest adapter_version mismatch"
    );
    ensure!(
        manifest.worker.distribution == DISTRIBUTION,
        "manifest distribution mismatch"
    );
    ensure!(
        manifest.worker.stability == STABILITY,
        "manifest stability mismatch"
    );
    ensure!(
        manifest.worker.executable.path == expected_worker_path(&manifest.build.target_triple)?,
        "manifest worker executable path does not match the target contract"
    );
    validate_relative_path(&manifest.worker.executable.path, "worker executable path")?;
    validate_hash(&manifest.worker.executable.sha256, "worker executable")?;
    // Exact key and value validation is repeated against source authority below.
    validate_sorted_by_path(&manifest.runtime_libraries, "runtime library inventory")?;
    ensure!(
        manifest.runtime_libraries.len() <= MAX_RUNTIME_LIBRARIES,
        "runtime library inventory is too large"
    );
    ensure!(
        manifest
            .build
            .cmake
            .ortools
            .contains_key("CMAKE_INSTALL_PREFIX")
            && manifest
                .build
                .cmake
                .worker
                .contains_key("CMAKE_INSTALL_PREFIX"),
        "manifest CMake maps must both include CMAKE_INSTALL_PREFIX"
    );
    for item in &manifest.runtime_libraries {
        validate_relative_path(&item.path, "runtime library path")?;
        validate_runtime_library_path(&item.path, &manifest.build.target_triple)?;
        validate_hash(&item.sha256, "runtime library")?;
    }
    validate_msvc_runtime_closure(
        manifest
            .runtime_libraries
            .iter()
            .map(|item| item.path.as_str()),
        &manifest.build.target_triple,
    )?;
    ensure!(
        manifest
            .licenses
            .iter()
            .map(|license| (license.path.as_str(), license.spdx.as_str()))
            .eq(LICENSE_PROFILE),
        "manifest license inventory does not match the exact reviewed path/SPDX profile"
    );
    for item in &manifest.licenses {
        validate_hash(&item.sha256, "license")?;
    }
    ensure!(
        manifest.sbom.path == SBOM_PATH,
        "manifest SBOM path must be {SBOM_PATH}"
    );
    validate_hash(&manifest.sbom.sha256, "SBOM")?;
    validate_hash(&manifest.protobuf.sha256, "protobuf source")?;
    ensure!(
        manifest.protobuf.approved_proto_checksums.cp_model_proto == CP_MODEL_PROTO_SHA256,
        "cp_model.proto checksum mismatch"
    );
    ensure!(
        manifest
            .protobuf
            .approved_proto_checksums
            .sat_parameters_proto
            == SAT_PARAMETERS_PROTO_SHA256,
        "sat_parameters.proto checksum mismatch"
    );
    validate_nonempty(&manifest.protobuf.source_url, "protobuf source URL")?;
    ensure!(
        manifest.protobuf.source_url.starts_with("https://"),
        "protobuf source URL must use HTTPS"
    );
    validate_nonempty(&manifest.protobuf.source_version, "protobuf source version")?;
    validate_nonempty(&manifest.protobuf.protoc_version, "protoc version")?;
    validate_nonempty(
        &manifest.protobuf.cpp_runtime_version,
        "protobuf C++ runtime version",
    )?;
    validate_nonempty(&manifest.worker.identity, "worker identity")?;
    validate_nonempty(&manifest.worker.version, "worker version")?;
    ensure!(
        manifest.protocol.major == 1,
        "unsupported protocol major {}",
        manifest.protocol.major
    );
    ensure!(
        manifest.protocol.wire_version == manifest.protocol.major,
        "protocol wire_version and major differ"
    );
    validate_hash(&manifest.protocol.schema_sha256, "protocol schema")?;
    Ok(())
}

fn validate_manifest_authority(
    manifest: &SolverManifest,
    source: &SourceContract,
    source_contract_sha256: &str,
    policy: &ProtocolPolicy,
) -> Result<()> {
    ensure!(
        manifest.approval.phase == source.approval.phase
            && manifest.approval.record == source.approval.record
            && manifest.approval.source_contract_sha256 == source_contract_sha256,
        "manifest approval does not match the approved source contract"
    );
    ensure!(
        manifest.backend_source.sha256 == source.ortools.sha256
            && manifest.backend_source.source_url == source.ortools.source_url
            && manifest.backend_source.version == source.ortools.version,
        "manifest backend source does not match the approved source contract"
    );
    ensure!(
        manifest.protobuf.cpp_runtime_version == source.protobuf.cpp_runtime_version
            && manifest.protobuf.protoc_version == source.protobuf.protoc_version
            && manifest.protobuf.sha256 == source.protobuf.sha256
            && manifest.protobuf.source_url == source.protobuf.source_url
            && manifest.protobuf.source_version == source.protobuf.source_version,
        "manifest protobuf contract does not match the approved source contract"
    );
    ensure!(
        manifest.protocol.major == policy.version.major
            && manifest.protocol.minor == policy.version.minor
            && manifest.protocol.wire_version == source.protocol.wire_version
            && manifest.protocol.schema_sha256 == source.protocol.schema_sha256,
        "manifest protocol does not match the approved protocol contract"
    );
    ensure!(
        manifest.worker.identity == source.worker.identity
            && manifest.worker.version == source.worker.version,
        "manifest worker identity does not match the approved source contract"
    );
    validate_exact_cmake_keys(
        &manifest.build.cmake.ortools,
        &source.cmake.cache_entries,
        &ORTOOLS_CMAKE_KEYS,
        &manifest.build.target_triple,
        "manifest OR-Tools CMake",
    )?;
    validate_exact_cmake_keys(
        &manifest.build.cmake.worker,
        &source.cmake.cache_entries,
        &WORKER_CMAKE_KEYS,
        &manifest.build.target_triple,
        "manifest worker CMake",
    )?;
    validate_reviewed_cmake_values(
        &manifest.build.cmake.ortools,
        &manifest.build.target_triple,
        CmakeScope::Ortools,
        "manifest OR-Tools CMake",
    )?;
    validate_reviewed_cmake_values(
        &manifest.build.cmake.worker,
        &manifest.build.target_triple,
        CmakeScope::Worker,
        "manifest worker CMake",
    )?;
    Ok(())
}

fn validate_artifacts(manifest: &SolverManifest, artifact_root: &Path) -> Result<()> {
    validate_referenced_artifacts(manifest, artifact_root)?;
    validate_exact_artifact_tree(manifest, artifact_root, true)?;
    validate_spdx_document(manifest, artifact_root)
}

fn validate_pre_manifest_artifacts(manifest: &SolverManifest, artifact_root: &Path) -> Result<()> {
    validate_referenced_artifacts(manifest, artifact_root)?;
    validate_exact_artifact_tree(manifest, artifact_root, false)?;
    validate_spdx_document(manifest, artifact_root)
}

fn validate_referenced_artifacts(manifest: &SolverManifest, artifact_root: &Path) -> Result<()> {
    let mut seen = HashSet::new();
    verify_artifact(&manifest.worker.executable, artifact_root, &mut seen)?;
    for item in &manifest.runtime_libraries {
        verify_artifact(item, artifact_root, &mut seen)?;
    }
    for item in &manifest.licenses {
        verify_artifact(
            &FileDigest {
                path: item.path.clone(),
                sha256: item.sha256.clone(),
            },
            artifact_root,
            &mut seen,
        )?;
    }
    verify_artifact(&manifest.sbom, artifact_root, &mut seen)?;
    hash_artifact(NOTICE_PATH, artifact_root, &mut seen)?;
    Ok(())
}

fn verify_artifact(item: &FileDigest, root: &Path, seen: &mut HashSet<PathBuf>) -> Result<()> {
    let actual = hash_artifact(&item.path, root, seen)?;
    ensure!(
        actual == item.sha256,
        "artifact SHA-256 mismatch for {}",
        item.path
    );
    Ok(())
}

fn hash_artifact(relative: &str, root: &Path, seen: &mut HashSet<PathBuf>) -> Result<String> {
    validate_relative_path(relative, "artifact path")?;
    validate_local_symlink_components(relative, root)?;
    let path = root.join(relative);
    let canonical = fs::canonicalize(&path)
        .with_context(|| format!("artifact is missing or inaccessible: {relative}"))?;
    ensure!(
        canonical.starts_with(root),
        "artifact path escapes the canonical artifact root: {relative}"
    );
    let metadata = fs::metadata(&canonical)
        .with_context(|| format!("failed to inspect artifact: {relative}"))?;
    ensure!(
        metadata.is_file(),
        "artifact path is not a regular file: {relative}"
    );
    ensure!(
        seen.insert(canonical.clone()),
        "duplicate artifact alias: {relative}"
    );
    sha256_file(&canonical)
}

fn validate_local_symlink_components(relative: &str, root: &Path) -> Result<()> {
    let mut candidate = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            bail!("artifact path has an invalid component: {relative}");
        };
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate)
            .with_context(|| format!("artifact is missing or inaccessible: {relative}"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "artifact payload must not contain symlinks: {relative}"
        );
    }
    Ok(())
}

fn canonical_artifact_root(root: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(root).with_context(|| {
        format!(
            "artifact root is missing or inaccessible: {}",
            root.display()
        )
    })?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "artifact root must not be a symlink: {}",
        root.display()
    );
    let canonical = fs::canonicalize(root).with_context(|| {
        format!(
            "artifact root is missing or inaccessible: {}",
            root.display()
        )
    })?;
    ensure!(
        canonical.is_dir(),
        "artifact root is not a directory: {}",
        root.display()
    );
    Ok(canonical)
}

fn validate_exact_artifact_tree(
    manifest: &SolverManifest,
    root: &Path,
    include_manifest: bool,
) -> Result<()> {
    let (actual_files, actual_directories) = collect_artifact_tree(root)?;
    let mut expected_files = BTreeSet::new();
    expected_files.insert(manifest.worker.executable.path.clone());
    expected_files.extend(
        manifest
            .runtime_libraries
            .iter()
            .map(|item| item.path.clone()),
    );
    expected_files.extend(manifest.licenses.iter().map(|item| item.path.clone()));
    expected_files.insert(NOTICE_PATH.to_owned());
    expected_files.insert(SBOM_PATH.to_owned());
    if include_manifest {
        expected_files.insert("solver-manifest.json".to_owned());
    }
    ensure!(
        actual_files == expected_files,
        "artifact recursive file inventory differs from the exact manifest contract"
    );

    let mut expected_directories = BTreeSet::new();
    for path in &expected_files {
        let mut parent = Path::new(path).parent();
        while let Some(directory) = parent {
            if directory.as_os_str().is_empty() {
                break;
            }
            let value = directory
                .to_str()
                .context("artifact directory path is not UTF-8")?
                .replace('\\', "/");
            expected_directories.insert(value);
            parent = directory.parent();
        }
    }
    ensure!(
        actual_directories == expected_directories,
        "artifact recursive directory inventory differs from the exact manifest contract"
    );
    Ok(())
}

fn collect_artifact_tree(root: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0usize;
    while let Some(directory) = pending.pop() {
        let mut children = fs::read_dir(&directory)
            .with_context(|| format!("failed to read artifact directory {}", directory.display()))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| {
                format!(
                    "failed to enumerate artifact directory {}",
                    directory.display()
                )
            })?;
        children.sort_by_key(fs::DirEntry::file_name);
        for child in children {
            entries = entries
                .checked_add(1)
                .context("artifact entry count overflow")?;
            ensure!(
                entries <= MAX_TREE_ENTRIES,
                "artifact tree exceeds {MAX_TREE_ENTRIES} entries"
            );
            let path = child.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("failed to inspect artifact entry {}", path.display()))?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "artifact payload must not contain symlinks: {}",
                path.display()
            );
            let relative = path
                .strip_prefix(root)
                .context("artifact entry escaped root")?
                .to_str()
                .context("artifact entry path is not UTF-8")?
                .replace('\\', "/");
            validate_relative_path(&relative, "artifact inventory path")?;
            if metadata.is_dir() {
                ensure!(directories.insert(relative), "duplicate artifact directory");
                pending.push(path);
            } else {
                ensure!(
                    metadata.is_file(),
                    "artifact contains a special file: {}",
                    path.display()
                );
                ensure!(files.insert(relative), "duplicate artifact file");
            }
        }
    }
    Ok((files, directories))
}

fn validate_spdx_document(manifest: &SolverManifest, root: &Path) -> Result<()> {
    let bytes = read_bounded_to(&root.join(SBOM_PATH), "solver SPDX SBOM", MAX_SBOM_BYTES)?;
    require_canonical_json(&bytes, "solver SPDX SBOM")?;
    let document: SpdxDocument = parse_bounded_json(&bytes, "solver SPDX SBOM")?;
    ensure!(
        document.spdx_version == "SPDX-2.3"
            && document.data_license == "CC0-1.0"
            && document.spdx_id == "SPDXRef-DOCUMENT"
            && document.name == format!("eutheto-solver-{}", manifest.build.target_triple),
        "solver SBOM document identity does not match the SPDX-2.3 contract"
    );
    validate_source_date(&document.creation_info.created)?;
    ensure!(
        document.creation_info.creators == ["Tool: eutheto-xtask-solver-finalizer".to_owned()],
        "solver SBOM creator does not match the reviewed tool identity"
    );

    let mut expected_files = BTreeMap::new();
    expected_files.insert(
        manifest.worker.executable.path.clone(),
        manifest.worker.executable.sha256.clone(),
    );
    expected_files.extend(
        manifest
            .runtime_libraries
            .iter()
            .map(|item| (item.path.clone(), item.sha256.clone())),
    );
    expected_files.extend(
        manifest
            .licenses
            .iter()
            .map(|item| (item.path.clone(), item.sha256.clone())),
    );
    expected_files.insert(
        NOTICE_PATH.to_owned(),
        sha256_file(&root.join(NOTICE_PATH))?,
    );
    let digest_files: Vec<(String, String)> = expected_files
        .iter()
        .map(|(path, digest)| (path.clone(), digest.clone()))
        .collect();
    let namespace_digest = pre_sbom_digest(
        &manifest.build.target_triple,
        &document.creation_info.created,
        &manifest.approval.source_contract_sha256,
        &digest_files,
    )?;
    ensure!(
        document.document_namespace
            == format!(
                "https://eutheto.dev/spdx/solver/{}/{}",
                manifest.build.target_triple, namespace_digest
            ),
        "solver SBOM namespace does not match its deterministic pre-SBOM digest"
    );

    ensure!(
        document.files.len() == expected_files.len(),
        "solver SBOM file inventory has the wrong size"
    );
    for (index, (file, (path, expected_hash))) in
        document.files.iter().zip(expected_files.iter()).enumerate()
    {
        ensure!(
            file.file_name == *path
                && file.spdx_id == format!("SPDXRef-File-{index:04}")
                && file.checksums
                    == [SpdxChecksum {
                        algorithm: "SHA256".to_owned(),
                        checksum_value: expected_hash.clone(),
                    }]
                && file.copyright_text == "NOASSERTION",
            "solver SBOM file entry does not match artifact bytes for {path}"
        );
        let expected_license = LICENSE_PROFILE
            .iter()
            .find_map(|(license_path, spdx)| (*license_path == path).then_some(*spdx))
            .unwrap_or("NOASSERTION");
        ensure!(
            file.license_concluded == expected_license
                && file.license_info_in_files == [expected_license.to_owned()]
                && file.file_types
                    == [
                        if path == &manifest.worker.executable.path
                            || manifest
                                .runtime_libraries
                                .iter()
                                .any(|item| item.path == *path)
                        {
                            "BINARY".to_owned()
                        } else {
                            "TEXT".to_owned()
                        }
                    ],
            "solver SBOM classification is incorrect for {path}"
        );
    }
    validate_spdx_packages(&document.packages, manifest, &namespace_digest)?;
    validate_spdx_relationships(&document.relationships, &document.files, manifest)
}

fn validate_spdx_packages(
    packages: &[SpdxPackage],
    manifest: &SolverManifest,
    namespace_digest: &str,
) -> Result<()> {
    let package = spdx_package;
    let mut expected = vec![
        package(
            "SPDXRef-Package-abseil",
            "abseil-cpp",
            ABSEIL_VERSION,
            ABSEIL_URL,
            ABSEIL_SHA256,
            "Apache-2.0",
        ),
        package(
            "SPDXRef-Package-bzip2",
            "bzip2",
            BZIP2_VERSION,
            BZIP2_URL,
            BZIP2_SHA256,
            "bzip2-1.0.6",
        ),
        package(
            "SPDXRef-Package-eutheto",
            "eutheto-ortools-worker",
            &manifest.worker.version,
            "NOASSERTION",
            namespace_digest,
            "Apache-2.0",
        ),
        package(
            "SPDXRef-Package-ortools",
            "OR-Tools",
            &manifest.backend_source.version,
            &manifest.backend_source.source_url,
            &manifest.backend_source.sha256,
            "Apache-2.0",
        ),
        package(
            "SPDXRef-Package-protobuf",
            "protobuf",
            &manifest.protobuf.source_version,
            &manifest.protobuf.source_url,
            &manifest.protobuf.sha256,
            "BSD-3-Clause",
        ),
        package(
            "SPDXRef-Package-re2",
            "RE2",
            RE2_VERSION,
            RE2_URL,
            RE2_SHA256,
            "BSD-3-Clause",
        ),
        package(
            "SPDXRef-Package-utf8-range",
            "utf8_range",
            &manifest.protobuf.source_version,
            &manifest.protobuf.source_url,
            &manifest.protobuf.sha256,
            "MIT",
        ),
        package(
            "SPDXRef-Package-zlib",
            "zlib",
            ZLIB_VERSION,
            ZLIB_URL,
            ZLIB_SHA256,
            "Zlib",
        ),
    ];
    if manifest.build.target_triple == "x86_64-pc-windows-msvc" {
        expected.push(spdx_unverified_package(
            "SPDXRef-Package-msvc-runtime",
            "Microsoft Visual C++ Runtime",
        ));
    }
    ensure!(
        packages == expected,
        "solver SBOM package authority profile does not match reviewed sources"
    );
    Ok(())
}
fn spdx_package(
    id: &str,
    name: &str,
    version: &str,
    url: &str,
    sha256: &str,
    license: &str,
) -> SpdxPackage {
    SpdxPackage {
        spdx_id: id.to_owned(),
        checksums: vec![SpdxChecksum {
            algorithm: "SHA256".to_owned(),
            checksum_value: sha256.to_owned(),
        }],
        download_location: url.to_owned(),
        files_analyzed: false,
        license_concluded: license.to_owned(),
        license_declared: license.to_owned(),
        name: name.to_owned(),
        version_info: Some(version.to_owned()),
    }
}

fn spdx_unverified_package(id: &str, name: &str) -> SpdxPackage {
    SpdxPackage {
        spdx_id: id.to_owned(),
        checksums: Vec::new(),
        download_location: "NOASSERTION".to_owned(),
        files_analyzed: false,
        license_concluded: "NOASSERTION".to_owned(),
        license_declared: "NOASSERTION".to_owned(),
        name: name.to_owned(),
        version_info: None,
    }
}

fn is_msvc_runtime_file(path: &str) -> bool {
    let Some(file_name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_ascii_lowercase();
    Path::new(&file_name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        && ["concrt", "msvcp", "vcomp", "vcruntime"]
            .iter()
            .any(|prefix| file_name.starts_with(prefix))
}

fn validate_spdx_relationships(
    relationships: &[SpdxRelationship],
    files: &[SpdxFile],
    manifest: &SolverManifest,
) -> Result<()> {
    let mut expected = vec![
        SpdxRelationship {
            spdx_element_id: "SPDXRef-DOCUMENT".to_owned(),
            relationship_type: "DESCRIBES".to_owned(),
            related_spdx_element: "SPDXRef-Package-eutheto".to_owned(),
        },
        SpdxRelationship {
            spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
            relationship_type: "STATIC_LINK".to_owned(),
            related_spdx_element: "SPDXRef-Package-ortools".to_owned(),
        },
    ];
    for package in ["abseil", "bzip2", "protobuf", "re2", "utf8-range", "zlib"] {
        expected.push(SpdxRelationship {
            spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
            relationship_type: "DYNAMIC_LINK".to_owned(),
            related_spdx_element: format!("SPDXRef-Package-{package}"),
        });
    }
    if manifest.build.target_triple == "x86_64-pc-windows-msvc" {
        expected.push(SpdxRelationship {
            spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
            relationship_type: "DYNAMIC_LINK".to_owned(),
            related_spdx_element: "SPDXRef-Package-msvc-runtime".to_owned(),
        });
    }
    expected.push(SpdxRelationship {
        spdx_element_id: "SPDXRef-Package-protobuf".to_owned(),
        relationship_type: "CONTAINS".to_owned(),
        related_spdx_element: "SPDXRef-Package-utf8-range".to_owned(),
    });
    expected.extend(files.iter().map(|file| {
        if manifest.build.target_triple == "x86_64-pc-windows-msvc"
            && is_msvc_runtime_file(&file.file_name)
        {
            SpdxRelationship {
                spdx_element_id: file.spdx_id.clone(),
                relationship_type: "GENERATED_FROM".to_owned(),
                related_spdx_element: "SPDXRef-Package-msvc-runtime".to_owned(),
            }
        } else {
            SpdxRelationship {
                spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
                relationship_type: "CONTAINS".to_owned(),
                related_spdx_element: file.spdx_id.clone(),
            }
        }
    }));
    ensure!(
        relationships == expected,
        "solver SBOM relationships do not match the reviewed linkage and file profile"
    );
    Ok(())
}

pub(crate) fn pre_sbom_digest(
    target: &str,
    source_date: &str,
    source_contract_sha256: &str,
    files: &[(String, String)],
) -> Result<String> {
    validate_source_date(source_date)?;
    architecture_for_target(target)?;
    validate_hash(source_contract_sha256, "source contract")?;
    let seed = serde_json::json!({
        "files": files,
        "source_contract_sha256": source_contract_sha256,
        "source_date": source_date,
        "target": target,
    });
    Ok(sha256_bytes(&canonical_bytes(&seed)?))
}

pub(crate) fn validate_source_date(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 20
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes[19] == b'Z'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16 | 19)
                    || byte.is_ascii_digit()),
        "source date must be strict UTC YYYY-MM-DDTHH:MM:SSZ"
    );
    let number = |range: std::ops::Range<usize>| -> Result<u32> {
        std::str::from_utf8(&bytes[range])
            .context("source date contains non-UTF-8 bytes")?
            .parse()
            .context("source date contains a non-numeric component")
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    ensure!(
        year >= 1970 && (1..=12).contains(&month),
        "source date is out of range"
    );
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    ensure!(
        day >= 1
            && day <= days[usize::try_from(month - 1).context("invalid source month")?]
            && hour <= 23
            && minute <= 59
            && second <= 59,
        "source date is out of range"
    );
    Ok(())
}

fn architecture_for_target(target: &str) -> Result<&'static str> {
    match target {
        "x86_64-unknown-linux-gnu" | "x86_64-apple-darwin" | "x86_64-pc-windows-msvc" => {
            Ok("x86_64")
        }
        "aarch64-apple-darwin" => Ok("aarch64"),
        _ => bail!("unsupported solver target triple: {target}"),
    }
}
fn expected_worker_path(target: &str) -> Result<&'static str> {
    architecture_for_target(target)?;
    Ok(if target == "x86_64-pc-windows-msvc" {
        "bin/ortools-worker.exe"
    } else {
        "bin/ortools-worker"
    })
}

fn validate_runtime_library_path(path: &str, target: &str) -> Result<()> {
    let leaf = path
        .rsplit('/')
        .next()
        .context("runtime library path has no leaf")?;
    ensure!(
        !leaf.is_empty()
            && leaf.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
            }),
        "runtime library path has an unsafe filename"
    );
    let valid = match target {
        "x86_64-pc-windows-msvc" => path.starts_with("bin/") && has_file_extension(leaf, "dll"),
        "x86_64-unknown-linux-gnu" => {
            path.starts_with("lib/")
                && (has_file_extension(leaf, "so")
                    || leaf.split_once(".so.").is_some_and(|(_, version)| {
                        !version.is_empty()
                            && version.split('.').all(|part| {
                                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                            })
                    }))
        }
        "x86_64-apple-darwin" | "aarch64-apple-darwin" => {
            path.starts_with("lib/") && has_file_extension(leaf, "dylib")
        }
        _ => false,
    };
    ensure!(
        valid,
        "runtime library path does not match the target contract"
    );
    Ok(())
}
fn validate_msvc_runtime_closure<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    target: &str,
) -> Result<()> {
    if target != "x86_64-pc-windows-msvc" {
        return Ok(());
    }
    let mut has_msvcp = false;
    let mut has_vcruntime = false;
    for path in paths {
        let file_name = path.rsplit('/').next().unwrap_or_default();
        has_msvcp |= file_name.to_ascii_lowercase().starts_with("msvcp");
        has_vcruntime |= file_name.to_ascii_lowercase().starts_with("vcruntime");
    }
    ensure!(
        has_msvcp && has_vcruntime,
        "Windows MultiThreadedDLL artifacts must bundle the imported MSVC C++ runtime closure"
    );
    Ok(())
}

fn has_file_extension(name: &str, expected: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case(expected))
}

fn validate_capabilities(values: &[String]) -> Result<()> {
    ensure!(
        values.len() <= MAX_CAPABILITIES,
        "capability inventory exceeds {MAX_CAPABILITIES} entries"
    );
    validate_sorted_unique_strings(values, "capabilities")
}

fn validate_sorted_unique_paths(values: &[String], name: &str, maximum: usize) -> Result<()> {
    ensure!(values.len() <= maximum, "{name} exceeds {maximum} entries");
    validate_sorted_unique_strings(values, name)?;
    for value in values {
        validate_relative_path(value, name)?;
    }
    Ok(())
}

fn validate_sorted_unique_strings(values: &[String], name: &str) -> Result<()> {
    for pair in values.windows(2) {
        ensure!(
            pair[0] < pair[1],
            "{name} must be lexically sorted and unique"
        );
    }
    Ok(())
}

trait HasPath {
    fn path(&self) -> &str;
}

fn validate_compiler(compiler: &Compiler) -> Result<()> {
    ensure!(
        matches!(compiler.identity.as_str(), "clang" | "gcc" | "msvc"),
        "compiler identity must be clang, gcc, or msvc"
    );
    ensure!(
        !compiler.version.is_empty()
            && compiler.version.len() <= 64
            && compiler.version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
            }),
        "compiler version must be a normalized ASCII version"
    );
    Ok(())
}

fn validate_sorted_by_path<T: HasPath>(values: &[T], name: &str) -> Result<()> {
    for pair in values.windows(2) {
        ensure!(
            pair[0].path() < pair[1].path(),
            "{name} must be sorted by path and unique"
        );
    }
    Ok(())
}

fn validate_relative_path(path: &str, name: &str) -> Result<()> {
    ensure!(!path.is_empty(), "{name} must not be empty");
    ensure!(path.len() <= MAX_STRING_BYTES, "{name} is too long");
    ensure!(!path.contains('\\'), "{name} must use forward slashes");
    ensure!(
        !path.starts_with('/') && !path.starts_with('~'),
        "{name} must be bundle-relative"
    );
    let lowercase = path.to_ascii_lowercase();
    ensure!(
        !lowercase.contains("$home")
            && !lowercase.contains("${home}")
            && !lowercase.contains("%userprofile%"),
        "{name} contains a home-directory expansion"
    );
    ensure!(
        !path.contains(':') && !path.chars().any(char::is_control),
        "{name} contains a forbidden path prefix or control character"
    );
    for component in path.split('/') {
        ensure!(
            !component.is_empty() && component != "." && component != "..",
            "{name} contains an unsafe component"
        );
    }
    Ok(())
}

fn validate_cmake_map(map: &CmakeMap, name: &str) -> Result<()> {
    ensure!(
        map.len() <= MAX_CMAKE_ENTRIES,
        "{name} exceeds {MAX_CMAKE_ENTRIES} entries"
    );
    for (key, value) in map {
        let mut bytes = key.bytes();
        ensure!(
            key.len() <= 128
                && bytes
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| {
                    byte.is_ascii_alphabetic() || byte.is_ascii_digit() || byte == b'_'
                }),
            "{name} has an invalid key {key:?}"
        );
        if let CacheValue::String(value) = value {
            validate_cmake_string(value, name, key)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CmakeScope {
    Ortools,
    Worker,
}

fn validate_exact_cmake_keys(
    map: &CmakeMap,
    approved: &CmakeMap,
    measured: &[&str],
    target: &str,
    name: &str,
) -> Result<()> {
    validate_cmake_map(map, name)?;
    let mut expected: BTreeSet<&str> = approved.keys().map(String::as_str).collect();
    expected.extend(measured.iter().copied());
    if target.ends_with("-apple-darwin") {
        expected.extend(DARWIN_CMAKE_KEYS);
    }
    if target == "x86_64-pc-windows-msvc" {
        expected.extend(WINDOWS_CMAKE_KEYS);
    }
    let actual: BTreeSet<&str> = map.keys().map(String::as_str).collect();
    ensure!(
        actual == expected,
        "{name} keys do not match the exact reviewed target-specific set"
    );
    for (key, value) in approved {
        ensure!(
            map.get(key) == Some(value),
            "{name} entry {key} differs from the approved source contract"
        );
    }
    Ok(())
}

fn validate_reviewed_cmake_values(
    map: &CmakeMap,
    target: &str,
    scope: CmakeScope,
    name: &str,
) -> Result<()> {
    let windows = target == "x86_64-pc-windows-msvc";
    require_cmake_string(map, "CMAKE_GENERATOR", "Ninja", name)?;
    require_cmake_string(
        map,
        "CMAKE_INSTALL_PREFIX",
        match scope {
            CmakeScope::Ortools => "@target-root@",
            CmakeScope::Worker => "@artifact-root@",
        },
        name,
    )?;
    if windows {
        require_cmake_string(map, "CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL", name)?;
    }
    match scope {
        CmakeScope::Ortools => validate_ortools_cmake_values(map, windows, name)?,
        CmakeScope::Worker => validate_worker_cmake_values(map, target, name)?,
    }
    if target.ends_with("-apple-darwin") {
        require_cmake_string(map, "CMAKE_INSTALL_NAME_DIR", "@rpath", name)?;
        require_cmake_string(map, "CMAKE_INSTALL_RPATH", "@artifact-root@/lib", name)?;
        require_cmake_bool(map, "CMAKE_MACOSX_RPATH", true, name)?;
    }
    Ok(())
}
fn validate_ortools_cmake_values(map: &CmakeMap, windows: bool, name: &str) -> Result<()> {
    require_tokenized_compiler(map, "CMAKE_C_COMPILER", name)?;
    require_tokenized_compiler(map, "CMAKE_CXX_COMPILER", name)?;
    require_cmake_string(
        map,
        "CMAKE_CXX_FLAGS",
        if windows {
            "/DEIGEN_MPL2_ONLY"
        } else {
            "-DEIGEN_MPL2_ONLY"
        },
        name,
    )?;
    require_cmake_string(map, "CMAKE_POLICY_DEFAULT_CMP0077", "NEW", name)?;
    require_cmake_bool(map, "FETCHCONTENT_FULLY_DISCONNECTED", true, name)?;
    for (key, path) in [
        (
            "FETCHCONTENT_SOURCE_DIR_ABSL",
            "@source-root@/sources/abseil-cpp-20250814.1",
        ),
        (
            "FETCHCONTENT_SOURCE_DIR_BZIP2",
            "@source-root@/sources/bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
        ),
        (
            "FETCHCONTENT_SOURCE_DIR_EIGEN3",
            "@source-root@/sources/eigen-3.4.0",
        ),
        (
            "FETCHCONTENT_SOURCE_DIR_PROTOBUF",
            "@source-root@/sources/protobuf-33.1",
        ),
        (
            "FETCHCONTENT_SOURCE_DIR_RE2",
            "@source-root@/sources/re2-2025-08-12",
        ),
        (
            "FETCHCONTENT_SOURCE_DIR_ZLIB",
            "@source-root@/sources/zlib-1.3.1",
        ),
    ] {
        require_cmake_string(map, key, path, name)?;
    }
    Ok(())
}

fn validate_worker_cmake_values(map: &CmakeMap, target: &str, name: &str) -> Result<()> {
    require_tokenized_compiler(map, "CMAKE_CXX_COMPILER", name)?;
    require_cmake_string(map, "CMAKE_PREFIX_PATH", "@target-root@", name)?;
    let library_directory = if target == "x86_64-unknown-linux-gnu" {
        "lib64"
    } else {
        "lib"
    };
    require_cmake_string(
        map,
        "ortools_DIR",
        &format!("@target-root@/{library_directory}/cmake/ortools"),
        name,
    )?;
    require_cmake_string(
        map,
        "Protobuf_DIR",
        &format!("@target-root@/{library_directory}/cmake/protobuf"),
        name,
    )?;
    require_cmake_string(
        map,
        "EUTHETO_ORTOOLS_PHASE3_CONTRACT",
        "@source-root@/workers/ortools/source-contract.json",
        name,
    )?;
    for key in [
        "CMAKE_FIND_USE_CMAKE_ENVIRONMENT_PATH",
        "CMAKE_FIND_USE_PACKAGE_REGISTRY",
        "CMAKE_FIND_USE_PACKAGE_ROOT_PATH",
        "CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY",
        "EUTHETO_ORTOOLS_BUILD_CANDIDATE_BENCHMARKS",
        "EUTHETO_ORTOOLS_DEVELOPMENT_BUILD",
    ] {
        require_cmake_bool(map, key, false, name)?;
    }
    require_cmake_bool(map, "EUTHETO_ORTOOLS_BUILD_TESTS", true, name)
}

fn require_cmake_string(map: &CmakeMap, key: &str, expected: &str, name: &str) -> Result<()> {
    ensure!(
        matches!(map.get(key), Some(CacheValue::String(value)) if value == expected),
        "{name} entry {key} does not match the reviewed value"
    );
    Ok(())
}

fn require_cmake_bool(map: &CmakeMap, key: &str, expected: bool, name: &str) -> Result<()> {
    ensure!(
        matches!(map.get(key), Some(CacheValue::Boolean(value)) if *value == expected),
        "{name} entry {key} does not match the reviewed Boolean value"
    );
    Ok(())
}

fn require_tokenized_compiler(map: &CmakeMap, key: &str, name: &str) -> Result<()> {
    ensure!(
        matches!(
            map.get(key),
            Some(CacheValue::String(value))
                if value.strip_prefix("@toolchain-root@/").is_some_and(|leaf| {
                    !leaf.is_empty()
                        && !leaf.contains('/')
                        && !leaf.chars().any(char::is_whitespace)
                })
        ),
        "{name} entry {key} must be a normalized toolchain leaf path"
    );
    Ok(())
}

fn validate_cmake_string(value: &str, name: &str, key: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_STRING_BYTES,
        "{name} value for {key} is too long"
    );
    ensure!(
        !value.contains('\\') && !value.chars().any(char::is_control),
        "{name} value for {key} contains a forbidden path spelling"
    );
    let lowercase = value.to_ascii_lowercase();
    ensure!(
        !lowercase.contains("$home")
            && !lowercase.contains("${home}")
            && !lowercase.contains("%userprofile%")
            && !lowercase.contains("/home/"),
        "{name} value for {key} contains a build-host/home path"
    );
    if is_cmake_path_key(key) {
        validate_cmake_path_string(value, name, key)?;
    } else if value.contains('/') {
        ensure!(
            key == "CMAKE_CXX_FLAGS" && matches!(value, "-DEIGEN_MPL2_ONLY" | "/DEIGEN_MPL2_ONLY"),
            "{name} value for {key} contains an unnormalized path or unsupported flag"
        );
    } else {
        ensure!(
            !(value.len() >= 2
                && value.as_bytes()[0].is_ascii_alphabetic()
                && value.as_bytes()[1] == b':'),
            "{name} value for {key} contains an absolute path"
        );
    }
    for piece in value
        .split(|character: char| character.is_whitespace() || character == ';' || character == '=')
    {
        if piece.starts_with('@') {
            ensure!(
                PATH_TOKENS
                    .iter()
                    .any(|token| matches_path_token(piece, token)),
                "{name} value for {key} uses an unknown @ path token"
            );
        }
    }
    Ok(())
}

fn is_cmake_path_key(key: &str) -> bool {
    key.starts_with("FETCHCONTENT_SOURCE_DIR_")
        || matches!(
            key,
            "CMAKE_C_COMPILER"
                | "CMAKE_CXX_COMPILER"
                | "CMAKE_INSTALL_NAME_DIR"
                | "CMAKE_INSTALL_PREFIX"
                | "CMAKE_INSTALL_RPATH"
                | "CMAKE_PREFIX_PATH"
                | "EUTHETO_ORTOOLS_PHASE3_CONTRACT"
                | "Protobuf_DIR"
                | "ortools_DIR"
        )
}

fn validate_cmake_path_string(value: &str, name: &str, key: &str) -> Result<()> {
    let Some(token) = PATH_TOKENS
        .iter()
        .find(|token| matches_path_token(value, token))
    else {
        bail!("{name} path value for {key} must start with a documented @ token");
    };
    let remainder = value
        .strip_prefix(token)
        .context("matched CMake path token disappeared")?;
    if let Some(path) = remainder.strip_prefix('/') {
        for component in path.split('/') {
            ensure!(
                !component.is_empty()
                    && component != "."
                    && component != ".."
                    && !component.chars().any(|character| matches!(
                        character,
                        ':' | '@' | '\\' | ';' | '=' | '$' | '%' | '~'
                    ))
                    && !component.chars().any(char::is_whitespace),
                "{name} path value for {key} contains an unsafe component"
            );
        }
    } else {
        ensure!(
            remainder.is_empty(),
            "{name} path value for {key} has invalid text after its @ token"
        );
    }
    Ok(())
}

fn matches_path_token(value: &str, token: &str) -> bool {
    value
        .strip_prefix(token)
        .is_some_and(|remainder| remainder.is_empty() || remainder.starts_with('/'))
}

fn validate_nonempty(value: &str, name: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{name} must not be empty");
    ensure!(
        value.len() <= MAX_STRING_BYTES,
        "{name} exceeds {MAX_STRING_BYTES} bytes"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "{name} contains a control character"
    );
    Ok(())
}

fn validate_hash(value: &str, name: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} SHA-256 must be 64 lowercase hexadecimal characters"
    );
    Ok(())
}

fn read_bounded(path: &Path, name: &str) -> Result<Vec<u8>> {
    read_bounded_to(path, name, MAX_INPUT_BYTES)
}

fn read_bounded_to(path: &Path, name: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect {name} {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "{name} is not a regular file: {}",
        path.display()
    );
    let max_bytes_on_disk = max_bytes as u64;
    ensure!(
        metadata.len() <= max_bytes_on_disk,
        "{name} exceeds {max_bytes} bytes"
    );
    let capacity =
        usize::try_from(metadata.len()).context("input length does not fit memory size")?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)
        .with_context(|| format!("failed to open {name} {}", path.display()))?
        .take(max_bytes_on_disk + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {name} {}", path.display()))?;
    ensure!(
        bytes.len() <= max_bytes,
        "{name} changed while reading and exceeds {max_bytes} bytes"
    );
    Ok(bytes)
}

fn parse_bounded_json<T: for<'de> Deserialize<'de>>(bytes: &[u8], name: &str) -> Result<T> {
    ensure!(
        !bytes.starts_with(&[0xef, 0xbb, 0xbf]),
        "{name} must not contain a UTF-8 byte-order mark"
    );
    let text = std::str::from_utf8(bytes).with_context(|| format!("{name} is not UTF-8"))?;
    prescan_json(text, name)?;
    let value: serde_json::Value =
        serde_json::from_str(text).with_context(|| format!("failed to parse {name}"))?;
    validate_json_value(&value, 1, name)?;
    serde_json::from_str(text).with_context(|| format!("invalid {name}"))
}

fn prescan_json(text: &str, name: &str) -> Result<()> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut string_bytes = 0usize;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                ensure!(
                    string_bytes <= MAX_STRING_BYTES * 6,
                    "{name} contains an overlong encoded string"
                );
                string_bytes = 0;
            } else {
                string_bytes = string_bytes
                    .checked_add(1)
                    .context("JSON string length overflow")?;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'{' || byte == b'[' {
            depth = depth.checked_add(1).context("JSON depth overflow")?;
            ensure!(
                depth <= MAX_DEPTH,
                "{name} exceeds maximum JSON nesting depth {MAX_DEPTH}"
            );
        } else if byte == b'}' || byte == b']' {
            depth = depth.checked_sub(1).context("unbalanced JSON delimiters")?;
        }
    }
    ensure!(
        !in_string && depth == 0,
        "{name} has unterminated JSON structure"
    );
    Ok(())
}

fn validate_json_value(value: &serde_json::Value, depth: usize, name: &str) -> Result<()> {
    ensure!(
        depth <= MAX_DEPTH,
        "{name} exceeds maximum JSON nesting depth {MAX_DEPTH}"
    );
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) => {}
        serde_json::Value::Number(number) => ensure!(
            number.is_i64() || number.is_u64(),
            "{name} contains a non-integer number"
        ),
        serde_json::Value::String(value) => ensure!(
            value.len() <= MAX_STRING_BYTES,
            "{name} contains a string exceeding {MAX_STRING_BYTES} UTF-8 bytes"
        ),
        serde_json::Value::Array(values) => {
            ensure!(
                values.len() <= MAX_ARRAY_ENTRIES,
                "{name} contains an array exceeding {MAX_ARRAY_ENTRIES} entries"
            );
            for value in values {
                validate_json_value(value, depth + 1, name)?;
            }
        }
        serde_json::Value::Object(values) => {
            ensure!(
                values.len() <= MAX_OBJECT_MEMBERS,
                "{name} contains an object exceeding {MAX_OBJECT_MEMBERS} members"
            );
            for (key, value) in values {
                ensure!(
                    key.len() <= MAX_STRING_BYTES,
                    "{name} contains an overlong object key"
                );
                validate_json_value(value, depth + 1, name)?;
            }
        }
    }
    Ok(())
}

fn require_canonical_json(bytes: &[u8], name: &str) -> Result<()> {
    let value: serde_json::Value = parse_bounded_json(bytes, name)?;
    ensure!(
        bytes == canonical_bytes(&value)?,
        "{name} is not canonical JSON"
    );
    Ok(())
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context("failed to normalize canonical JSON")?;
    let mut bytes = serde_json::to_vec_pretty(&value).context("failed to encode canonical JSON")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    crate::hex_bytes(&Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(
        crate::solver_artifact::open_read_no_follow(path)
            .with_context(|| format!("failed to open artifact {}", path.display()))?,
    );
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .with_context(|| format!("failed to hash artifact {}", path.display()))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(crate::hex_bytes(&digest.finalize()))
}

type CmakeMap = BTreeMap<String, CacheValue>;

fn deserialize_cmake_map<'de, D>(deserializer: D) -> std::result::Result<CmakeMap, D::Error>
where
    D: Deserializer<'de>,
{
    struct CmakeMapVisitor;

    impl<'de> Visitor<'de> for CmakeMapVisitor {
        type Value = CmakeMap;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a CMake cache object with unique keys")
        }

        fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = access.next_entry::<String, CacheValue>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(de::Error::custom(format!("duplicate CMake key {key}")));
                }
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(CmakeMapVisitor)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum CacheValue {
    Boolean(bool),
    Integer(i64),
    String(String),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpdxDocument {
    #[serde(rename = "SPDXID")]
    spdx_id: String,
    #[serde(rename = "creationInfo")]
    creation_info: SpdxCreationInfo,
    #[serde(rename = "dataLicense")]
    data_license: String,
    #[serde(rename = "documentNamespace")]
    document_namespace: String,
    files: Vec<SpdxFile>,
    name: String,
    packages: Vec<SpdxPackage>,
    relationships: Vec<SpdxRelationship>,
    #[serde(rename = "spdxVersion")]
    spdx_version: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpdxCreationInfo {
    created: String,
    creators: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpdxFile {
    #[serde(rename = "SPDXID")]
    spdx_id: String,
    checksums: Vec<SpdxChecksum>,
    #[serde(rename = "copyrightText")]
    copyright_text: String,
    #[serde(rename = "fileName")]
    file_name: String,
    #[serde(rename = "fileTypes")]
    file_types: Vec<String>,
    #[serde(rename = "licenseConcluded")]
    license_concluded: String,
    #[serde(rename = "licenseInfoInFiles")]
    license_info_in_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpdxChecksum {
    algorithm: String,
    #[serde(rename = "checksumValue")]
    checksum_value: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpdxPackage {
    #[serde(rename = "SPDXID")]
    spdx_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    checksums: Vec<SpdxChecksum>,
    #[serde(rename = "downloadLocation")]
    download_location: String,
    #[serde(rename = "filesAnalyzed")]
    files_analyzed: bool,
    #[serde(rename = "licenseConcluded")]
    license_concluded: String,
    #[serde(rename = "licenseDeclared")]
    license_declared: String,
    name: String,
    #[serde(
        default,
        rename = "versionInfo",
        skip_serializing_if = "Option::is_none"
    )]
    version_info: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpdxRelationship {
    #[serde(rename = "spdxElementId")]
    spdx_element_id: String,
    #[serde(rename = "relationshipType")]
    relationship_type: String,
    #[serde(rename = "relatedSpdxElement")]
    related_spdx_element: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceContract {
    approval: SourceApproval,
    cmake: SourceCmake,
    ortools: SourceOrtools,
    protobuf: SourceProtobuf,
    protocol: SourceProtocol,
    schema_version: u32,
    worker: SourceWorker,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceApproval {
    phase: u32,
    record: String,
    status: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCmake {
    #[serde(deserialize_with = "deserialize_cmake_map")]
    cache_entries: CmakeMap,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceOrtools {
    patch_path: String,
    patch_sha256: String,
    sha256: String,
    source_url: String,
    version: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceProtobuf {
    cpp_runtime_version: String,
    protoc_version: String,
    sha256: String,
    source_url: String,
    source_version: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceProtocol {
    schema_sha256: String,
    wire_version: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceWorker {
    identity: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolPolicy {
    #[serde(rename = "applied_parameters_hash")]
    _applied_parameters_hash: serde_json::Value,
    #[serde(rename = "compatibility")]
    _compatibility: serde_json::Value,
    #[serde(rename = "field_limits")]
    _field_limits: serde_json::Value,
    #[serde(rename = "frame_classes")]
    _frame_classes: serde_json::Value,
    #[serde(rename = "framing")]
    _framing: serde_json::Value,
    #[serde(rename = "limits")]
    _limits: serde_json::Value,
    package: String,
    protocol: String,
    version: ProtocolVersion,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolVersion {
    major: u32,
    minor: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildEvidence {
    architecture: String,
    cmake: MeasuredCmake,
    compiler: Compiler,
    linkage: String,
    runtime_library_paths: Vec<String>,
    schema_version: u32,
    target_triple: String,
    worker: TestedWorker,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasuredCmake {
    #[serde(deserialize_with = "deserialize_cmake_map")]
    ortools: CmakeMap,
    #[serde(deserialize_with = "deserialize_cmake_map")]
    worker: CmakeMap,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Compiler {
    identity: String,
    version: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestedWorker {
    adapter_version: String,
    backend_id: String,
    capabilities: Vec<String>,
    executable_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadEvidence {
    licenses: Vec<PayloadLicense>,
    sbom_path: String,
    schema_version: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadLicense {
    path: String,
    spdx: String,
}
impl HasPath for PayloadLicense {
    fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolverManifest {
    approval: Approval,
    backend_source: BackendSource,
    build: Build,
    capabilities: Vec<String>,
    licenses: Vec<LicenseDigest>,
    manifest: ManifestVersion,
    protobuf: Protobuf,
    protocol: Protocol,
    runtime_libraries: Vec<FileDigest>,
    sbom: FileDigest,
    worker: Worker,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    phase: u32,
    record: String,
    source_contract_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendSource {
    kind: String,
    sha256: String,
    source_url: String,
    version: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    architecture: String,
    cmake: MeasuredCmake,
    compiler: Compiler,
    linkage: String,
    target_triple: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LicenseDigest {
    path: String,
    sha256: String,
    spdx: String,
}
impl HasPath for LicenseDigest {
    fn path(&self) -> &str {
        &self.path
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestVersion {
    generation_contract_version: u32,
    schema_version: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Protobuf {
    approved_proto_checksums: ApprovedProtoChecksums,
    cpp_runtime_version: String,
    protoc_version: String,
    sha256: String,
    source_url: String,
    source_version: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovedProtoChecksums {
    cp_model_proto: String,
    sat_parameters_proto: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Protocol {
    major: u32,
    minor: u32,
    schema_sha256: String,
    wire_version: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDigest {
    path: String,
    sha256: String,
}
impl HasPath for FileDigest {
    fn path(&self) -> &str {
        &self.path
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Worker {
    adapter_version: String,
    backend_id: String,
    distribution: String,
    executable: FileDigest,
    identity: String,
    stability: String,
    version: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        _temporary: TempDir,
        artifact_root: PathBuf,
        build_evidence: PathBuf,
        payload_evidence: PathBuf,
        protocol_policy: PathBuf,
        protocol_schema: PathBuf,
        source_contract: PathBuf,
    }

    impl Fixture {
        fn options(&self) -> AssembleOptions<'_> {
            AssembleOptions {
                source_contract: &self.source_contract,
                protocol_schema: &self.protocol_schema,
                protocol_policy: &self.protocol_policy,
                build_evidence: &self.build_evidence,
                payload_evidence: &self.payload_evidence,
                artifact_root: &self.artifact_root,
            }
        }

        fn validation_options(&self) -> ValidateOptions<'_> {
            ValidateOptions {
                source_contract: &self.source_contract,
                protocol_schema: &self.protocol_schema,
                protocol_policy: &self.protocol_policy,
                artifact_root: &self.artifact_root,
            }
        }

        fn manifest_path(&self) -> PathBuf {
            self.artifact_root.join("solver-manifest.json")
        }
    }

    fn approved_cmake() -> CmakeMap {
        [
            ("BUILD_CXX", CacheValue::Boolean(true)),
            ("BUILD_CXX_EXAMPLES", CacheValue::Boolean(false)),
            ("BUILD_CXX_SAMPLES", CacheValue::Boolean(false)),
            ("BUILD_DEPS", CacheValue::Boolean(true)),
            ("BUILD_DOC", CacheValue::Boolean(false)),
            ("BUILD_DOTNET", CacheValue::Boolean(false)),
            ("BUILD_EXAMPLES", CacheValue::Boolean(false)),
            ("BUILD_FLATZINC", CacheValue::Boolean(false)),
            ("BUILD_JAVA", CacheValue::Boolean(false)),
            ("BUILD_MATH_OPT", CacheValue::Boolean(false)),
            ("BUILD_PYTHON", CacheValue::Boolean(false)),
            ("BUILD_SAMPLES", CacheValue::Boolean(false)),
            ("BUILD_SHARED_LIBS", CacheValue::Boolean(false)),
            ("BUILD_TESTING", CacheValue::Boolean(false)),
            ("CMAKE_BUILD_TYPE", CacheValue::String("Release".to_owned())),
            ("INSTALL_BUILD_DEPS", CacheValue::Boolean(true)),
            ("USE_BOP", CacheValue::Boolean(true)),
            ("USE_COINOR", CacheValue::Boolean(false)),
            ("USE_CPLEX", CacheValue::Boolean(false)),
            ("USE_GLOP", CacheValue::Boolean(true)),
            ("USE_GLPK", CacheValue::Boolean(false)),
            ("USE_GUROBI", CacheValue::Boolean(false)),
            ("USE_HIGHS", CacheValue::Boolean(false)),
            ("USE_PDLP", CacheValue::Boolean(false)),
            ("USE_SCIP", CacheValue::Boolean(false)),
            ("USE_XPRESS", CacheValue::Boolean(false)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
    }

    fn write_json(path: &Path, value: &Value) {
        fs::write(path, canonical_bytes(value).unwrap()).unwrap();
    }
    #[allow(clippy::too_many_lines)]
    fn measured_cmake(target: &str, scope: CmakeScope) -> CmakeMap {
        let mut map = approved_cmake();
        let compiler = CacheValue::String("@toolchain-root@/clang++".to_owned());
        map.insert("CMAKE_CXX_COMPILER".to_owned(), compiler);
        map.insert(
            "CMAKE_GENERATOR".to_owned(),
            CacheValue::String("Ninja".to_owned()),
        );
        map.insert(
            "CMAKE_INSTALL_PREFIX".to_owned(),
            CacheValue::String(
                match scope {
                    CmakeScope::Ortools => "@target-root@",
                    CmakeScope::Worker => "@artifact-root@",
                }
                .to_owned(),
            ),
        );
        match scope {
            CmakeScope::Ortools => {
                map.insert(
                    "CMAKE_C_COMPILER".to_owned(),
                    CacheValue::String("@toolchain-root@/clang".to_owned()),
                );
                map.insert(
                    "CMAKE_CXX_FLAGS".to_owned(),
                    CacheValue::String(
                        if target == "x86_64-pc-windows-msvc" {
                            "/DEIGEN_MPL2_ONLY"
                        } else {
                            "-DEIGEN_MPL2_ONLY"
                        }
                        .to_owned(),
                    ),
                );
                map.insert(
                    "CMAKE_POLICY_DEFAULT_CMP0077".to_owned(),
                    CacheValue::String("NEW".to_owned()),
                );
                map.insert(
                    "FETCHCONTENT_FULLY_DISCONNECTED".to_owned(),
                    CacheValue::Boolean(true),
                );
                for (key, value) in [
                    (
                        "FETCHCONTENT_SOURCE_DIR_ABSL",
                        "@source-root@/sources/abseil-cpp-20250814.1",
                    ),
                    (
                        "FETCHCONTENT_SOURCE_DIR_BZIP2",
                        "@source-root@/sources/bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
                    ),
                    (
                        "FETCHCONTENT_SOURCE_DIR_EIGEN3",
                        "@source-root@/sources/eigen-3.4.0",
                    ),
                    (
                        "FETCHCONTENT_SOURCE_DIR_PROTOBUF",
                        "@source-root@/sources/protobuf-33.1",
                    ),
                    (
                        "FETCHCONTENT_SOURCE_DIR_RE2",
                        "@source-root@/sources/re2-2025-08-12",
                    ),
                    (
                        "FETCHCONTENT_SOURCE_DIR_ZLIB",
                        "@source-root@/sources/zlib-1.3.1",
                    ),
                ] {
                    map.insert(key.to_owned(), CacheValue::String(value.to_owned()));
                }
            }
            CmakeScope::Worker => {
                for key in [
                    "CMAKE_FIND_USE_CMAKE_ENVIRONMENT_PATH",
                    "CMAKE_FIND_USE_PACKAGE_REGISTRY",
                    "CMAKE_FIND_USE_PACKAGE_ROOT_PATH",
                    "CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY",
                    "EUTHETO_ORTOOLS_BUILD_CANDIDATE_BENCHMARKS",
                    "EUTHETO_ORTOOLS_DEVELOPMENT_BUILD",
                ] {
                    map.insert(key.to_owned(), CacheValue::Boolean(false));
                }
                map.insert(
                    "EUTHETO_ORTOOLS_BUILD_TESTS".to_owned(),
                    CacheValue::Boolean(true),
                );
                for (key, value) in [
                    ("CMAKE_PREFIX_PATH", "@target-root@"),
                    (
                        "EUTHETO_ORTOOLS_PHASE3_CONTRACT",
                        "@source-root@/workers/ortools/source-contract.json",
                    ),
                ] {
                    map.insert(key.to_owned(), CacheValue::String(value.to_owned()));
                }
                let library_directory = if target == "x86_64-unknown-linux-gnu" {
                    "lib64"
                } else {
                    "lib"
                };
                map.insert(
                    "Protobuf_DIR".to_owned(),
                    CacheValue::String(format!("@target-root@/{library_directory}/cmake/protobuf")),
                );
                map.insert(
                    "ortools_DIR".to_owned(),
                    CacheValue::String(format!("@target-root@/{library_directory}/cmake/ortools")),
                );
            }
        }
        if target.ends_with("-apple-darwin") {
            map.insert(
                "CMAKE_INSTALL_NAME_DIR".to_owned(),
                CacheValue::String("@rpath".to_owned()),
            );
            map.insert(
                "CMAKE_INSTALL_RPATH".to_owned(),
                CacheValue::String("@artifact-root@/lib".to_owned()),
            );
            map.insert("CMAKE_MACOSX_RPATH".to_owned(), CacheValue::Boolean(true));
        }
        if target == "x86_64-pc-windows-msvc" {
            map.insert(
                "CMAKE_MSVC_RUNTIME_LIBRARY".to_owned(),
                CacheValue::String("MultiThreadedDLL".to_owned()),
            );
        }
        map
    }

    #[allow(clippy::too_many_lines)]
    fn write_spdx_fixture(
        artifact_root: &Path,
        target: &str,
        executable: &str,
        runtime_paths: &[&str],
        source_contract_sha256: &str,
    ) {
        let mut inventory = BTreeMap::new();
        inventory.insert(
            executable.to_owned(),
            sha256_file(&artifact_root.join(executable)).unwrap(),
        );
        for path in runtime_paths {
            inventory.insert(
                (*path).to_owned(),
                sha256_file(&artifact_root.join(path)).unwrap(),
            );
        }
        for (path, _) in LICENSE_PROFILE {
            inventory.insert(
                path.to_owned(),
                sha256_file(&artifact_root.join(path)).unwrap(),
            );
        }
        inventory.insert(
            NOTICE_PATH.to_owned(),
            sha256_file(&artifact_root.join(NOTICE_PATH)).unwrap(),
        );
        let digest_files: Vec<(String, String)> = inventory
            .iter()
            .map(|(path, digest)| (path.clone(), digest.clone()))
            .collect();
        let created = "2026-01-02T03:04:05Z";
        let namespace_digest =
            pre_sbom_digest(target, created, source_contract_sha256, &digest_files).unwrap();
        let mut files = Vec::new();
        for (index, (path, digest)) in inventory.iter().enumerate() {
            let license = LICENSE_PROFILE
                .iter()
                .find_map(|(license_path, spdx)| (*license_path == path).then_some(*spdx))
                .unwrap_or("NOASSERTION");
            files.push(SpdxFile {
                spdx_id: format!("SPDXRef-File-{index:04}"),
                checksums: vec![SpdxChecksum {
                    algorithm: "SHA256".to_owned(),
                    checksum_value: digest.clone(),
                }],
                copyright_text: "NOASSERTION".to_owned(),
                file_name: path.clone(),
                file_types: vec![
                    if path == executable || runtime_paths.contains(&path.as_str()) {
                        "BINARY"
                    } else {
                        "TEXT"
                    }
                    .to_owned(),
                ],
                license_concluded: license.to_owned(),
                license_info_in_files: vec![license.to_owned()],
            });
        }
        let mut packages = vec![
            spdx_package(
                "SPDXRef-Package-abseil",
                "abseil-cpp",
                ABSEIL_VERSION,
                ABSEIL_URL,
                ABSEIL_SHA256,
                "Apache-2.0",
            ),
            spdx_package(
                "SPDXRef-Package-bzip2",
                "bzip2",
                BZIP2_VERSION,
                BZIP2_URL,
                BZIP2_SHA256,
                "bzip2-1.0.6",
            ),
            spdx_package(
                "SPDXRef-Package-eutheto",
                "eutheto-ortools-worker",
                "0.1.0",
                "NOASSERTION",
                &namespace_digest,
                "Apache-2.0",
            ),
            spdx_package(
                "SPDXRef-Package-ortools",
                "OR-Tools",
                ORTOOLS_VERSION,
                ORTOOLS_URL,
                ORTOOLS_SHA256,
                "Apache-2.0",
            ),
            spdx_package(
                "SPDXRef-Package-protobuf",
                "protobuf",
                PROTOBUF_SOURCE_VERSION,
                PROTOBUF_URL,
                PROTOBUF_SHA256,
                "BSD-3-Clause",
            ),
            spdx_package(
                "SPDXRef-Package-re2",
                "RE2",
                RE2_VERSION,
                RE2_URL,
                RE2_SHA256,
                "BSD-3-Clause",
            ),
            spdx_package(
                "SPDXRef-Package-utf8-range",
                "utf8_range",
                PROTOBUF_SOURCE_VERSION,
                PROTOBUF_URL,
                PROTOBUF_SHA256,
                "MIT",
            ),
            spdx_package(
                "SPDXRef-Package-zlib",
                "zlib",
                ZLIB_VERSION,
                ZLIB_URL,
                ZLIB_SHA256,
                "Zlib",
            ),
        ];
        if target == "x86_64-pc-windows-msvc" {
            packages.push(spdx_unverified_package(
                "SPDXRef-Package-msvc-runtime",
                "Microsoft Visual C++ Runtime",
            ));
        }
        let mut relationships = vec![
            SpdxRelationship {
                spdx_element_id: "SPDXRef-DOCUMENT".to_owned(),
                relationship_type: "DESCRIBES".to_owned(),
                related_spdx_element: "SPDXRef-Package-eutheto".to_owned(),
            },
            SpdxRelationship {
                spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
                relationship_type: "STATIC_LINK".to_owned(),
                related_spdx_element: "SPDXRef-Package-ortools".to_owned(),
            },
        ];
        for package in ["abseil", "bzip2", "protobuf", "re2", "utf8-range", "zlib"] {
            relationships.push(SpdxRelationship {
                spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
                relationship_type: "DYNAMIC_LINK".to_owned(),
                related_spdx_element: format!("SPDXRef-Package-{package}"),
            });
        }
        if target == "x86_64-pc-windows-msvc" {
            relationships.push(SpdxRelationship {
                spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
                relationship_type: "DYNAMIC_LINK".to_owned(),
                related_spdx_element: "SPDXRef-Package-msvc-runtime".to_owned(),
            });
        }
        relationships.push(SpdxRelationship {
            spdx_element_id: "SPDXRef-Package-protobuf".to_owned(),
            relationship_type: "CONTAINS".to_owned(),
            related_spdx_element: "SPDXRef-Package-utf8-range".to_owned(),
        });
        relationships.extend(files.iter().map(|file| {
            if target == "x86_64-pc-windows-msvc" && is_msvc_runtime_file(&file.file_name) {
                SpdxRelationship {
                    spdx_element_id: file.spdx_id.clone(),
                    relationship_type: "GENERATED_FROM".to_owned(),
                    related_spdx_element: "SPDXRef-Package-msvc-runtime".to_owned(),
                }
            } else {
                SpdxRelationship {
                    spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
                    relationship_type: "CONTAINS".to_owned(),
                    related_spdx_element: file.spdx_id.clone(),
                }
            }
        }));
        let document = SpdxDocument {
            spdx_id: "SPDXRef-DOCUMENT".to_owned(),
            creation_info: SpdxCreationInfo {
                created: created.to_owned(),
                creators: vec!["Tool: eutheto-xtask-solver-finalizer".to_owned()],
            },
            data_license: "CC0-1.0".to_owned(),
            document_namespace: format!(
                "https://eutheto.dev/spdx/solver/{target}/{namespace_digest}"
            ),
            files,
            name: format!("eutheto-solver-{target}"),
            packages,
            relationships,
            spdx_version: "SPDX-2.3".to_owned(),
        };
        fs::write(
            artifact_root.join(SBOM_PATH),
            canonical_bytes(&document).unwrap(),
        )
        .unwrap();
    }

    #[allow(clippy::too_many_lines)]
    fn fixture(target: &str, architecture: &str) -> Fixture {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let artifact_root = root.join("artifact");
        fs::create_dir_all(artifact_root.join("bin")).unwrap();
        fs::create_dir_all(artifact_root.join("licenses")).unwrap();
        fs::create_dir_all(artifact_root.join("sbom")).unwrap();
        let executable = if target == "x86_64-pc-windows-msvc" {
            "bin/ortools-worker.exe"
        } else {
            "bin/ortools-worker"
        };
        fs::write(artifact_root.join(executable), b"worker-bytes").unwrap();
        fs::write(artifact_root.join(NOTICE_PATH), b"notice").unwrap();
        for (path, _) in LICENSE_PROFILE {
            fs::write(artifact_root.join(path), path.as_bytes()).unwrap();
        }
        let runtime_paths = if target == "x86_64-pc-windows-msvc" {
            vec!["bin/msvcp140.dll", "bin/vcruntime140.dll"]
        } else {
            Vec::new()
        };
        for path in &runtime_paths {
            fs::write(artifact_root.join(path), path.as_bytes()).unwrap();
        }

        let protocol_schema = root.join("solver-worker.proto");
        fs::write(
            &protocol_schema,
            b"syntax = \"proto3\";\npackage eutheto.worker.v1;\n",
        )
        .unwrap();
        let protocol_hash = sha256_file(&protocol_schema).unwrap();
        let source_contract = root.join("source-contract.json");
        write_json(
            &source_contract,
            &json!({
                "approval": {
                    "phase": 3,
                    "record": "docs/roadmap/assumptions.md@0123456789abcdef0123456789abcdef01234567",
                    "status": "approved"
                },
                "cmake": {"cache_entries": approved_cmake()},
                "ortools": {
                    "patch_path": "workers/ortools/patches/9.15-candidate-fixes.patch",
                    "patch_sha256": "1".repeat(64),
                    "sha256": ORTOOLS_SHA256,
                    "source_url": ORTOOLS_URL,
                    "version": ORTOOLS_VERSION
                },
                "protobuf": {
                    "cpp_runtime_version": PROTOBUF_RUNTIME_VERSION,
                    "protoc_version": PROTOBUF_SOURCE_VERSION,
                    "sha256": PROTOBUF_SHA256,
                    "source_url": PROTOBUF_URL,
                    "source_version": PROTOBUF_SOURCE_VERSION
                },
                "protocol": {"schema_sha256": protocol_hash, "wire_version": 1},
                "schema_version": 1,
                "worker": {"identity": "eutheto-ortools-worker", "version": "0.1.0"}
            }),
        );
        let protocol_policy = root.join("protocol-policy.json");
        write_json(
            &protocol_policy,
            &json!({
                "applied_parameters_hash": {},
                "compatibility": {},
                "field_limits": {},
                "frame_classes": {},
                "framing": {},
                "limits": {},
                "package": "eutheto.worker.v1",
                "protocol": "eutheto.solver-worker",
                "version": {"major": 1, "minor": 1}
            }),
        );
        let build_evidence = root.join("build-evidence.json");
        write_json(
            &build_evidence,
            &json!({
                "architecture": architecture,
                "cmake": {
                    "ortools": measured_cmake(target, CmakeScope::Ortools),
                    "worker": measured_cmake(target, CmakeScope::Worker)
                },
                "compiler": {"identity": "clang", "version": "1.0"},
                "linkage": "static-ortools",
                "runtime_library_paths": runtime_paths,
                "schema_version": 1,
                "target_triple": target,
                "worker": {
                    "adapter_version": "0.1.0",
                    "backend_id": "ortools-cp-sat",
                    "capabilities": CAPABILITIES,
                    "executable_path": executable
                }
            }),
        );
        let payload_evidence = root.join("payload-evidence.json");
        write_json(
            &payload_evidence,
            &json!({
                "licenses": LICENSE_PROFILE
                    .iter()
                    .map(|(path, spdx)| json!({"path": path, "spdx": spdx}))
                    .collect::<Vec<_>>(),
                "sbom_path": SBOM_PATH,
                "schema_version": 1
            }),
        );
        write_spdx_fixture(
            &artifact_root,
            target,
            executable,
            &runtime_paths,
            &sha256_file(&source_contract).unwrap(),
        );
        Fixture {
            _temporary: temporary,
            artifact_root,
            build_evidence,
            payload_evidence,
            protocol_policy,
            protocol_schema,
            source_contract,
        }
    }

    fn load_manifest(fixture: &Fixture) -> SolverManifest {
        let bytes = fs::read(fixture.manifest_path()).unwrap();
        parse_bounded_json(&bytes, "test manifest").unwrap()
    }

    fn portable_projection(manifest: &SolverManifest) -> Value {
        let mut value = serde_json::to_value(manifest).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("build");
        object.remove("licenses");
        object.remove("runtime_libraries");
        object.remove("sbom");
        object
            .get_mut("worker")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("executable");
        value
    }

    #[test]
    fn deterministic_double_assembly_has_identical_bytes_and_digest() {
        let first = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let second = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let first_digest = assemble(first.options()).unwrap();
        let second_digest = assemble(second.options()).unwrap();
        assert_eq!(first_digest, second_digest);
        assert_eq!(
            fs::read(first.manifest_path()).unwrap(),
            fs::read(second.manifest_path()).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(first.manifest_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
        assert!(assemble(first.options()).is_err());
    }

    #[test]
    fn portable_projection_is_equal_while_linux_and_windows_builds_differ() {
        let linux = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let windows = fixture("x86_64-pc-windows-msvc", "x86_64");
        assemble(linux.options()).unwrap();
        assemble(windows.options()).unwrap();
        let linux_manifest = load_manifest(&linux);
        let windows_manifest = load_manifest(&windows);
        assert_ne!(linux_manifest.build, windows_manifest.build);
        assert_ne!(
            linux_manifest.worker.executable.path,
            windows_manifest.worker.executable.path
        );
        assert_eq!(
            portable_projection(&linux_manifest),
            portable_projection(&windows_manifest)
        );
    }

    #[test]
    fn validation_requires_canonical_bytes_and_current_artifacts() {
        let test_fixture = fixture("aarch64-apple-darwin", "aarch64");
        let digest = assemble(test_fixture.options()).unwrap();
        let path = test_fixture.manifest_path();
        assert_eq!(validate(test_fixture.validation_options()).unwrap(), digest);

        let mut noncanonical = fs::read(&path).unwrap();
        noncanonical.push(b'\n');
        fs::write(&path, noncanonical).unwrap();
        assert!(validate(test_fixture.validation_options()).is_err());

        let fresh = fixture("aarch64-apple-darwin", "aarch64");

        assemble(fresh.options()).unwrap();
        fs::write(fresh.artifact_root.join("bin/ortools-worker"), b"mutated").unwrap();
        assert!(validate(fresh.validation_options()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validation_rejects_a_symlinked_installed_manifest() {
        use std::os::unix::fs::symlink;

        let fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        assemble(fixture.options()).unwrap();
        let manifest = fixture.manifest_path();
        fs::rename(&manifest, fixture.artifact_root.join("other-manifest.json")).unwrap();
        symlink("other-manifest.json", &manifest).unwrap();

        assert!(validate(fixture.validation_options()).is_err());
    }

    #[test]
    fn assembly_cross_checks_protocol_inputs_and_leaves_no_output_on_failure() {
        let schema_mismatch = fixture("x86_64-unknown-linux-gnu", "x86_64");
        fs::write(&schema_mismatch.protocol_schema, b"mutated schema").unwrap();
        assert!(assemble(schema_mismatch.options()).is_err());
        assert!(!schema_mismatch.manifest_path().exists());

        let missing_payload = fixture("x86_64-unknown-linux-gnu", "x86_64");
        fs::remove_file(missing_payload.artifact_root.join("licenses/NOTICE.txt")).unwrap();
        assert!(assemble(missing_payload.options()).is_err());
        assert!(!missing_payload.manifest_path().exists());

        let wire_mismatch = ProtocolPolicy {
            _applied_parameters_hash: json!({}),
            _compatibility: json!({}),
            _field_limits: json!({}),
            _frame_classes: json!({}),
            _framing: json!({}),
            _limits: json!({}),
            package: "eutheto.worker.v1".to_owned(),
            protocol: "eutheto.solver-worker".to_owned(),
            version: ProtocolVersion { major: 2, minor: 0 },
        };
        assert!(validate_protocol_policy(&wire_mismatch, 1).is_err());
    }

    #[test]
    fn validation_rejects_canonical_manifest_authority_mismatch() {
        let authority_fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        assemble(authority_fixture.options()).unwrap();
        let path = authority_fixture.manifest_path();
        let bytes = fs::read(&path).unwrap();
        let mut manifest: SolverManifest = parse_bounded_json(&bytes, "manifest").unwrap();
        manifest.approval.record.push_str("-different");
        fs::write(&path, canonical_bytes(&manifest).unwrap()).unwrap();

        assert!(validate(authority_fixture.validation_options()).is_err());

        let compiler_fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        assemble(compiler_fixture.options()).unwrap();
        let mut manifest = load_manifest(&compiler_fixture);
        manifest.build.compiler.identity = "AWS_SECRET_ACCESS_KEY".to_owned();
        fs::write(
            compiler_fixture.manifest_path(),
            canonical_bytes(&manifest).unwrap(),
        )
        .unwrap();
        assert!(validate(compiler_fixture.validation_options()).is_err());

        let cmake_fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        assemble(cmake_fixture.options()).unwrap();
        let mut manifest = load_manifest(&cmake_fixture);
        manifest.build.cmake.worker.insert(
            "BUILD_TOKEN".to_owned(),
            CacheValue::String("secret".to_owned()),
        );
        fs::write(
            cmake_fixture.manifest_path(),
            canonical_bytes(&manifest).unwrap(),
        )
        .unwrap();
        assert!(validate(cmake_fixture.validation_options()).is_err());
    }

    #[test]
    fn typed_inputs_reject_unknown_duplicate_and_float_fields() {
        let unknown = br#"{
          "architecture":"x86_64","cmake":{"ortools":{},"worker":{}},
          "compiler":{"identity":"x","version":"1"},"linkage":"static-ortools",
          "runtime_library_paths":[],"schema_version":1,
          "target_triple":"x86_64-unknown-linux-gnu",
          "unknown":true,
          "worker":{"adapter_version":"0.1.0","backend_id":"ortools-cp-sat","capabilities":[],"executable_path":"bin/worker"}
        }"#;
        assert!(parse_bounded_json::<BuildEvidence>(unknown, "build evidence").is_err());

        let duplicate =
            br#"{"licenses":[],"sbom_path":"sbom.json","schema_version":1,"schema_version":1}"#;
        assert!(parse_bounded_json::<PayloadEvidence>(duplicate, "payload evidence").is_err());

        let float = r#"{"A":1.5}"#;
        assert!(parse_test_cmake(float).is_err());
        let duplicate_cmake = r#"{"A":true,"A":false}"#;
        assert!(parse_test_cmake(duplicate_cmake).is_err());
        let invalid_key = parse_test_cmake(r#"{"9BAD":true}"#).unwrap();
        assert!(validate_cmake_map(&invalid_key, "CMake").is_err());
    }

    fn parse_test_cmake(text: &str) -> serde_json::Result<CmakeMap> {
        let mut deserializer = serde_json::Deserializer::from_str(text);
        deserialize_cmake_map(&mut deserializer)
    }

    #[test]
    fn source_authority_schema_constraints_are_enforced() {
        let fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let bytes = fs::read(&fixture.source_contract).unwrap();
        let mut source: SourceContract = parse_bounded_json(&bytes, "source").unwrap();
        source.approval.record = "docs/roadmap/assumptions.md@0123456789abcdef".to_owned();
        assert!(validate_source_contract(&source).is_err());

        let mut source: SourceContract = parse_bounded_json(&bytes, "source").unwrap();
        source.ortools.patch_path = "workers/ortools/patches/other.patch".to_owned();
        assert!(validate_source_contract(&source).is_err());
    }

    #[test]
    fn rejects_unsupported_versions_targets_and_portable_mismatches() {
        assert!(architecture_for_target("aarch64-unknown-linux-gnu").is_err());
        let test_fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let source_bytes = fs::read(&test_fixture.source_contract).unwrap();
        let source: SourceContract = parse_bounded_json(&source_bytes, "source").unwrap();
        let build_bytes = fs::read(&test_fixture.build_evidence).unwrap();
        let mut build: BuildEvidence = parse_bounded_json(&build_bytes, "build").unwrap();

        build.schema_version = 2;
        assert!(validate_build_evidence(&build, &source).is_err());
        build.schema_version = 1;
        build.architecture = "aarch64".to_owned();
        assert!(validate_build_evidence(&build, &source).is_err());
        build.architecture = "x86_64".to_owned();
        build.worker.capabilities.pop();
        assert!(validate_build_evidence(&build, &source).is_err());
        let fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let source: SourceContract =
            parse_bounded_json(&fs::read(&fixture.source_contract).unwrap(), "source").unwrap();
        let mut build: BuildEvidence =
            parse_bounded_json(&fs::read(&fixture.build_evidence).unwrap(), "build").unwrap();
        build.cmake.ortools.remove("USE_GLPK");
        assert!(validate_build_evidence(&build, &source).is_err());

        let mut build: BuildEvidence =
            parse_bounded_json(&fs::read(&fixture.build_evidence).unwrap(), "build").unwrap();
        build
            .cmake
            .worker
            .insert("USE_GLPK".to_owned(), CacheValue::Boolean(true));
        assert!(validate_build_evidence(&build, &source).is_err());
        let mut build: BuildEvidence =
            parse_bounded_json(&fs::read(&fixture.build_evidence).unwrap(), "build").unwrap();
        build.cmake.ortools.insert(
            "UNREVIEWED_SECRET".to_owned(),
            CacheValue::String("credential".to_owned()),
        );
        assert!(validate_build_evidence(&build, &source).is_err());

        let mut build: BuildEvidence =
            parse_bounded_json(&fs::read(&fixture.build_evidence).unwrap(), "build").unwrap();
        build.cmake.worker.remove("CMAKE_INSTALL_PREFIX");
        assert!(validate_build_evidence(&build, &source).is_err());

        let build: BuildEvidence =
            parse_bounded_json(&fs::read(&fixture.build_evidence).unwrap(), "build").unwrap();
        assert!(validate_build_evidence(&build, &source).is_ok());
    }

    #[test]
    fn rejects_unsafe_escaping_missing_and_duplicate_artifacts() {
        for path in [
            "/bin/worker",
            "../worker",
            "bin\\worker",
            "C:/worker",
            "bin//worker",
        ] {
            assert!(
                validate_relative_path(path, "test").is_err(),
                "accepted {path}"
            );
        }
        assert!(validate_cmake_string("/home/alice/build", "test", "PATH").is_err());
        assert!(validate_cmake_string("C:\\build", "test", "PATH").is_err());
        assert!(validate_cmake_string("@unknown@/bin", "test", "CMAKE_CXX_COMPILER").is_err());
        assert!(
            validate_cmake_string("@toolchain-root@/bin/clang", "test", "CMAKE_CXX_COMPILER",)
                .is_ok()
        );
        assert!(
            validate_cmake_string(
                "@toolchain-root@/bin:/Users/alice",
                "test",
                "CMAKE_CXX_COMPILER",
            )
            .is_err()
        );
        assert!(validate_cmake_string("/DEIGEN_MPL2_ONLY", "test", "CMAKE_CXX_FLAGS").is_ok());

        let fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        let root = canonical_artifact_root(&fixture.artifact_root).unwrap();
        let mut seen = HashSet::new();
        hash_artifact("bin/ortools-worker", &root, &mut seen).unwrap();
        assert!(hash_artifact("bin/ortools-worker", &root, &mut seen).is_err());
        assert!(hash_artifact("../outside", &root, &mut HashSet::new()).is_err());
        assert!(hash_artifact("missing", &root, &mut HashSet::new()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_all_payload_symlinks() {
        use std::os::unix::fs::symlink;

        let fixture = fixture("x86_64-unknown-linux-gnu", "x86_64");
        symlink(
            "ortools-worker",
            fixture.artifact_root.join("bin/relative-alias"),
        )
        .unwrap();
        let root = canonical_artifact_root(&fixture.artifact_root).unwrap();
        assert!(hash_artifact("bin/relative-alias", &root, &mut HashSet::new()).is_err());
        assert!(validate(fixture.validation_options()).is_err());
    }

    #[test]
    fn inventories_must_be_sorted_unique_and_bounded() {
        let unsorted = PayloadEvidence {
            licenses: vec![
                PayloadLicense {
                    path: "licenses/z".to_owned(),
                    spdx: "Apache-2.0".to_owned(),
                },
                PayloadLicense {
                    path: "licenses/a".to_owned(),
                    spdx: "Apache-2.0".to_owned(),
                },
            ],
            sbom_path: "sbom/file".to_owned(),
            schema_version: 1,
        };
        assert!(validate_payload_evidence(&unsorted).is_err());

        let duplicate = vec!["a".to_owned(), "a".to_owned()];
        assert!(validate_sorted_unique_strings(&duplicate, "inventory").is_err());
        let excessive = vec![String::new(); MAX_RUNTIME_LIBRARIES + 1];
        assert!(
            validate_sorted_unique_paths(&excessive, "runtime", MAX_RUNTIME_LIBRARIES).is_err()
        );
    }

    #[test]
    fn byte_string_count_and_depth_limits_are_enforced() {
        let temporary = TempDir::new().unwrap();
        let oversized = temporary.path().join("oversized.json");
        fs::write(&oversized, vec![b' '; MAX_INPUT_BYTES + 1]).unwrap();
        assert!(read_bounded(&oversized, "oversized").is_err());

        assert!(parse_bounded_json::<Value>(br"[[[[[[[[[0]]]]]]]]]", "test").is_err());
        let long = format!("\"{}\"", "x".repeat(MAX_STRING_BYTES + 1));
        assert!(parse_bounded_json::<Value>(long.as_bytes(), "test").is_err());
        assert!(parse_bounded_json::<Value>(b"1.0", "test").is_err());
        let many = Value::Array(vec![Value::Null; MAX_ARRAY_ENTRIES + 1]);
        assert!(validate_json_value(&many, 1, "test").is_err());
        let too_many_capabilities = vec!["capability".to_owned(); MAX_CAPABILITIES + 1];
        assert!(validate_capabilities(&too_many_capabilities).is_err());
    }
}
