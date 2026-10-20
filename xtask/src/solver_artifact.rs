use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempPath};

use crate::solver_manifest::{
    AssembleOptions, LICENSE_PROFILE, NOTICE_PATH, SBOM_PATH, ValidateOptions,
};

const MAX_AUTHORITY_BYTES: u64 = 65_536;
const MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LICENSE_BYTES: u64 = 65_536;
const MAX_TREE_ENTRIES: usize = 256;
const LICENSE_SOURCE_SHA256: [(&str, &str); 8] = [
    (
        "licenses/abseil-Apache-2.0.txt",
        "c79a7fea0e3cac04cd43f20e7b648e5a0ff8fa5344e644b0ee09ca1162b62747",
    ),
    (
        "licenses/bzip2-bzip2-1.0.6.txt",
        "452871c08826ffb38b1a77960f23ef9c4b8b457887c3a6b691b7d42e882426e3",
    ),
    (
        "licenses/eutheto-Apache-2.0.txt",
        "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
    ),
    (
        "licenses/or-tools-Apache-2.0.txt",
        "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
    ),
    (
        "licenses/protobuf-BSD-3-Clause.txt",
        "6e5e117324afd944dcf67f36cf329843bc1a92229a8cd9bb573d7a83130fea7d",
    ),
    (
        "licenses/re2-BSD-3-Clause.txt",
        "6040cda75d90b1738292a631d89934c411ef7ffd543c4d6a1b7edfc8edf29449",
    ),
    (
        "licenses/utf8-range-MIT.txt",
        "02de69b64fc36d9e938f418e52723e42f0b2b226d58a9cb3c8dcbdf7059f5074",
    ),
    (
        "licenses/zlib-Zlib.txt",
        "845efc77857d485d91fb3e0b884aaa929368c717ae8186b66fe1ed2495753243",
    ),
];
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
const ORTOOLS_KEYS: [&str; 13] = [
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
const WORKER_KEYS: [&str; 14] = [
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
const DARWIN_KEYS: [&str; 3] = [
    "CMAKE_INSTALL_NAME_DIR",
    "CMAKE_INSTALL_RPATH",
    "CMAKE_MACOSX_RPATH",
];
const WINDOWS_KEYS: [&str; 1] = ["CMAKE_MSVC_RUNTIME_LIBRARY"];

#[derive(Debug, Clone, Copy)]
pub(crate) struct FinalizeOptions<'a> {
    pub authority_root: &'a Path,
    pub work_root: &'a Path,
    pub artifact_root: &'a Path,
    pub target_triple: &'a str,
    pub compiler_identity: &'a str,
    pub compiler_version: &'a str,
    pub source_date: &'a str,
}

pub(crate) fn finalize(options: FinalizeOptions<'_>) -> Result<String> {
    crate::solver_manifest::validate_source_date(options.source_date)?;
    validate_compiler_claim(options.compiler_identity, options.compiler_version)?;
    let authority_root = canonical_directory(options.authority_root, "authority root")?;
    let work_root = canonical_directory(options.work_root, "work root")?;
    let artifact_root = canonical_directory(options.artifact_root, "artifact root")?;
    ensure!(
        authority_root != work_root
            && authority_root != artifact_root
            && work_root != artifact_root,
        "authority, work, and artifact roots must be distinct"
    );
    let authorities = load_authorities(&authority_root)?;
    let (worker_path, runtime_paths) =
        validate_pristine_payload(&artifact_root, options.target_triple)?;
    detach_payload_files(
        &artifact_root,
        std::iter::once(worker_path.as_str()).chain(runtime_paths.iter().map(String::as_str)),
    )?;
    let detached_payload = validate_pristine_payload(&artifact_root, options.target_triple)?;
    ensure!(
        detached_payload == (worker_path.clone(), runtime_paths.clone()),
        "artifact payload changed while detaching file identities"
    );
    let build = derive_build_evidence(
        &authorities.source,
        &work_root,
        &authority_root,
        &artifact_root,
        options.target_triple,
        options.compiler_identity,
        options.compiler_version,
        worker_path,
        runtime_paths,
    )?;
    install_compliance_payload(&artifact_root, &work_root, &authorities, options, &build)?;
    let payload = PayloadEvidence {
        licenses: LICENSE_PROFILE
            .iter()
            .map(|(path, spdx)| PayloadLicense {
                path: (*path).to_owned(),
                spdx: (*spdx).to_owned(),
            })
            .collect(),
        sbom_path: SBOM_PATH.to_owned(),
        schema_version: 1,
    };
    let build_evidence = private_json(&work_root, "build evidence", &build)?;
    let payload_evidence = private_json(&work_root, "payload evidence", &payload)?;
    let digest = crate::solver_manifest::assemble(AssembleOptions {
        source_contract: &authorities.source_contract_path,
        protocol_schema: &authorities.protocol_schema_path,
        protocol_policy: &authorities.protocol_policy_path,
        build_evidence: build_evidence.as_ref(),
        payload_evidence: payload_evidence.as_ref(),
        artifact_root: &artifact_root,
    })?;
    let validated = crate::solver_manifest::validate(ValidateOptions {
        source_contract: &authorities.source_contract_path,
        protocol_schema: &authorities.protocol_schema_path,
        protocol_policy: &authorities.protocol_policy_path,
        artifact_root: &artifact_root,
    })?;
    ensure!(
        digest == validated,
        "assembled and validated manifest digests differ"
    );
    validate_final_tree(&artifact_root, &build)?;
    Ok(digest)
}

fn load_authorities(authority_root: &Path) -> Result<Authorities> {
    let source_contract_path = authority_root.join("workers/ortools/source-contract.json");
    let dependency_sources_path = authority_root.join("workers/ortools/dependency-sources.json");
    let protocol_schema_path = authority_root.join("protocol/solver-worker.proto");
    let protocol_policy_path = authority_root.join("protocol/version.json");
    crate::solver_manifest::validate_authority_inputs(
        &source_contract_path,
        &protocol_schema_path,
        &protocol_policy_path,
    )?;
    let source_bytes = read_bounded_regular(
        &source_contract_path,
        MAX_AUTHORITY_BYTES,
        "source contract",
    )?;
    require_canonical_json(&source_bytes, "source contract")?;
    let source: SourceContract =
        serde_json::from_slice(&source_bytes).context("invalid source contract")?;
    validate_source_profile(&source)?;
    let dependency_bytes = read_bounded_regular(
        &dependency_sources_path,
        MAX_AUTHORITY_BYTES,
        "dependency sources",
    )?;
    require_canonical_json(&dependency_bytes, "dependency sources")?;
    let dependencies: DependencySources =
        serde_json::from_slice(&dependency_bytes).context("invalid dependency sources")?;
    validate_dependency_profile(&dependencies, &source)?;
    Ok(Authorities {
        authority_root: authority_root.to_path_buf(),
        source_contract_path,
        protocol_schema_path,
        protocol_policy_path,
        source_bytes,
        source,
        dependencies,
    })
}

fn install_compliance_payload(
    artifact_root: &Path,
    work_root: &Path,
    authorities: &Authorities,
    options: FinalizeOptions<'_>,
    build: &BuildEvidence,
) -> Result<()> {
    let licenses = load_license_inputs(
        &authorities.authority_root,
        work_root,
        &authorities.dependencies,
    )?;
    let notice = notice_bytes(
        &authorities.source,
        &authorities.dependencies,
        options.source_date,
        build,
    );
    fs::create_dir(artifact_root.join("licenses"))
        .context("failed to create artifact licenses directory")?;
    fs::create_dir(artifact_root.join("sbom"))
        .context("failed to create artifact SBOM directory")?;
    for license in &licenses {
        persist_noclobber(
            &artifact_root.join(license.artifact_path),
            &license.bytes,
            "license",
        )?;
    }
    persist_noclobber(&artifact_root.join(NOTICE_PATH), &notice, "NOTICE")?;
    let inventory = pre_sbom_inventory(artifact_root, build)?;
    let source_sha256 = sha256_bytes(&authorities.source_bytes);
    let namespace_digest = crate::solver_manifest::pre_sbom_digest(
        options.target_triple,
        options.source_date,
        &source_sha256,
        &inventory,
    )?;
    let sbom = build_spdx_document(
        &authorities.source,
        &authorities.dependencies,
        options.target_triple,
        options.source_date,
        &namespace_digest,
        build,
        &inventory,
    );
    let sbom_bytes = canonical_json(&sbom)?;
    persist_noclobber(&artifact_root.join(SBOM_PATH), &sbom_bytes, "SPDX SBOM")
}

struct Authorities {
    authority_root: PathBuf,
    source_contract_path: PathBuf,
    protocol_schema_path: PathBuf,
    protocol_policy_path: PathBuf,
    source_bytes: Vec<u8>,
    source: SourceContract,
    dependencies: DependencySources,
}

fn canonical_directory(path: &Path, name: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {name} {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{name} must not be a symlink"
    );
    ensure!(metadata.is_dir(), "{name} must be a directory");
    fs::canonicalize(path).with_context(|| format!("failed to canonicalize {name}"))
}

fn validate_compiler_claim(identity: &str, version: &str) -> Result<()> {
    ensure!(
        matches!(identity, "clang" | "gcc" | "msvc"),
        "compiler identity must be clang, gcc, or msvc"
    );
    ensure!(
        !version.is_empty()
            && version.len() <= 64
            && version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'-' | b'+' | b'_')),
        "compiler version must be a normalized ASCII version"
    );
    Ok(())
}

