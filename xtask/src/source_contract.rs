use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const OUTPUT_PATH: &str = "workers/ortools/source-contract.json";
const SCHEMA_PATH: &str = "workers/ortools/source-contract.schema.json";
const ORTOOLS_PATCH_PATH: &str = "workers/ortools/patches/9.15-candidate-fixes.patch";
const PROTOCOL_SCHEMA_PATH: &str = "protocol/solver-worker.proto";
const WORKER_VERSION_PATH: &str = "workers/ortools/VERSION";
const APPROVAL_RECORD: &str =
    "docs/roadmap/assumptions.md@d74be8614150ad8474dd78f59ffe0dc761fd742f";
const ORTOOLS_ARCHIVE_SHA256: &str =
    "6395a00a97ff30af878ee8d7fd5ad0ab1c7844f7219182c6d71acbee1b5f3026";
const ORTOOLS_PATCH_SHA256: &str =
    "3ab9c8c45d76aab2416195bc97266718986a395a37fcd6c8d6e6fa5322ecf6a6";
const PROTOBUF_ARCHIVE_SHA256: &str =
    "fda132cb0c86400381c0af1fe98bd0f775cb566cb247cdcc105e344e00acc30e";
const PROTOCOL_SCHEMA_SHA256: &str =
    "fc93888cd78db39bb35a430e6e904092b6bbc73b3276885fc75ecedad4c0381d";
pub(crate) const DEPENDENCY_SOURCES_OUTPUT_PATH: &str = "workers/ortools/dependency-sources.json";
const DEPENDENCY_SOURCES_SCHEMA_VERSION: u32 = 1;
const ORTOOLS_VERSION: &str = "9.15.6755";
const EXPECTED_DEPENDENCY_VERSIONS: [(&str, &str); 5] = [
    ("abseil", "20250814.1"),
    ("bzip2", "66c46b8c9436613fd81bc5d03f63a61933a4dcc3"),
    ("eigen", "3.4.0"),
    ("re2", "2025-08-12"),
    ("zlib", "1.3.1"),
];

#[derive(Serialize)]
struct SourceContract {
    approval: Approval,
    cmake: Cmake,
    ortools: Ortools,
    protobuf: Protobuf,
    protocol: Protocol,
    schema_version: u32,
    worker: Worker,
}