fn validate_source_profile(source: &SourceContract) -> Result<()> {
    ensure!(
        source.schema_version == 1
            && source.approval.phase == 3
            && source.approval.status == "approved"
            && source.worker.identity == "eutheto-ortools-worker"
            && source.worker.version == "0.1.0",
        "unsupported solver source contract"
    );
    ensure!(
        source.ortools.version == "9.15.6755"
            && source.ortools.source_url
                == "https://github.com/google/or-tools/archive/refs/tags/v9.15.tar.gz"
            && source.ortools.sha256
                == "6395a00a97ff30af878ee8d7fd5ad0ab1c7844f7219182c6d71acbee1b5f3026",
        "unsupported OR-Tools authority"
    );
    ensure!(
        source.protobuf.source_version == "33.1"
            && source.protobuf.cpp_runtime_version == "33.1.0"
            && source.protobuf.source_url
                == "https://github.com/protocolbuffers/protobuf/releases/download/v33.1/protobuf-33.1.tar.gz"
            && source.protobuf.sha256
                == "fda132cb0c86400381c0af1fe98bd0f775cb566cb247cdcc105e344e00acc30e",
        "unsupported protobuf authority"
    );
    ensure!(
        source.cmake.cache_entries.get("USE_PDLP") == Some(&serde_json::Value::Bool(false)),
        "Eigen may be excluded only while USE_PDLP is false"
    );
    Ok(())
}

fn validate_dependency_profile(
    dependencies: &DependencySources,
    source: &SourceContract,
) -> Result<()> {
    ensure!(
        dependencies.schema_version == 1
            && dependencies.ortools.version == source.ortools.version
            && dependencies.ortools.sha256 == source.ortools.sha256,
        "dependency-source lock does not match OR-Tools authority"
    );
    let expected = [
        (
            "abseil",
            "20250814.1",
            "abseil-cpp-20250814.1.tar.gz",
            "abseil-cpp-20250814.1",
            "abseil-cpp-20250814.1.patch",
            "https://github.com/abseil/abseil-cpp/archive/refs/tags/20250814.1.tar.gz",
            "1692f77d1739bacf3f94337188b78583cf09bab7e420d2dc6c5605a4f86785a1",
        ),
        (
            "bzip2",
            "66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
            "bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz",
            "bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
            "bzip2.patch",
            "https://gitlab.com/bzip2/bzip2/-/archive/66c46b8c9436613fd81bc5d03f63a61933a4dcc3/bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz",
            "3a4cff5f9d197e9e6c6138660afa6b1f9370df0bed135bd949243f6dfc83b3e1",
        ),
        (
            "eigen",
            "3.4.0",
            "eigen-3.4.0.tar.gz",
            "eigen-3.4.0",
            "eigen3-3.4.0.patch",
            "https://gitlab.com/libeigen/eigen/-/archive/3.4.0/eigen-3.4.0.tar.gz",
            "8586084f71f9bde545ee7fa6d00288b264a2b7ac3607b974e54d13e7162c1c72",
        ),
        (
            "re2",
            "2025-08-12",
            "re2-2025-08-12.tar.gz",
            "re2-2025-08-12",
            "re2-2025-08-12.patch",
            "https://github.com/google/re2/archive/refs/tags/2025-08-12.tar.gz",
            "2f3bec634c3e51ea1faf0d441e0a8718b73ef758d7020175ed7e352df3f6ae12",
        ),
        (
            "zlib",
            "1.3.1",
            "zlib-v1.3.1.tar.gz",
            "zlib-1.3.1",
            "ZLIB-v1.3.1.patch",
            "https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz",
            "17e88863f3600672ab49182f217281b6fc4d3c762bde361935e436a95214d05c",
        ),
    ];
    ensure!(
        dependencies.dependencies.len() == expected.len(),
        "dependency-source lock must contain exactly five reviewed dependencies"
    );
    for (name, version, archive_name, archive_root, patch, url, sha256) in expected {
        let dependency = dependencies
            .dependencies
            .get(name)
            .with_context(|| format!("dependency-source lock is missing {name}"))?;
        ensure!(
            dependency.version == version
                && dependency.archive_name == archive_name
                && dependency.archive_root == archive_root
                && dependency.patch == patch
                && dependency.source_url == url
                && dependency.sha256 == sha256,
            "dependency-source lock has unsupported {name} authority"
        );
    }
    Ok(())
}

fn validate_leaf(value: &str, name: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value != "."
            && value != ".."
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'+' | b'-')),
        "{name} is not a safe bounded leaf"
    );
    Ok(())
}

fn validate_pristine_payload(root: &Path, target: &str) -> Result<(String, Vec<String>)> {
    let worker = match target {
        "x86_64-pc-windows-msvc" => "bin/ortools-worker.exe",
        "x86_64-unknown-linux-gnu" | "x86_64-apple-darwin" | "aarch64-apple-darwin" => {
            "bin/ortools-worker"
        }
        _ => bail!("unsupported solver target triple: {target}"),
    };
    let (files, directories) = collect_tree(root)?;
    ensure!(
        files.contains(worker),
        "pristine artifact is missing {worker}"
    );
    let mut runtime = Vec::new();
    for path in &files {
        if path == worker {
            continue;
        }
        ensure!(
            is_runtime_path(path, target),
            "unexpected pristine artifact file: {path}"
        );
        runtime.push(path.clone());
    }
    let mut expected_directories = BTreeSet::from(["bin".to_owned()]);
    if runtime.iter().any(|path| path.starts_with("lib/")) {
        expected_directories.insert("lib".to_owned());
    }
    ensure!(
        directories == expected_directories,
        "pristine artifact contains unexpected or empty directories"
    );
    Ok((worker.to_owned(), runtime))
}

fn is_runtime_path(path: &str, target: &str) -> bool {
    let Some(leaf) = path.rsplit('/').next() else {
        return false;
    };
    if leaf.is_empty()
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        return false;
    }
    match target {
        "x86_64-pc-windows-msvc" => path.starts_with("bin/") && has_file_extension(leaf, "dll"),
        "x86_64-unknown-linux-gnu" => {
            path.starts_with("lib/") && (has_file_extension(leaf, "so") || leaf.contains(".so."))
        }
        "x86_64-apple-darwin" | "aarch64-apple-darwin" => {
            path.starts_with("lib/") && has_file_extension(leaf, "dylib")
        }
        _ => false,
    }
}