#[derive(Serialize)]
struct Approval {
    phase: u32,
    record: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
struct Cmake {
    cache_entries: BTreeMap<&'static str, CacheValue>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum CacheValue {
    Bool(bool),
    String(&'static str),
}

#[derive(Serialize)]
struct Ortools {
    patch_path: &'static str,
    patch_sha256: &'static str,
    sha256: &'static str,
    source_url: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct Protobuf {
    cpp_runtime_version: &'static str,
    protoc_version: &'static str,
    sha256: &'static str,
    source_url: &'static str,
    source_version: &'static str,
}

#[derive(Serialize)]
struct Protocol {
    schema_sha256: &'static str,
    wire_version: u32,
}

#[derive(Serialize)]
struct Worker {
    identity: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct DependencySources {
    dependencies: BTreeMap<&'static str, DependencySource>,
    ortools: DependencySourcesOrtools,
    schema_version: u32,
}

#[derive(Serialize)]
struct DependencySource {
    archive_name: &'static str,
    archive_root: &'static str,
    patch: &'static str,
    sha256: &'static str,
    source_url: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct DependencySourcesOrtools {
    sha256: &'static str,
    version: &'static str,
}

pub(crate) fn generated_file(repo_root: &Path) -> Result<(String, Vec<u8>)> {
    let contract = source_contract();
    validate_contract(repo_root, &contract)?;
    let mut contents = serde_json::to_string_pretty(&contract)
        .context("failed to serialize the approved OR-Tools source contract")?;
    contents.push('\n');
    Ok((OUTPUT_PATH.to_owned(), contents.into_bytes()))
}

pub(crate) fn dependency_sources_generated_file() -> Result<(String, Vec<u8>)> {
    let sources = dependency_sources();
    validate_dependency_sources(&sources)?;
    let mut contents = serde_json::to_string_pretty(&sources)
        .context("failed to serialize the OR-Tools dependency sources")?;
    contents.push('\n');
    Ok((
        DEPENDENCY_SOURCES_OUTPUT_PATH.to_owned(),
        contents.into_bytes(),
    ))
}

fn dependency_sources() -> DependencySources {
    DependencySources {
        dependencies: BTreeMap::from([
            (
                "abseil",
                DependencySource {
                    archive_name: "abseil-cpp-20250814.1.tar.gz",
                    archive_root: "abseil-cpp-20250814.1",
                    patch: "abseil-cpp-20250814.1.patch",
                    sha256: "1692f77d1739bacf3f94337188b78583cf09bab7e420d2dc6c5605a4f86785a1",
                    source_url: "https://github.com/abseil/abseil-cpp/archive/refs/tags/20250814.1.tar.gz",
                    version: "20250814.1",
                },
            ),
            (
                "bzip2",
                DependencySource {
                    archive_name: "bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz",
                    archive_root: "bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
                    patch: "bzip2.patch",
                    sha256: "3a4cff5f9d197e9e6c6138660afa6b1f9370df0bed135bd949243f6dfc83b3e1",
                    source_url: "https://gitlab.com/bzip2/bzip2/-/archive/66c46b8c9436613fd81bc5d03f63a61933a4dcc3/bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz",
                    version: "66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
                },
            ),
            (
                "eigen",
                DependencySource {
                    archive_name: "eigen-3.4.0.tar.gz",
                    archive_root: "eigen-3.4.0",
                    patch: "eigen3-3.4.0.patch",
                    sha256: "8586084f71f9bde545ee7fa6d00288b264a2b7ac3607b974e54d13e7162c1c72",
                    source_url: "https://gitlab.com/libeigen/eigen/-/archive/3.4.0/eigen-3.4.0.tar.gz",
                    version: "3.4.0",
                },
            ),
            (
                "re2",
                DependencySource {
                    archive_name: "re2-2025-08-12.tar.gz",
                    archive_root: "re2-2025-08-12",
                    patch: "re2-2025-08-12.patch",
                    sha256: "2f3bec634c3e51ea1faf0d441e0a8718b73ef758d7020175ed7e352df3f6ae12",
                    source_url: "https://github.com/google/re2/archive/refs/tags/2025-08-12.tar.gz",
                    version: "2025-08-12",
                },
            ),
            (
                "zlib",
                DependencySource {
                    archive_name: "zlib-v1.3.1.tar.gz",
                    archive_root: "zlib-1.3.1",
                    patch: "ZLIB-v1.3.1.patch",
                    sha256: "17e88863f3600672ab49182f217281b6fc4d3c762bde361935e436a95214d05c",
                    source_url: "https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz",
                    version: "1.3.1",
                },
            ),
        ]),
        ortools: DependencySourcesOrtools {
            sha256: ORTOOLS_ARCHIVE_SHA256,
            version: ORTOOLS_VERSION,
        },
        schema_version: DEPENDENCY_SOURCES_SCHEMA_VERSION,
    }
}

fn source_contract() -> SourceContract {
    SourceContract {
        approval: Approval {
            phase: 3,
            record: APPROVAL_RECORD,
            status: "approved",
        },
        cmake: Cmake {
            cache_entries: BTreeMap::from([
                ("BUILD_CXX", CacheValue::Bool(true)),
                ("BUILD_CXX_EXAMPLES", CacheValue::Bool(false)),
                ("BUILD_CXX_SAMPLES", CacheValue::Bool(false)),
                ("BUILD_DEPS", CacheValue::Bool(true)),
                ("BUILD_DOC", CacheValue::Bool(false)),
                ("BUILD_DOTNET", CacheValue::Bool(false)),
                ("BUILD_EXAMPLES", CacheValue::Bool(false)),
                ("BUILD_FLATZINC", CacheValue::Bool(false)),
                ("BUILD_JAVA", CacheValue::Bool(false)),
                ("BUILD_MATH_OPT", CacheValue::Bool(false)),
                ("BUILD_PYTHON", CacheValue::Bool(false)),
                ("BUILD_SAMPLES", CacheValue::Bool(false)),
                ("BUILD_SHARED_LIBS", CacheValue::Bool(false)),
                ("BUILD_TESTING", CacheValue::Bool(false)),
                ("CMAKE_BUILD_TYPE", CacheValue::String("Release")),
                ("INSTALL_BUILD_DEPS", CacheValue::Bool(true)),
                ("USE_BOP", CacheValue::Bool(true)),
                ("USE_COINOR", CacheValue::Bool(false)),
                ("USE_CPLEX", CacheValue::Bool(false)),
                ("USE_GLOP", CacheValue::Bool(true)),
                ("USE_GLPK", CacheValue::Bool(false)),
                ("USE_GUROBI", CacheValue::Bool(false)),
                ("USE_HIGHS", CacheValue::Bool(false)),
                ("USE_PDLP", CacheValue::Bool(false)),
                ("USE_SCIP", CacheValue::Bool(false)),
                ("USE_XPRESS", CacheValue::Bool(false)),
            ]),
        },
        ortools: Ortools {
            patch_path: ORTOOLS_PATCH_PATH,
            patch_sha256: ORTOOLS_PATCH_SHA256,
            sha256: ORTOOLS_ARCHIVE_SHA256,
            source_url: "https://github.com/google/or-tools/archive/refs/tags/v9.15.tar.gz",
            version: ORTOOLS_VERSION,
        },
        protobuf: Protobuf {
            cpp_runtime_version: "33.1.0",
            protoc_version: "33.1",
            sha256: PROTOBUF_ARCHIVE_SHA256,
            source_url: "https://github.com/protocolbuffers/protobuf/releases/download/v33.1/protobuf-33.1.tar.gz",
            source_version: "33.1",
        },
        protocol: Protocol {
            schema_sha256: PROTOCOL_SCHEMA_SHA256,
            wire_version: 1,
        },
        schema_version: 1,
        worker: Worker {
            identity: "eutheto-ortools-worker",
            version: "0.1.0",
        },
    }
}

fn validate_dependency_sources(sources: &DependencySources) -> Result<()> {
    ensure!(
        sources.schema_version == DEPENDENCY_SOURCES_SCHEMA_VERSION,
        "OR-Tools dependency sources schema version changed"
    );
    ensure!(
        sources.ortools.version == ORTOOLS_VERSION
            && sources.ortools.sha256 == ORTOOLS_ARCHIVE_SHA256,
        "OR-Tools dependency sources are associated with an unexpected OR-Tools source"
    );
    ensure!(
        sources
            .dependencies
            .keys()
            .copied()
            .eq(EXPECTED_DEPENDENCY_VERSIONS.iter().map(|(name, _)| *name)),
        "OR-Tools dependency sources must contain exactly abseil, bzip2, eigen, re2, and zlib"
    );
    ensure!(
        is_sha256(sources.ortools.sha256),
        "OR-Tools dependency source digest is malformed"
    );

    for (name, expected_version) in EXPECTED_DEPENDENCY_VERSIONS {
        let dependency = &sources.dependencies[name];
        ensure!(
            dependency.version == expected_version,
            "{name} dependency version changed"
        );
        ensure!(
            dependency.source_url.starts_with("https://"),
            "{name} dependency source URL must use HTTPS"
        );
        ensure!(
            is_sha256(dependency.sha256),
            "{name} dependency digest must be lowercase SHA-256"
        );
        for (field, value) in [
            ("archive_name", dependency.archive_name),
            ("archive_root", dependency.archive_root),
            ("patch", dependency.patch),
        ] {
            ensure!(
                is_safe_leaf(value),
                "{name} dependency {field} must be a safe path leaf"
            );
        }
    }
    Ok(())
}

fn is_safe_leaf(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn validate_contract(repo_root: &Path, contract: &SourceContract) -> Result<()> {
    let worker_version = fs::read_to_string(repo_root.join(WORKER_VERSION_PATH))
        .with_context(|| format!("failed to read {WORKER_VERSION_PATH}"))?;
    ensure!(
        worker_version == format!("{}\n", contract.worker.version),
        "{WORKER_VERSION_PATH} does not match the approved source-contract worker version"
    );
    validate_schema(repo_root, contract)?;
    validate_values(contract)?;
    verify_file_sha256(
        repo_root,
        contract.ortools.patch_path,
        contract.ortools.patch_sha256,
    )?;
    verify_file_sha256(
        repo_root,
        PROTOCOL_SCHEMA_PATH,
        contract.protocol.schema_sha256,
    )
}

fn validate_schema(repo_root: &Path, contract: &SourceContract) -> Result<()> {
    let schema_bytes = fs::read(repo_root.join(SCHEMA_PATH))
        .with_context(|| format!("failed to read {SCHEMA_PATH}"))?;
    let schema: Value = serde_json::from_slice(&schema_bytes)
        .with_context(|| format!("failed to parse {SCHEMA_PATH}"))?;
    ensure_schema_const(&schema, "/properties/schema_version/const", &1_u64.into())?;
    ensure_schema_const(
        &schema,
        "/properties/approval/properties/phase/const",
        &3_u64.into(),
    )?;
    ensure_schema_const(
        &schema,
        "/properties/protocol/properties/wire_version/const",
        &1_u64.into(),
    )?;
    ensure_schema_const(
        &schema,
        "/properties/worker/properties/identity/const",
        &Value::String(contract.worker.identity.to_owned()),
    )?;

    for (pointer, expected) in [
        (
            "/required",
            [
                "approval",
                "cmake",
                "ortools",
                "protobuf",
                "protocol",
                "schema_version",
                "worker",
            ]
            .as_slice(),
        ),
        (
            "/properties/approval/required",
            ["phase", "record", "status"].as_slice(),
        ),
        ("/properties/cmake/required", ["cache_entries"].as_slice()),
        (
            "/properties/ortools/required",
            [
                "patch_path",
                "patch_sha256",
                "sha256",
                "source_url",
                "version",
            ]
            .as_slice(),
        ),
        (
            "/properties/protobuf/required",
            [
                "cpp_runtime_version",
                "protoc_version",
                "sha256",
                "source_url",
                "source_version",
            ]
            .as_slice(),
        ),
        (
            "/properties/protocol/required",
            ["schema_sha256", "wire_version"].as_slice(),
        ),
        (
            "/properties/worker/required",
            ["identity", "version"].as_slice(),
        ),
    ] {
        ensure_required_fields(&schema, pointer, expected)?;
    }
    Ok(())
}

fn validate_values(contract: &SourceContract) -> Result<()> {
    ensure!(contract.approval.status == "approved");
    ensure!(contract.approval.record == APPROVAL_RECORD);
    ensure!(contract.cmake.cache_entries.len() == 26);
    for target_specific_entry in ["CMAKE_CXX_FLAGS", "CMAKE_C_COMPILER", "CMAKE_CXX_COMPILER"] {
        ensure!(
            !contract
                .cmake
                .cache_entries
                .contains_key(target_specific_entry)
        );
    }
    ensure!(
        contract.ortools.patch_path == ORTOOLS_PATCH_PATH,
        "approved OR-Tools patch path changed"
    );
    for hash in [
        contract.ortools.patch_sha256,
        contract.ortools.sha256,
        contract.protobuf.sha256,
        contract.protocol.schema_sha256,
    ] {
        ensure!(
            is_sha256(hash),
            "approved source-contract digest is malformed"
        );
    }
    ensure!(contract.ortools.source_url.starts_with("https://"));
    ensure!(contract.protobuf.source_url.starts_with("https://"));
    Ok(())
}

fn verify_file_sha256(repo_root: &Path, relative_path: &str, expected: &str) -> Result<()> {
    let bytes = fs::read(repo_root.join(relative_path))
        .with_context(|| format!("failed to read approved input {relative_path}"))?;
    let actual = crate::hex_bytes(&Sha256::digest(bytes));
    ensure!(
        actual == expected,
        "approved input {relative_path} has SHA-256 {actual}, expected {expected}"
    );
    Ok(())
}

fn ensure_schema_const(schema: &Value, pointer: &str, expected: &Value) -> Result<()> {
    ensure!(
        schema.pointer(pointer) == Some(expected),
        "{SCHEMA_PATH} no longer matches the source-contract generator at {pointer}"
    );
    Ok(())
}

fn ensure_required_fields(schema: &Value, pointer: &str, expected: &[&str]) -> Result<()> {
    let actual = schema
        .pointer(pointer)
        .and_then(Value::as_array)
        .with_context(|| format!("{SCHEMA_PATH} has no required-field array at {pointer}"))?
        .iter()
        .map(|item| {
            item.as_str()
                .with_context(|| format!("{SCHEMA_PATH} has a non-string field at {pointer}"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    ensure!(
        actual == expected,
        "{SCHEMA_PATH} required fields no longer match the source-contract generator at {pointer}"
    );
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use serde_json::{Value, json};

    use super::{
        DEPENDENCY_SOURCES_OUTPUT_PATH, dependency_sources, dependency_sources_generated_file,
        generated_file, validate_dependency_sources,
    };

    #[test]
    fn approved_contract_is_deterministic_and_canonical() -> Result<()> {
        let root = crate::repository_root()?;
        let (_, first) = generated_file(&root)?;
        let (_, second) = generated_file(&root)?;
        assert_eq!(first, second);
        assert!(first.ends_with(b"\n"));
        assert!(!first.ends_with(b"\n\n"));

        let contract: Value = serde_json::from_slice(&first)?;
        assert_eq!(
            contract["approval"]["record"],
            "docs/roadmap/assumptions.md@d74be8614150ad8474dd78f59ffe0dc761fd742f"
        );
        assert_eq!(contract["approval"]["status"], "approved");
        assert_eq!(
            contract["cmake"]["cache_entries"],
            json!({
                "BUILD_CXX": true,
                "BUILD_CXX_EXAMPLES": false,
                "BUILD_CXX_SAMPLES": false,
                "BUILD_DEPS": true,
                "BUILD_DOC": false,
                "BUILD_DOTNET": false,
                "BUILD_EXAMPLES": false,
                "BUILD_FLATZINC": false,
                "BUILD_JAVA": false,
                "BUILD_MATH_OPT": false,
                "BUILD_PYTHON": false,
                "BUILD_SAMPLES": false,
                "BUILD_SHARED_LIBS": false,
                "BUILD_TESTING": false,
                "CMAKE_BUILD_TYPE": "Release",
                "INSTALL_BUILD_DEPS": true,
                "USE_BOP": true,
                "USE_COINOR": false,
                "USE_CPLEX": false,
                "USE_GLOP": true,
                "USE_GLPK": false,
                "USE_GUROBI": false,
                "USE_HIGHS": false,
                "USE_PDLP": false,
                "USE_SCIP": false,
                "USE_XPRESS": false
            })
        );
        assert!(
            contract["cmake"]["cache_entries"]
                .get("CMAKE_CXX_FLAGS")
                .is_none()
        );
        assert!(
            contract["cmake"]["cache_entries"]
                .get("CMAKE_C_COMPILER")
                .is_none()
        );
        assert!(
            contract["cmake"]["cache_entries"]
                .get("CMAKE_CXX_COMPILER")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn approved_contract_records_exact_reviewed_sources() -> Result<()> {
        let root = crate::repository_root()?;
        let (_, bytes) = generated_file(&root)?;
        let contract: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(contract["ortools"]["version"], "9.15.6755");
        assert_eq!(
            contract["ortools"]["source_url"],
            "https://github.com/google/or-tools/archive/refs/tags/v9.15.tar.gz"
        );
        assert_eq!(
            contract["ortools"]["patch_path"],
            "workers/ortools/patches/9.15-candidate-fixes.patch"
        );
        assert_eq!(
            contract["ortools"]["patch_sha256"],
            "3ab9c8c45d76aab2416195bc97266718986a395a37fcd6c8d6e6fa5322ecf6a6"
        );
        assert_eq!(
            contract["ortools"]["sha256"],
            "6395a00a97ff30af878ee8d7fd5ad0ab1c7844f7219182c6d71acbee1b5f3026"
        );
        assert_eq!(contract["protobuf"]["source_version"], "33.1");
        assert_eq!(
            contract["protobuf"]["source_url"],
            "https://github.com/protocolbuffers/protobuf/releases/download/v33.1/protobuf-33.1.tar.gz"
        );
        assert_eq!(contract["protobuf"]["cpp_runtime_version"], "33.1.0");
        assert_eq!(contract["protobuf"]["protoc_version"], "33.1");
        assert_eq!(
            contract["protobuf"]["sha256"],
            "fda132cb0c86400381c0af1fe98bd0f775cb566cb247cdcc105e344e00acc30e"
        );
        assert_eq!(
            contract["protocol"]["schema_sha256"],
            "fc93888cd78db39bb35a430e6e904092b6bbc73b3276885fc75ecedad4c0381d"
        );
        assert_eq!(contract["protocol"]["wire_version"], 1);
        assert_eq!(contract["schema_version"], 1);
        assert_eq!(contract["worker"]["identity"], "eutheto-ortools-worker");
        assert_eq!(contract["worker"]["version"], "0.1.0");
        Ok(())
    }

    #[test]
    fn dependency_sources_are_deterministic_and_exact() -> Result<()> {
        let (path, first) = dependency_sources_generated_file()?;
        let (_, second) = dependency_sources_generated_file()?;
        assert_eq!(path, DEPENDENCY_SOURCES_OUTPUT_PATH);
        assert_eq!(first, second);
        assert!(first.ends_with(b"\n"));
        assert!(!first.ends_with(b"\n\n"));

        let sources: Value = serde_json::from_slice(&first)?;
        assert_eq!(
            sources,
            json!({
                "dependencies": {
                    "abseil": {
                        "archive_name": "abseil-cpp-20250814.1.tar.gz",
                        "archive_root": "abseil-cpp-20250814.1",
                        "patch": "abseil-cpp-20250814.1.patch",
                        "sha256": "1692f77d1739bacf3f94337188b78583cf09bab7e420d2dc6c5605a4f86785a1",
                        "source_url": "https://github.com/abseil/abseil-cpp/archive/refs/tags/20250814.1.tar.gz",
                        "version": "20250814.1"
                    },
                    "bzip2": {
                        "archive_name": "bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz",
                        "archive_root": "bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3",
                        "patch": "bzip2.patch",
                        "sha256": "3a4cff5f9d197e9e6c6138660afa6b1f9370df0bed135bd949243f6dfc83b3e1",
                        "source_url": "https://gitlab.com/bzip2/bzip2/-/archive/66c46b8c9436613fd81bc5d03f63a61933a4dcc3/bzip2-66c46b8c9436613fd81bc5d03f63a61933a4dcc3.tar.gz",
                        "version": "66c46b8c9436613fd81bc5d03f63a61933a4dcc3"
                    },
                    "eigen": {
                        "archive_name": "eigen-3.4.0.tar.gz",
                        "archive_root": "eigen-3.4.0",
                        "patch": "eigen3-3.4.0.patch",
                        "sha256": "8586084f71f9bde545ee7fa6d00288b264a2b7ac3607b974e54d13e7162c1c72",
                        "source_url": "https://gitlab.com/libeigen/eigen/-/archive/3.4.0/eigen-3.4.0.tar.gz",
                        "version": "3.4.0"
                    },
                    "re2": {
                        "archive_name": "re2-2025-08-12.tar.gz",
                        "archive_root": "re2-2025-08-12",
                        "patch": "re2-2025-08-12.patch",
                        "sha256": "2f3bec634c3e51ea1faf0d441e0a8718b73ef758d7020175ed7e352df3f6ae12",
                        "source_url": "https://github.com/google/re2/archive/refs/tags/2025-08-12.tar.gz",
                        "version": "2025-08-12"
                    },
                    "zlib": {
                        "archive_name": "zlib-v1.3.1.tar.gz",
                        "archive_root": "zlib-1.3.1",
                        "patch": "ZLIB-v1.3.1.patch",
                        "sha256": "17e88863f3600672ab49182f217281b6fc4d3c762bde361935e436a95214d05c",
                        "source_url": "https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz",
                        "version": "1.3.1"
                    }
                },
                "ortools": {
                    "sha256": "6395a00a97ff30af878ee8d7fd5ad0ab1c7844f7219182c6d71acbee1b5f3026",
                    "version": "9.15.6755"
                },
                "schema_version": 1
            })
        );
        Ok(())
    }

    #[test]
    fn dependency_sources_reject_schema_association_and_inventory_drift() {
        let mut sources = dependency_sources();
        sources.schema_version = 2;
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        sources.ortools.version = "9.14.6206";
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        sources.dependencies.remove("eigen");
        assert!(validate_dependency_sources(&sources).is_err());
    }

    #[test]
    fn dependency_sources_reject_untrusted_source_values() -> Result<()> {
        let mut sources = dependency_sources();
        sources
            .dependencies
            .get_mut("zlib")
            .context("missing zlib test dependency")?
            .source_url = "http://example.invalid/zlib.tar.gz";
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        let uppercase_sha256 = "17E88863F3600672AB49182F217281B6FC4D3C762BDE361935E436A95214D05C";
        sources
            .dependencies
            .get_mut("zlib")
            .context("missing zlib test dependency")?
            .sha256 = uppercase_sha256;
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        sources
            .dependencies
            .get_mut("zlib")
            .context("missing zlib test dependency")?
            .archive_name = "../zlib.tar.gz";
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        sources
            .dependencies
            .get_mut("zlib")
            .context("missing zlib test dependency")?
            .archive_root = "../zlib";
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        sources
            .dependencies
            .get_mut("zlib")
            .context("missing zlib test dependency")?
            .patch = "../zlib.patch";
        assert!(validate_dependency_sources(&sources).is_err());

        let mut sources = dependency_sources();
        sources
            .dependencies
            .get_mut("zlib")
            .context("missing zlib test dependency")?
            .version = "1.3.2";
        assert!(validate_dependency_sources(&sources).is_err());
        Ok(())
    }
}