fn has_file_extension(name: &str, expected: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case(expected))
}
fn detach_payload_files<'a>(
    root: &Path,
    relative_paths: impl Iterator<Item = &'a str>,
) -> Result<()> {
    for relative in relative_paths {
        let path = root.join(relative);
        let parent = path
            .parent()
            .context("artifact payload file has no parent directory")?;
        let mut source = open_read_no_follow(&path)?;
        let metadata = source
            .metadata()
            .with_context(|| format!("failed to inspect artifact payload {}", path.display()))?;
        ensure!(
            metadata.is_file() && !metadata_is_link_like(&metadata),
            "artifact payload is not a regular file: {}",
            path.display()
        );
        let mut detached = NamedTempFile::new_in(parent)
            .with_context(|| format!("failed to stage detached payload {}", path.display()))?;
        std::io::copy(&mut source, detached.as_file_mut())
            .with_context(|| format!("failed to detach artifact payload {}", path.display()))?;
        detached.as_file_mut().sync_all()?;
        detached.as_file().set_permissions(metadata.permissions())?;
        drop(source);
        detached
            .persist(&path)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to replace artifact payload {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            ensure!(
                fs::metadata(&path)?.nlink() == 1,
                "detached artifact payload still has multiple links: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn collect_tree(root: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0usize;
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .with_context(|| format!("failed to read {}", directory.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            count = count
                .checked_add(1)
                .context("artifact entry count overflow")?;
            ensure!(count <= MAX_TREE_ENTRIES, "artifact tree is unbounded");
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "artifact payload contains a symlink"
            );
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .context("artifact path is not UTF-8")?
                .replace('\\', "/");
            validate_relative(&relative)?;
            if metadata.is_dir() {
                directories.insert(relative);
                pending.push(path);
            } else {
                ensure!(
                    metadata.is_file(),
                    "artifact payload contains a special file"
                );
                files.insert(relative);
            }
        }
    }
    Ok((files, directories))
}

fn validate_relative(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 2048 && !value.contains('\\'),
        "invalid path"
    );
    ensure!(
        value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        "unsafe relative path"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn derive_build_evidence(
    source: &SourceContract,
    work_root: &Path,
    authority_root: &Path,
    artifact_root: &Path,
    target: &str,
    compiler_identity: &str,
    compiler_version: &str,
    worker_path: String,
    runtime_paths: Vec<String>,
) -> Result<BuildEvidence> {
    let darwin = target.ends_with("-apple-darwin");
    let mut ortools_extra = ORTOOLS_KEYS.to_vec();
    let mut worker_extra = WORKER_KEYS.to_vec();
    if darwin {
        ortools_extra.extend(DARWIN_KEYS);
        worker_extra.extend(DARWIN_KEYS);
    }
    if target == "x86_64-pc-windows-msvc" {
        ortools_extra.extend(WINDOWS_KEYS);
        worker_extra.extend(WINDOWS_KEYS);
    }
    let approved_keys: Vec<&str> = source
        .cmake
        .cache_entries
        .keys()
        .map(String::as_str)
        .collect();
    let mut ortools_selected = approved_keys.clone();
    ortools_selected.extend(ortools_extra.iter().copied());
    let mut worker_selected = approved_keys;
    worker_selected.extend(worker_extra.iter().copied());
    let ortools_raw = read_cache(
        &work_root.join("ortools-build/CMakeCache.txt"),
        &ortools_selected,
    )?;
    let worker_raw = read_cache(
        &work_root.join("worker-build/CMakeCache.txt"),
        &worker_selected,
    )?;
    let roots = NormalizationRoots::new(authority_root, work_root, artifact_root);
    let ortools = measured_map(
        &source.cmake.cache_entries,
        &ortools_raw,
        &ortools_extra,
        &roots,
        CacheScope::Ortools,
    )?;
    let worker = measured_map(
        &source.cmake.cache_entries,
        &worker_raw,
        &worker_extra,
        &roots,
        CacheScope::Worker,
    )?;
    cross_check_compiler(&ortools, &worker, target, compiler_identity)?;
    Ok(BuildEvidence {
        architecture: if target == "aarch64-apple-darwin" {
            "aarch64".to_owned()
        } else {
            "x86_64".to_owned()
        },
        cmake: MeasuredCmake { ortools, worker },
        compiler: Compiler {
            identity: compiler_identity.to_owned(),
            version: compiler_version.to_owned(),
        },
        linkage: "static-ortools".to_owned(),
        runtime_library_paths: runtime_paths,
        schema_version: 1,
        target_triple: target.to_owned(),
        worker: TestedWorker {
            adapter_version: "0.1.0".to_owned(),
            backend_id: "ortools-cp-sat".to_owned(),
            capabilities: CAPABILITIES
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            executable_path: worker_path,
        },
    })
}

#[derive(Clone, Copy)]
enum CacheScope {
    Ortools,
    Worker,
}

fn read_cache(path: &Path, selected: &[&str]) -> Result<BTreeMap<String, String>> {
    let bytes = read_bounded_regular(path, MAX_CACHE_BYTES, "CMake cache")?;
    let text = std::str::from_utf8(&bytes).context("CMake cache is not UTF-8")?;
    let selected: BTreeSet<&str> = selected.iter().copied().collect();
    let mut values = BTreeMap::new();
    for line in text.lines() {
        ensure!(
            line.len() <= 16_384,
            "CMake cache contains an overlong line"
        );
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((typed_key, value)) = line.split_once('=') else {
            continue;
        };
        let Some((key, _kind)) = typed_key.split_once(':') else {
            continue;
        };
        if selected.contains(key) {
            ensure!(
                values.insert(key.to_owned(), value.to_owned()).is_none(),
                "CMake cache contains duplicate reviewed key {key}"
            );
        }
    }
    let actual: BTreeSet<&str> = values.keys().map(String::as_str).collect();
    ensure!(
        actual == selected,
        "CMake cache omits a reviewed target-specific key"
    );
    Ok(values)
}

fn measured_map(
    approved: &BTreeMap<String, serde_json::Value>,
    raw: &BTreeMap<String, String>,
    extra_keys: &[&str],
    roots: &NormalizationRoots,
    scope: CacheScope,
) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut output = BTreeMap::new();
    for (key, expected) in approved {
        let actual = raw
            .get(key)
            .with_context(|| format!("CMake cache omits approved key {key}"))?;
        let normalized = cache_authority_value(actual, expected, key)?;
        ensure!(
            &normalized == expected,
            "CMake key {key} differs from source authority"
        );
        output.insert(key.clone(), normalized);
    }
    for key in extra_keys {
        let value = raw
            .get(*key)
            .with_context(|| format!("CMake cache omits {key}"))?;
        let measured = if is_bool_key(key) {
            serde_json::Value::Bool(parse_cmake_bool(value, key)?)
        } else if is_path_key(key) {
            let normalized = if matches!(*key, "CMAKE_C_COMPILER" | "CMAKE_CXX_COMPILER") {
                normalize_compiler(value)?
            } else {
                roots.normalize(value)?
            };
            serde_json::Value::String(normalized)
        } else {
            ensure!(
                !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control),
                "CMake key {key} has an invalid value"
            );
            serde_json::Value::String(value.clone())
        };
        output.insert((*key).to_owned(), measured);
    }
    ensure!(
        output.get("CMAKE_INSTALL_PREFIX")
            == Some(&serde_json::Value::String(match scope {
                CacheScope::Ortools => "@target-root@".to_owned(),
                CacheScope::Worker => "@artifact-root@".to_owned(),
            })),
        "CMAKE_INSTALL_PREFIX does not match its reviewed scope"
    );
    Ok(output)
}

fn cache_authority_value(
    actual: &str,
    expected: &serde_json::Value,
    key: &str,
) -> Result<serde_json::Value> {
    match expected {
        serde_json::Value::Bool(expected) => {
            let actual = parse_cmake_bool(actual, key)?;
            ensure!(
                actual == *expected,
                "CMake Boolean {key} differs from authority"
            );
            Ok(serde_json::Value::Bool(actual))
        }
        serde_json::Value::String(expected) => {
            ensure!(
                actual == expected,
                "CMake string {key} differs from authority"
            );
            Ok(serde_json::Value::String(actual.to_owned()))
        }
        _ => bail!("source authority has unsupported CMake value for {key}"),
    }
}

fn parse_cmake_bool(value: &str, key: &str) -> Result<bool> {
    match value.to_ascii_uppercase().as_str() {
        "ON" | "TRUE" | "1" => Ok(true),
        "OFF" | "FALSE" | "0" => Ok(false),
        _ => bail!("CMake key {key} is not a strict Boolean"),
    }
}

fn is_bool_key(key: &str) -> bool {
    matches!(
        key,
        "CMAKE_FIND_USE_CMAKE_ENVIRONMENT_PATH"
            | "CMAKE_FIND_USE_PACKAGE_REGISTRY"
            | "CMAKE_FIND_USE_PACKAGE_ROOT_PATH"
            | "CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY"
            | "CMAKE_MACOSX_RPATH"
            | "EUTHETO_ORTOOLS_BUILD_CANDIDATE_BENCHMARKS"
            | "EUTHETO_ORTOOLS_BUILD_TESTS"
            | "EUTHETO_ORTOOLS_DEVELOPMENT_BUILD"
            | "FETCHCONTENT_FULLY_DISCONNECTED"
    )
}

fn is_path_key(key: &str) -> bool {
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

struct NormalizationRoots {
    candidates: Vec<(String, &'static str)>,
    case_insensitive: bool,
}

impl NormalizationRoots {
    fn new(authority: &Path, work: &Path, artifact: &Path) -> Self {
        let mut candidates = vec![
            (
                portable_path(&work.join("sources")),
                "@source-root@/sources",
            ),
            (
                portable_path(&work.join("ortools-install")),
                "@target-root@",
            ),
            (portable_path(artifact), "@artifact-root@"),
            (portable_path(work), "@build-root@"),
            (portable_path(authority), "@source-root@"),
        ];
        candidates.sort_by_key(|(root, _)| Reverse(root.len()));
        Self {
            candidates,
            case_insensitive: cfg!(windows),
        }
    }

    fn normalize(&self, value: &str) -> Result<String> {
        ensure!(
            !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control),
            "CMake path is empty, unbounded, or contains control characters"
        );
        if matches!(value, "@rpath" | "@loader_path") {
            return Ok(value.to_owned());
        }
        ensure!(!value.contains(';'), "CMake path lists are not permitted");
        let portable = portable_text(value);
        for (root, token) in &self.candidates {
            if let Some(suffix) = strip_component_root(&portable, root, self.case_insensitive) {
                return Ok(if suffix.is_empty() {
                    (*token).to_owned()
                } else {
                    format!("{token}{suffix}")
                });
            }
        }
        bail!("CMake path does not belong to a reviewed normalization root")
    }
}

fn portable_path(path: &Path) -> String {
    portable_text(&path.to_string_lossy())
}

fn portable_text(value: &str) -> String {
    let mut value = value.replace('\\', "/");
    if let Some(stripped) = value.strip_prefix("//?/") {
        value = stripped.to_owned();
    }
    while value.ends_with('/') && value.len() > 1 {
        value.pop();
    }
    value
}

fn strip_component_root<'a>(value: &'a str, root: &str, insensitive: bool) -> Option<&'a str> {
    let matches = if insensitive {
        value
            .get(..root.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(root))
    } else {
        value.starts_with(root)
    };
    if !matches {
        return None;
    }
    let suffix = &value[root.len()..];
    (suffix.is_empty() || suffix.starts_with('/')).then_some(suffix)
}

fn normalize_compiler(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.len() <= 4096,
        "compiler path is invalid"
    );
    let portable = portable_text(value);
    let leaf = portable
        .rsplit('/')
        .next()
        .context("compiler path has no leaf")?;
    validate_leaf(leaf, "compiler executable")?;
    Ok(format!("@toolchain-root@/{leaf}"))
}

fn cross_check_compiler(
    ortools: &BTreeMap<String, serde_json::Value>,
    worker: &BTreeMap<String, serde_json::Value>,
    target: &str,
    identity: &str,
) -> Result<()> {
    let cxx = cmake_string(ortools, "CMAKE_CXX_COMPILER")?;
    let worker_cxx = cmake_string(worker, "CMAKE_CXX_COMPILER")?;
    ensure!(
        cxx.eq_ignore_ascii_case(worker_cxx),
        "OR-Tools and worker C++ compiler cache entries differ"
    );
    let leaf = cxx
        .rsplit('/')
        .next()
        .context("normalized compiler has no leaf")?;
    let lower = leaf.to_ascii_lowercase();
    let identity_matches = match identity {
        "msvc" => target == "x86_64-pc-windows-msvc" && lower == "cl.exe",
        "clang" => lower == "c++" || lower.starts_with("clang++") || lower.starts_with("clang-cl"),
        "gcc" => lower == "c++" || lower.starts_with("g++"),
        _ => false,
    };
    ensure!(
        identity_matches,
        "compiler identity does not match CMake compiler executable"
    );
    if target == "x86_64-pc-windows-msvc" {
        let c = cmake_string(ortools, "CMAKE_C_COMPILER")?;
        ensure!(c.ends_with("/cl.exe"), "Windows C compiler must be cl.exe");
    }
    Ok(())
}

fn cmake_string<'a>(map: &'a BTreeMap<String, serde_json::Value>, key: &str) -> Result<&'a str> {
    map.get(key)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("CMake key {key} is not a string"))
}

fn load_license_inputs(
    authority_root: &Path,
    work_root: &Path,
    dependencies: &DependencySources,
) -> Result<Vec<LicenseInput>> {
    let sources = work_root.join("sources");
    let specs = [
        (
            LICENSE_PROFILE[0],
            sources
                .join(&dependencies.dependencies["abseil"].archive_root)
                .join("LICENSE"),
        ),
        (
            LICENSE_PROFILE[1],
            sources
                .join(&dependencies.dependencies["bzip2"].archive_root)
                .join("COPYING"),
        ),
        (LICENSE_PROFILE[2], authority_root.join("LICENSE")),
        (LICENSE_PROFILE[3], sources.join("or-tools-9.15/LICENSE")),
        (LICENSE_PROFILE[4], sources.join("protobuf-33.1/LICENSE")),
        (
            LICENSE_PROFILE[5],
            sources
                .join(&dependencies.dependencies["re2"].archive_root)
                .join("LICENSE"),
        ),
        (
            LICENSE_PROFILE[6],
            sources.join("protobuf-33.1/third_party/utf8_range/LICENSE"),
        ),
        (
            LICENSE_PROFILE[7],
            sources
                .join(&dependencies.dependencies["zlib"].archive_root)
                .join("LICENSE"),
        ),
    ];
    let mut output = Vec::with_capacity(specs.len());
    for (index, ((artifact_path, spdx), source_path)) in specs.into_iter().enumerate() {
        let &(expected_path, expected_sha256) = LICENSE_SOURCE_SHA256
            .get(index)
            .context("license source digest profile is incomplete")?;
        ensure!(
            artifact_path == expected_path,
            "license source digest profile does not match the artifact license profile"
        );
        let bytes = read_bounded_regular(&source_path, MAX_LICENSE_BYTES, "license source")?;
        ensure!(
            sha256_bytes(&bytes) == expected_sha256,
            "license source digest does not match the reviewed value for {artifact_path}"
        );
        let text = std::str::from_utf8(&bytes).context("license source is not UTF-8 text")?;
        ensure!(
            !text.chars().any(|character| character == '\0'),
            "license source contains a NUL character"
        );
        ensure!(!bytes.is_empty(), "license source is empty");
        output.push(LicenseInput {
            artifact_path,
            spdx,
            bytes,
        });
    }
    ensure!(
        output.len() == LICENSE_SOURCE_SHA256.len(),
        "license source digest profile contains unexpected entries"
    );
    Ok(output)
}

fn notice_bytes(
    source: &SourceContract,
    dependencies: &DependencySources,
    source_date: &str,
    build: &BuildEvidence,
) -> Vec<u8> {
    let dependency = |name: &str| &dependencies.dependencies[name];
    let mut notice = format!(
        "eutheto\nCopyright 2026 eutheto contributors\n\nThis product is licensed under the Apache License, Version 2.0.\nSee eutheto-Apache-2.0.txt distributed beside this notice.\n\nEutheto bundled OR-Tools solver notices\n\nGenerated: {source_date}\nNo upstream NOTICE files apply to the reviewed open-source components. Their verbatim license texts are installed beside this file.\n\n- eutheto {} — Apache-2.0 — LICENSE\n- OR-Tools {} — Apache-2.0 — {} — SHA256 {}\n- protobuf {} — BSD-3-Clause — {} — SHA256 {}\n- utf8_range (bundled with protobuf {}) — MIT — {} — SHA256 {}\n- Abseil {} — Apache-2.0 — {} — SHA256 {}\n- RE2 {} — BSD-3-Clause — {} — SHA256 {}\n- zlib {} — Zlib — {} — SHA256 {}\n- bzip2 {} — bzip2-1.0.6 — {} — SHA256 {}\n",
        source.worker.version,
        source.ortools.version,
        source.ortools.source_url,
        source.ortools.sha256,
        source.protobuf.source_version,
        source.protobuf.source_url,
        source.protobuf.sha256,
        source.protobuf.source_version,
        source.protobuf.source_url,
        source.protobuf.sha256,
        dependency("abseil").version,
        dependency("abseil").source_url,
        dependency("abseil").sha256,
        dependency("re2").version,
        dependency("re2").source_url,
        dependency("re2").sha256,
        dependency("zlib").version,
        dependency("zlib").source_url,
        dependency("zlib").sha256,
        dependency("bzip2").version,
        dependency("bzip2").source_url,
        dependency("bzip2").sha256,
    );
    if build.target_triple == "x86_64-pc-windows-msvc" {
        notice.push_str(
            "- Microsoft Visual C++ Runtime — Copyright (c) Microsoft Corporation; app-local files supplied by the active licensed MSVC toolchain; redistribution is governed by the applicable Microsoft Visual Studio license terms — https://aka.ms/VCRedistLicense\n",
        );
    }
    notice.into_bytes()
}

fn pre_sbom_inventory(root: &Path, build: &BuildEvidence) -> Result<Vec<(String, String)>> {
    let mut paths = BTreeSet::new();
    paths.insert(build.worker.executable_path.clone());
    paths.extend(build.runtime_library_paths.iter().cloned());
    paths.extend(LICENSE_PROFILE.iter().map(|(path, _)| (*path).to_owned()));
    paths.insert(NOTICE_PATH.to_owned());
    ensure!(
        !paths.contains(SBOM_PATH) && !paths.contains("solver-manifest.json"),
        "cyclic files entered pre-SBOM inventory"
    );
    paths
        .into_iter()
        .map(|path| Ok((path.clone(), sha256_file(&root.join(path))?)))
        .collect()
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

fn spdx_packages(
    source: &SourceContract,
    dependencies: &DependencySources,
    namespace_digest: &str,
    build: &BuildEvidence,
) -> Vec<SpdxPackage> {
    let dependency = |name: &str| &dependencies.dependencies[name];
    let mut packages = vec![
        spdx_package(
            "SPDXRef-Package-abseil",
            "abseil-cpp",
            &dependency("abseil").version,
            &dependency("abseil").source_url,
            &dependency("abseil").sha256,
            "Apache-2.0",
        ),
        spdx_package(
            "SPDXRef-Package-bzip2",
            "bzip2",
            &dependency("bzip2").version,
            &dependency("bzip2").source_url,
            &dependency("bzip2").sha256,
            "bzip2-1.0.6",
        ),
        spdx_package(
            "SPDXRef-Package-eutheto",
            "eutheto-ortools-worker",
            &source.worker.version,
            "NOASSERTION",
            namespace_digest,
            "Apache-2.0",
        ),
        spdx_package(
            "SPDXRef-Package-ortools",
            "OR-Tools",
            &source.ortools.version,
            &source.ortools.source_url,
            &source.ortools.sha256,
            "Apache-2.0",
        ),
        spdx_package(
            "SPDXRef-Package-protobuf",
            "protobuf",
            &source.protobuf.source_version,
            &source.protobuf.source_url,
            &source.protobuf.sha256,
            "BSD-3-Clause",
        ),
        spdx_package(
            "SPDXRef-Package-re2",
            "RE2",
            &dependency("re2").version,
            &dependency("re2").source_url,
            &dependency("re2").sha256,
            "BSD-3-Clause",
        ),
        spdx_package(
            "SPDXRef-Package-utf8-range",
            "utf8_range",
            &source.protobuf.source_version,
            &source.protobuf.source_url,
            &source.protobuf.sha256,
            "MIT",
        ),
        spdx_package(
            "SPDXRef-Package-zlib",
            "zlib",
            &dependency("zlib").version,
            &dependency("zlib").source_url,
            &dependency("zlib").sha256,
            "Zlib",
        ),
    ];
    if build.target_triple == "x86_64-pc-windows-msvc" {
        packages.push(spdx_unverified_package(
            "SPDXRef-Package-msvc-runtime",
            "Microsoft Visual C++ Runtime",
        ));
    }
    packages
}

fn spdx_files(inventory: &[(String, String)], build: &BuildEvidence) -> Vec<SpdxFile> {
    inventory
        .iter()
        .enumerate()
        .map(|(index, (path, digest))| {
            let license = LICENSE_PROFILE
                .iter()
                .find_map(|(license_path, spdx)| (*license_path == path).then_some(*spdx))
                .unwrap_or("NOASSERTION");
            SpdxFile {
                spdx_id: format!("SPDXRef-File-{index:04}"),
                checksums: vec![SpdxChecksum {
                    algorithm: "SHA256".to_owned(),
                    checksum_value: digest.clone(),
                }],
                copyright_text: "NOASSERTION".to_owned(),
                file_name: path.clone(),
                file_types: vec![if path == &build.worker.executable_path
                    || build.runtime_library_paths.contains(path)
                {
                    "BINARY".to_owned()
                } else {
                    "TEXT".to_owned()
                }],
                license_concluded: license.to_owned(),
                license_info_in_files: vec![license.to_owned()],
            }
        })
        .collect()
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

fn spdx_relationships(files: &[SpdxFile], target: &str) -> Vec<SpdxRelationship> {
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
    for dependency in ["abseil", "bzip2", "protobuf", "re2", "utf8-range", "zlib"] {
        relationships.push(SpdxRelationship {
            spdx_element_id: "SPDXRef-Package-eutheto".to_owned(),
            relationship_type: "DYNAMIC_LINK".to_owned(),
            related_spdx_element: format!("SPDXRef-Package-{dependency}"),
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
    relationships
}

fn build_spdx_document(
    source: &SourceContract,
    dependencies: &DependencySources,
    target: &str,
    source_date: &str,
    namespace_digest: &str,
    build: &BuildEvidence,
    inventory: &[(String, String)],
) -> SpdxDocument {
    let files = spdx_files(inventory, build);
    SpdxDocument {
        spdx_id: "SPDXRef-DOCUMENT".to_owned(),
        creation_info: SpdxCreationInfo {
            created: source_date.to_owned(),
            creators: vec!["Tool: eutheto-xtask-solver-finalizer".to_owned()],
        },
        data_license: "CC0-1.0".to_owned(),
        document_namespace: format!("https://eutheto.dev/spdx/solver/{target}/{namespace_digest}"),
        relationships: spdx_relationships(&files, target),
        files,
        name: format!("eutheto-solver-{target}"),
        packages: spdx_packages(source, dependencies, namespace_digest, build),
        spdx_version: "SPDX-2.3".to_owned(),
    }
}

fn private_json<T: Serialize>(directory: &Path, name: &str, value: &T) -> Result<TempPath> {
    let bytes = canonical_json(value)?;
    ensure!(
        bytes.len() <= usize::try_from(MAX_AUTHORITY_BYTES)?,
        "{name} is too large"
    );
    let mut file = NamedTempFile::new_in(directory)
        .with_context(|| format!("failed to create private {name}"))?;
    file.as_file_mut().write_all(&bytes)?;
    file.as_file_mut().sync_all()?;
    Ok(file.into_temp_path())
}

fn persist_noclobber(path: &Path, bytes: &[u8], name: &str) -> Result<()> {
    ensure!(
        !path.exists(),
        "refusing to replace existing {name} {}",
        path.display()
    );
    let directory = path.parent().context("output has no parent directory")?;
    let mut temporary = NamedTempFile::new_in(directory)
        .with_context(|| format!("failed to create temporary {name}"))?;
    temporary.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to install {name} {}: {}",
            path.display(),
            error.error
        )
    })?;
    Ok(())
}

fn validate_final_tree(root: &Path, build: &BuildEvidence) -> Result<()> {
    let (files, directories) = collect_tree(root)?;
    let mut expected_files = BTreeSet::new();
    expected_files.insert(build.worker.executable_path.clone());
    expected_files.extend(build.runtime_library_paths.iter().cloned());
    expected_files.extend(LICENSE_PROFILE.iter().map(|(path, _)| (*path).to_owned()));
    expected_files.extend([
        NOTICE_PATH.to_owned(),
        SBOM_PATH.to_owned(),
        "solver-manifest.json".to_owned(),
    ]);
    ensure!(
        files == expected_files,
        "final artifact file inventory differs from contract"
    );
    let mut expected_directories = BTreeSet::new();
    for path in expected_files {
        let mut parent = Path::new(&path).parent();
        while let Some(value) = parent {
            if value.as_os_str().is_empty() {
                break;
            }
            expected_directories.insert(value.to_string_lossy().replace('\\', "/"));
            parent = value.parent();
        }
    }
    ensure!(
        directories == expected_directories,
        "final artifact directory inventory differs from contract"
    );
    Ok(())
}

#[cfg(unix)]
pub(crate) fn open_read_no_follow(path: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| format!("failed to open regular file {}", path.display()))?;
    Ok(File::from(descriptor))
}

#[cfg(windows)]
pub(crate) fn open_read_no_follow(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .with_context(|| format!("failed to open regular file {}", path.display()))
}

fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn read_bounded_regular(path: &Path, maximum: u64, name: &str) -> Result<Vec<u8>> {
    reject_symlink_components(path, name)?;
    let file = open_read_no_follow(path)?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect {name} {}", path.display()))?;
    ensure!(
        metadata.is_file() && !metadata_is_link_like(&metadata),
        "{name} is not a regular file"
    );
    ensure!(metadata.len() <= maximum, "{name} exceeds its byte bound");
    let capacity = usize::try_from(metadata.len())?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? <= maximum,
        "{name} grew beyond its byte bound"
    );
    Ok(bytes)
}

fn reject_symlink_components(path: &Path, name: &str) -> Result<()> {
    let mut ancestors: Vec<&Path> = path
        .ancestors()
        .filter(|value| !value.as_os_str().is_empty())
        .collect();
    ancestors.reverse();
    for component in ancestors {
        let metadata = fs::symlink_metadata(component).with_context(|| {
            format!(
                "failed to inspect {name} path component {}",
                component.display()
            )
        })?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "{name} path contains a symlink"
        );
    }
    Ok(())
}

fn require_canonical_json(bytes: &[u8], name: &str) -> Result<()> {
    ensure!(
        bytes.len() <= usize::try_from(MAX_AUTHORITY_BYTES)?,
        "{name} is too large"
    );
    ensure!(
        !bytes.starts_with(&[0xef, 0xbb, 0xbf]),
        "{name} has a byte-order mark"
    );
    let text = std::str::from_utf8(bytes).with_context(|| format!("{name} is not UTF-8"))?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if matches!(byte, b'{' | b'[') {
            depth = depth.checked_add(1).context("JSON nesting overflow")?;
            ensure!(depth <= 8, "{name} exceeds maximum JSON depth");
        } else if matches!(byte, b'}' | b']') {
            depth = depth.checked_sub(1).context("unbalanced JSON delimiters")?;
        }
    }
    ensure!(
        !in_string && depth == 0,
        "{name} has malformed JSON structure"
    );
    let value: serde_json::Value =
        serde_json::from_slice(bytes).with_context(|| format!("failed to parse {name}"))?;
    ensure!(
        bytes == canonical_json(&value)?,
        "{name} is not canonical JSON"
    );
    Ok(())
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context("failed to normalize canonical JSON")?;
    let mut bytes =
        serde_json::to_vec_pretty(&value).context("failed to serialize canonical JSON")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(open_read_no_follow(path)?);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

struct LicenseInput {
    artifact_path: &'static str,
    #[allow(dead_code)]
    spdx: &'static str,
    bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceContract {
    approval: SourceApproval,
    cmake: SourceCmake,
    ortools: SourceOrtools,
    protobuf: SourceProtobuf,
    #[serde(rename = "protocol")]
    _protocol: serde_json::Value,
    schema_version: u32,
    worker: SourceWorker,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceApproval {
    phase: u32,
    #[serde(rename = "record")]
    _record: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCmake {
    cache_entries: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceOrtools {
    #[serde(rename = "patch_path")]
    _patch_path: String,
    #[serde(rename = "patch_sha256")]
    _patch_sha256: String,
    sha256: String,
    source_url: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceProtobuf {
    cpp_runtime_version: String,
    #[serde(rename = "protoc_version")]
    _protoc_version: String,
    sha256: String,
    source_url: String,
    source_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceWorker {
    identity: String,
    version: String,
}

fn deserialize_dependency_map<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, DependencySource>, D::Error>
where
    D: Deserializer<'de>,
{
    struct DependencyMapVisitor;

    impl<'de> Visitor<'de> for DependencyMapVisitor {
        type Value = BTreeMap<String, DependencySource>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a dependency source object with unique keys")
        }

        fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = access.next_entry::<String, DependencySource>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(de::Error::custom(format!(
                        "duplicate dependency source {key}"
                    )));
                }
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(DependencyMapVisitor)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencySources {
    #[serde(deserialize_with = "deserialize_dependency_map")]
    dependencies: BTreeMap<String, DependencySource>,
    ortools: DependencyOrtools,
    schema_version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencySource {
    archive_name: String,
    archive_root: String,
    patch: String,
    sha256: String,
    source_url: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyOrtools {
    sha256: String,
    version: String,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct MeasuredCmake {
    ortools: BTreeMap<String, serde_json::Value>,
    worker: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct Compiler {
    identity: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct TestedWorker {
    adapter_version: String,
    backend_id: String,
    capabilities: Vec<String>,
    executable_path: String,
}

#[derive(Debug, Serialize)]
struct PayloadEvidence {
    licenses: Vec<PayloadLicense>,
    sbom_path: String,
    schema_version: u32,
}

#[derive(Debug, Serialize)]
struct PayloadLicense {
    path: String,
    spdx: String,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct SpdxCreationInfo {
    created: String,
    creators: Vec<String>,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct SpdxChecksum {
    algorithm: String,
    #[serde(rename = "checksumValue")]
    checksum_value: String,
}

#[derive(Debug, Serialize)]
struct SpdxPackage {
    #[serde(rename = "SPDXID")]
    spdx_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    #[serde(rename = "versionInfo", skip_serializing_if = "Option::is_none")]
    version_info: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpdxRelationship {
    #[serde(rename = "spdxElementId")]
    spdx_element_id: String,
    #[serde(rename = "relationshipType")]
    relationship_type: String,
    #[serde(rename = "relatedSpdxElement")]
    related_spdx_element: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn source_date_is_strict_utc_and_calendar_valid() {
        assert!(crate::solver_manifest::validate_source_date("2026-09-02T12:34:56Z").is_ok());
        for invalid in [
            "2026-09-02T12:34:56+00:00",
            "2026-9-02T12:34:56Z",
            "2025-02-29T00:00:00Z",
            "2026-09-02T24:00:00Z",
        ] {
            assert!(crate::solver_manifest::validate_source_date(invalid).is_err());
        }
    }

    #[test]
    fn pristine_inventory_rejects_unexpected_files() {
        let temporary = TempDir::new().unwrap();
        fs::create_dir(temporary.path().join("bin")).unwrap();
        fs::write(temporary.path().join("bin/ortools-worker"), b"worker").unwrap();
        assert!(validate_pristine_payload(temporary.path(), "x86_64-unknown-linux-gnu").is_ok());
        fs::write(temporary.path().join("README"), b"unexpected").unwrap();
        assert!(validate_pristine_payload(temporary.path(), "x86_64-unknown-linux-gnu").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn pristine_inventory_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new().unwrap();
        fs::create_dir(temporary.path().join("bin")).unwrap();
        fs::write(temporary.path().join("bin/ortools-worker"), b"worker").unwrap();
        symlink("ortools-worker", temporary.path().join("bin/alias")).unwrap();
        assert!(validate_pristine_payload(temporary.path(), "x86_64-unknown-linux-gnu").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn payload_detachment_breaks_external_hardlinks() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new().unwrap();
        fs::create_dir(temporary.path().join("bin")).unwrap();
        let worker = temporary.path().join("bin/ortools-worker");
        let outside = temporary.path().join("outside-worker");
        fs::write(&worker, b"worker").unwrap();
        fs::hard_link(&worker, &outside).unwrap();

        detach_payload_files(temporary.path(), ["bin/ortools-worker"].into_iter()).unwrap();
        fs::write(&outside, b"changed").unwrap();

        assert_eq!(fs::read(&worker).unwrap(), b"worker");
        assert_eq!(fs::metadata(&worker).unwrap().nlink(), 1);
    }

    #[test]
    fn pre_sbom_inventory_excludes_cyclic_outputs() {
        let temporary = TempDir::new().unwrap();
        fs::create_dir_all(temporary.path().join("bin")).unwrap();
        fs::create_dir_all(temporary.path().join("licenses")).unwrap();
        fs::write(temporary.path().join("bin/ortools-worker"), b"worker").unwrap();
        for (path, _) in LICENSE_PROFILE {
            fs::write(temporary.path().join(path), path.as_bytes()).unwrap();
        }
        fs::write(temporary.path().join(NOTICE_PATH), b"notice").unwrap();
        let build = BuildEvidence {
            architecture: "x86_64".to_owned(),
            cmake: MeasuredCmake {
                ortools: BTreeMap::new(),
                worker: BTreeMap::new(),
            },
            compiler: Compiler {
                identity: "gcc".to_owned(),
                version: "1".to_owned(),
            },
            linkage: "static-ortools".to_owned(),
            runtime_library_paths: vec![],
            schema_version: 1,
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            worker: TestedWorker {
                adapter_version: "0.1.0".to_owned(),
                backend_id: "ortools-cp-sat".to_owned(),
                capabilities: vec![],
                executable_path: "bin/ortools-worker".to_owned(),
            },
        };
        let inventory = pre_sbom_inventory(temporary.path(), &build).unwrap();
        assert!(
            !inventory
                .iter()
                .any(|(path, _)| path == SBOM_PATH || path == "solver-manifest.json")
        );
        assert!(inventory.iter().any(|(path, _)| path == NOTICE_PATH));
    }

    #[test]
    fn spdx_distinguishes_static_ortools_from_dynamic_dependencies() {
        let relationships = spdx_relationships(&[], "x86_64-unknown-linux-gnu");
        assert!(relationships.iter().any(|relationship| {
            relationship.relationship_type == "STATIC_LINK"
                && relationship.related_spdx_element == "SPDXRef-Package-ortools"
        }));
        assert_eq!(
            relationships
                .iter()
                .filter(|relationship| relationship.relationship_type == "DYNAMIC_LINK")
                .count(),
            6
        );
        assert!(!relationships.iter().any(|relationship| {
            relationship.relationship_type == "STATIC_LINK"
                && relationship.related_spdx_element != "SPDXRef-Package-ortools"
        }));
        let windows_relationships = spdx_relationships(&[], "x86_64-pc-windows-msvc");
        assert_eq!(
            windows_relationships
                .iter()
                .filter(|relationship| relationship.relationship_type == "DYNAMIC_LINK")
                .count(),
            7
        );
        assert!(windows_relationships.iter().any(|relationship| {
            relationship.relationship_type == "DYNAMIC_LINK"
                && relationship.related_spdx_element == "SPDXRef-Package-msvc-runtime"
        }));
        let runtime_file = SpdxFile {
            spdx_id: "SPDXRef-File-msvcp140".to_owned(),
            checksums: vec![],
            copyright_text: "NOASSERTION".to_owned(),
            file_name: "bin/msvcp140.dll".to_owned(),
            file_types: vec!["BINARY".to_owned()],
            license_concluded: "NOASSERTION".to_owned(),
            license_info_in_files: vec!["NOASSERTION".to_owned()],
        };
        let runtime_relationships = spdx_relationships(&[runtime_file], "x86_64-pc-windows-msvc");
        assert!(runtime_relationships.iter().any(|relationship| {
            relationship.spdx_element_id == "SPDXRef-File-msvcp140"
                && relationship.relationship_type == "GENERATED_FROM"
                && relationship.related_spdx_element == "SPDXRef-Package-msvc-runtime"
        }));
        assert!(!runtime_relationships.iter().any(|relationship| {
            relationship.spdx_element_id == "SPDXRef-Package-msvc-runtime"
                && relationship.relationship_type == "CONTAINS"
        }));
        assert!(is_msvc_runtime_file("bin/MSVCP140.dll"));
        assert!(is_msvc_runtime_file("bin/vcruntime140_1.dll"));
        assert!(!is_msvc_runtime_file("bin/libprotobuf.dll"));
    }

    #[test]
    fn license_profile_is_exact_and_sorted() {
        assert_eq!(LICENSE_PROFILE.len(), 8);
        assert!(LICENSE_PROFILE.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(LICENSE_PROFILE[6], ("licenses/utf8-range-MIT.txt", "MIT"));
        assert!(
            !LICENSE_PROFILE
                .iter()
                .any(|(path, _)| path.contains("eigen"))
        );
        assert_eq!(
            LICENSE_SOURCE_SHA256.map(|(path, _)| path),
            LICENSE_PROFILE.map(|(path, _)| path)
        );
        assert!(LICENSE_SOURCE_SHA256.iter().all(|(_, digest)| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }));
    }
}
