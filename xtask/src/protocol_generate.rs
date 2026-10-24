use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use prost::{Message, bytes::Bytes};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::hex_bytes;
use crate::protocol::wire::{
    Capability, Finished, HandshakeError, HandshakeErrorCode, HandshakeRequest, HandshakeResponse,
    HandshakeSuccess, Incumbent, ParentFrame, Progress, ProgressKind, ProjectedCandidate,
    ProjectedValue, ProjectionRequest, ResourceLimits, SolveParameters, SolveRequest, Started,
    TerminationReason, WorkerError, WorkerErrorCode, WorkerFrame, WorkerSolveStatus,
    handshake_response, parent_frame, worker_frame,
};

pub(crate) const REQUIRED_PROTOC_VERSION: &str = "libprotoc 33.1";
pub(crate) const RUST_BINDING_PATH: &str =
    "crates/eutheto-protocol/src/generated/eutheto.worker.v1.rs";
pub(crate) const ORTOOLS_RUST_BINDING_PATH: &str =
    "crates/eutheto-solver-ortools/src/generated/operations_research.sat.rs";
pub(crate) const CPP_HEADER_PATH: &str = "protocol/generated/cpp/solver-worker.pb.h";
pub(crate) const CPP_SOURCE_PATH: &str = "protocol/generated/cpp/solver-worker.pb.cc";
pub(crate) const CPP_POLICY_HEADER_PATH: &str = "protocol/generated/cpp/protocol-policy.h";
pub(crate) const DESCRIPTOR_PATH: &str = "protocol/generated/eutheto.worker.v1.descriptor.pb";
const OWNED_PROTOCOL_ROOTS: &[&str] = &[
    "crates/eutheto-solver-ortools/src/generated",
    "protocol/generated",
    "protocol/golden",
];
const ORTOOLS_PROTO_PATH: &str = "workers/ortools/proto/ortools/sat/cp_model.proto";
const ORTOOLS_PROTO_ROOT: &str = "workers/ortools/proto";
const ORTOOLS_PROTO_SHA256: &str =
    "c967180600fab5db4fc8b7477fef56c3c6d3c0714b1f0355697f96d147f77d96";

pub(crate) type GeneratedOutput = (String, Vec<u8>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppPolicy {
    applied_parameters_hash: CppAppliedParametersHash,
    #[serde(rename = "compatibility")]
    _compatibility: Value,
    field_limits: BTreeMap<String, CppFieldLimit>,
    frame_classes: BTreeMap<String, CppFrameClass>,
    framing: CppFraming,
    limits: CppLimits,
    #[serde(rename = "package")]
    _package: String,
    #[serde(rename = "protocol")]
    _protocol: String,
    version: CppProtocolVersion,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppAppliedParametersHash {
    algorithm: String,
    domain_separator: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppFieldLimit {
    max_bytes: Option<usize>,
    max_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppFrameClass {
    max_payload_bytes: usize,
    #[serde(rename = "routes")]
    _routes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppFraming {
    length_prefix_bytes: usize,
    length_prefix_order: String,
    min_payload_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppLimits {
    events_per_second: usize,
    events_per_session: usize,
    frames_per_session: usize,
    max_nesting_depth: usize,
    max_repeated_field_items: usize,
    max_stderr_bytes: usize,
    max_string_bytes: usize,
    max_worker_threads: usize,
    total_session_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CppProtocolVersion {
    major: u32,
    minor: u32,
}

pub(crate) fn generated_files(repo_root: &Path) -> Result<Vec<GeneratedOutput>> {
    let protoc = protoc_path()?;
    require_protoc_version(protoc.as_os_str())?;

    let first_root = TempDir::new().context("failed to create first protocol generation root")?;
    let second_root = TempDir::new().context("failed to create second protocol generation root")?;
    let first = generate_once(repo_root, first_root.path(), &protoc)?;
    let second = generate_once(repo_root, second_root.path(), &protoc)?;
    ensure!(
        first == second,
        "protocol generation is not deterministic across two isolated runs"
    );
    Ok(first)
}

pub(crate) fn remove_obsolete(repo_root: &Path, expected: &[GeneratedOutput]) -> Result<()> {
    let expected = expected
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>();
    for root in OWNED_PROTOCOL_ROOTS {
        let absolute = repo_root.join(root);
        if !absolute.exists() {
            continue;
        }
        for path in regular_files_recursive(&absolute)? {
            let relative = path
                .strip_prefix(repo_root)
                .context("owned generated path escaped repository root")?
                .to_string_lossy()
                .replace('\\', "/");
            if !expected.contains(relative.as_str()) {
                fs::remove_file(&path).with_context(|| {
                    format!(
                        "failed to remove obsolete generated file {}",
                        path.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

pub(crate) fn reject_unexpected(repo_root: &Path, expected: &[GeneratedOutput]) -> Result<()> {
    let expected = expected
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>();
    let mut unexpected = Vec::new();
    for root in OWNED_PROTOCOL_ROOTS {
        let absolute = repo_root.join(root);
        if !absolute.exists() {
            continue;
        }
        for path in regular_files_recursive(&absolute)? {
            let relative = path
                .strip_prefix(repo_root)
                .context("owned generated path escaped repository root")?
                .to_string_lossy()
                .replace('\\', "/");
            if !expected.contains(relative.as_str()) {
                unexpected.push(relative);
            }
        }
    }
    unexpected.sort();
    ensure!(
        unexpected.is_empty(),
        "generated protocol inventory contains unowned files: {}",
        unexpected.join(", ")
    );
    Ok(())
}

fn protoc_path() -> Result<PathBuf> {
    env::var_os("PROTOC")
        .map(PathBuf::from)
        .context("PROTOC must name the repository-pinned protoc executable")
}

pub(crate) fn require_protoc_version(protoc: &OsStr) -> Result<()> {
    let output = Command::new(protoc)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "failed to execute protoc at {}",
                Path::new(protoc).display()
            )
        })?;
    ensure!(
        output.status.success(),
        "protoc version probe failed with status {}",
        output.status
    );
    let actual = String::from_utf8(output.stdout).context("protoc version was not UTF-8")?;
    validate_protoc_version_output(&actual)
}

fn validate_protoc_version_output(actual: &str) -> Result<()> {
    ensure!(
        actual.trim() == REQUIRED_PROTOC_VERSION,
        "protocol generation requires exactly {REQUIRED_PROTOC_VERSION}; found `{}`",
        actual.trim()
    );
    Ok(())
}

fn generate_once(
    repo_root: &Path,
    output_root: &Path,
    protoc: &Path,
) -> Result<Vec<GeneratedOutput>> {
    let proto_root = repo_root.join("protocol");
    let proto = proto_root.join("solver-worker.proto");
    let rust_root = output_root.join("rust");
    let cpp_root = output_root.join("cpp");
    fs::create_dir_all(&rust_root).context("failed to create Rust protocol output root")?;
    fs::create_dir_all(&cpp_root).context("failed to create C++ protocol output root")?;
    let ortools_rust_binding = generate_ortools_rust_binding(repo_root, output_root, protoc)?;

    let mut prost = prost_build::Config::new();
    prost
        .out_dir(&rust_root)
        .bytes(["."])
        .protoc_executable(protoc)
        .compile_protos(
            std::slice::from_ref(&proto),
            std::slice::from_ref(&proto_root),
        )
        .context("failed to generate Rust worker protocol binding")?;

    run_protoc(
        protoc,
        &proto_root,
        [
            format!("--cpp_out={}", cpp_root.display()),
            "solver-worker.proto".to_owned(),
        ],
    )?;

    let descriptor = output_root.join("eutheto.worker.v1.descriptor.pb");
    run_protoc(
        protoc,
        &proto_root,
        [
            "--include_imports".to_owned(),
            "--include_source_info".to_owned(),
            format!("--descriptor_set_out={}", descriptor.display()),
            "solver-worker.proto".to_owned(),
        ],
    )?;

    let mut files = vec![
        (
            RUST_BINDING_PATH.to_owned(),
            with_source_header(
                b"//",
                "prost-build Rust binding",
                &fs::read(rust_root.join("eutheto.worker.v1.rs"))
                    .context("failed to read generated Rust protocol binding")?,
            ),
        ),
        (
            ORTOOLS_RUST_BINDING_PATH.to_owned(),
            with_ortools_source_header(
                b"//",
                "prost-build Rust binding from pinned OR-Tools 9.15 cp_model.proto",
                &ortools_rust_binding,
            ),
        ),
        (
            CPP_HEADER_PATH.to_owned(),
            with_source_header(
                b"//",
                "protoc C++ binding",
                &fs::read(cpp_root.join("solver-worker.pb.h"))
                    .context("failed to read generated C++ protocol header")?,
            ),
        ),
        (
            CPP_POLICY_HEADER_PATH.to_owned(),
            generate_cpp_policy_header(repo_root)?,
        ),
        (
            CPP_SOURCE_PATH.to_owned(),
            with_source_header(
                b"//",
                "protoc C++ binding",
                &fs::read(cpp_root.join("solver-worker.pb.cc"))
                    .context("failed to read generated C++ protocol source")?,
            ),
        ),
        (
            DESCRIPTOR_PATH.to_owned(),
            fs::read(&descriptor).context("failed to read generated protocol descriptor")?,
        ),
    ];
    files.extend(golden_fixtures()?);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}
fn generate_ortools_rust_binding(
    repo_root: &Path,
    output_root: &Path,
    protoc: &Path,
) -> Result<Vec<u8>> {
    let rust_root = output_root.join("ortools-rust");
    fs::create_dir_all(&rust_root)
        .context("failed to create Rust OR-Tools protocol output root")?;
    let proto = repo_root.join(ORTOOLS_PROTO_PATH);
    let proto_bytes = fs::read(&proto).context("failed to read pinned OR-Tools CP-SAT schema")?;
    ensure!(
        hex_bytes(&Sha256::digest(&proto_bytes)) == ORTOOLS_PROTO_SHA256,
        "pinned OR-Tools CP-SAT schema SHA-256 does not match its approved source contract"
    );
    prost_build::Config::new()
        .out_dir(&rust_root)
        .protoc_executable(protoc)
        .compile_protos(
            std::slice::from_ref(&proto),
            &[repo_root.join(ORTOOLS_PROTO_ROOT)],
        )
        .context("failed to generate Rust OR-Tools CP-SAT binding")?;
    fs::read(rust_root.join("operations_research.sat.rs"))
        .context("failed to read generated Rust OR-Tools CP-SAT binding")
}

#[allow(clippy::too_many_lines)]
fn generate_cpp_policy_header(repo_root: &Path) -> Result<Vec<u8>> {
    let source = fs::read(repo_root.join("protocol/version.json"))
        .context("failed to read protocol policy for C++ projection")?;
    let policy: CppPolicy = serde_json::from_slice(&source)
        .context("failed to decode protocol policy for C++ projection")?;
    ensure!(
        policy.applied_parameters_hash.algorithm == "sha256",
        "C++ policy projection requires the SHA-256 applied-parameter hash"
    );
    let domain_separator = policy.applied_parameters_hash.domain_separator.as_bytes();
    ensure!(
        domain_separator == b"eutheto.applied-solve-parameters.v1\0",
        "C++ policy projection requires the exact 36-byte applied-parameter domain separator"
    );
    ensure!(
        policy.framing.length_prefix_order == "big-endian",
        "C++ policy projection requires big-endian framing"
    );

    let mut output = String::from(
        "#pragma once\n\n#include <array>\n#include <cstddef>\n#include <cstdint>\n#include <string_view>\n\nnamespace eutheto::worker::v1::policy {\n\n",
    );
    writeln!(
        output,
        "inline constexpr std::uint32_t kProtocolMajor = {};",
        policy.version.major
    )?;
    writeln!(
        output,
        "inline constexpr std::uint32_t kProtocolMinor = {};",
        policy.version.minor
    )?;
    writeln!(
        output,
        "inline constexpr std::size_t kLengthPrefixBytes = {};",
        policy.framing.length_prefix_bytes
    )?;
    writeln!(
        output,
        "inline constexpr std::size_t kMinPayloadBytes = {};",
        policy.framing.min_payload_bytes
    )?;
    output.push_str("inline constexpr bool kLengthPrefixBigEndian = true;\n\n");

    for (name, class) in &policy.frame_classes {
        write_size_constant(
            &mut output,
            &format!("k{}MaxPayloadBytes", cpp_pascal_case(name)?),
            class.max_payload_bytes,
        )?;
    }
    output.push('\n');
    for (name, value) in [
        ("kEventsPerSecond", policy.limits.events_per_second),
        ("kEventsPerSession", policy.limits.events_per_session),
        ("kFramesPerSession", policy.limits.frames_per_session),
        ("kMaxNestingDepth", policy.limits.max_nesting_depth),
        (
            "kMaxRepeatedFieldItems",
            policy.limits.max_repeated_field_items,
        ),
        ("kMaxStderrBytes", policy.limits.max_stderr_bytes),
        ("kMaxStringBytes", policy.limits.max_string_bytes),
        ("kMaxWorkerThreads", policy.limits.max_worker_threads),
        ("kTotalSessionBytes", policy.limits.total_session_bytes),
    ] {
        write_size_constant(&mut output, name, value)?;
    }
    output.push('\n');
    for (field, limit) in &policy.field_limits {
        let field = field.strip_prefix("eutheto.worker.v1.").with_context(|| {
            format!("field limit `{field}` is outside the worker protocol package")
        })?;
        let (suffix, value) = match (limit.max_bytes, limit.max_count) {
            (Some(value), None) => ("MaxBytes", value),
            (None, Some(value)) => ("MaxCount", value),
            _ => bail!("field limit `{field}` must define exactly one cap"),
        };
        write_size_constant(
            &mut output,
            &format!("k{}{suffix}", cpp_pascal_case(field)?),
            value,
        )?;
    }
    output.push('\n');
    writeln!(
        output,
        "inline constexpr std::string_view kAppliedParametersHashAlgorithm = \"{}\";",
        policy.applied_parameters_hash.algorithm
    )?;
    writeln!(
        output,
        "inline constexpr std::array<std::uint8_t, {}>",
        domain_separator.len()
    )?;
    output.push_str("    kAppliedParametersHashDomainSeparator = {\n");
    for chunk in domain_separator.chunks(12) {
        output.push_str("        ");
        for byte in chunk {
            write!(output, "0x{byte:02x}, ")?;
        }
        output.push('\n');
    }
    output.push_str("    };\n\n}  // namespace eutheto::worker::v1::policy\n");
    Ok(with_source_header(
        b"//",
        "C++ protocol policy projection",
        output.as_bytes(),
    ))
}

fn write_size_constant(output: &mut String, name: &str, value: usize) -> std::fmt::Result {
    writeln!(output, "inline constexpr std::size_t {name} = {value};")
}

fn cpp_pascal_case(value: &str) -> Result<String> {
    let mut result = String::new();
    for word in value.split(['.', '_']) {
        ensure!(
            !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "policy name `{value}` cannot be projected to a C++ identifier"
        );
        let mut bytes = word.bytes();
        let first = bytes.next().context("policy identifier word is empty")?;
        result.push(char::from(first.to_ascii_uppercase()));
        result.extend(bytes.map(char::from));
    }
    Ok(result)
}

fn run_protoc<const N: usize>(protoc: &Path, current_dir: &Path, args: [String; N]) -> Result<()> {
    let output = Command::new(protoc)
        .args(args)
        .current_dir(current_dir)
        .output()
        .context("failed to invoke pinned protoc")?;
    if !output.status.success() {
        bail!(
            "pinned protoc failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn with_source_header(comment: &[u8], kind: &str, generated: &[u8]) -> Vec<u8> {
    let marker = String::from_utf8_lossy(comment);
    let header = format!(
        "{marker} SPDX-License-Identifier: Apache-2.0\n{marker} @generated by `cargo xtask generate` ({kind}); DO NOT EDIT.\n{marker} Authoritative inputs: protocol/solver-worker.proto, protocol/version.json; tool: {REQUIRED_PROTOC_VERSION}.\n\n"
    );
    let mut output = header.into_bytes();
    append_trimmed_lines(&mut output, generated);
    output
}

fn with_ortools_source_header(comment: &[u8], kind: &str, generated: &[u8]) -> Vec<u8> {
    let marker = String::from_utf8_lossy(comment);
    let header = format!(
        "{marker} SPDX-License-Identifier: Apache-2.0\n{marker} @generated by `cargo xtask generate` ({kind}); DO NOT EDIT.\n{marker} Authoritative input: {ORTOOLS_PROTO_PATH} at SHA-256 {ORTOOLS_PROTO_SHA256}; tool: {REQUIRED_PROTOC_VERSION}.\n\n"
    );
    let mut output = header.into_bytes();
    append_trimmed_lines(&mut output, generated);
    output
}

fn append_trimmed_lines(output: &mut Vec<u8>, generated: &[u8]) {
    for line in generated.split_inclusive(|byte| *byte == b'\n') {
        let content_end = line.len() - usize::from(line.ends_with(b"\n"));
        let trimmed_end = line[..content_end]
            .iter()
            .rposition(|byte| !matches!(byte, b' ' | b'\t'))
            .map_or(0, |index| index + 1);
        output.extend_from_slice(&line[..trimmed_end]);
        if content_end != line.len() {
            output.push(b'\n');
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum FixtureFrame {
    Parent(ParentFrame),
    Worker(WorkerFrame),
}

impl FixtureFrame {
    fn encode_payload(&self) -> Vec<u8> {
        match self {
            Self::Parent(frame) => frame.encode_to_vec(),
            Self::Worker(frame) => frame.encode_to_vec(),
        }
    }

    fn decode_payload(&self, payload: Bytes) -> Result<Self> {
        match self {
            Self::Parent(_) => ParentFrame::decode(payload)
                .map(Self::Parent)
                .context("fixture payload is not a ParentFrame"),
            Self::Worker(_) => WorkerFrame::decode(payload)
                .map(Self::Worker)
                .context("fixture payload is not a WorkerFrame"),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn fixture_frames() -> Vec<(&'static str, FixtureFrame)> {
    let fingerprint = Bytes::from_static(&[
        0x94, 0x17, 0x6f, 0x9b, 0x2c, 0xe2, 0xf9, 0xaf, 0x84, 0x32, 0x21, 0x5d, 0x87, 0x7f, 0x15,
        0x03, 0x13, 0x23, 0x85, 0xa4, 0xb1, 0x0f, 0xfd, 0x3f, 0x22, 0xd9, 0x85, 0x1d, 0xb2, 0x18,
        0xac, 0x27,
    ]);
    vec![
        (
            "handshake-request",
            FixtureFrame::Parent(ParentFrame {
                body: Some(parent_frame::Body::HandshakeRequest(HandshakeRequest {
                    protocol_major: 1,
                    protocol_minor: 1,
                    core_version: "0.1.0".to_owned(),
                    expected_backend_id: "ortools-cp-sat".to_owned(),
                    required_capabilities: vec![
                        Capability::CpSat as i32,
                        Capability::SolutionProjection as i32,
                    ],
                    expected_manifest_sha256: Bytes::from_static(&[0x11; 32]),
                })),
            }),
        ),
        (
            "handshake-response",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
                    outcome: Some(handshake_response::Outcome::Success(HandshakeSuccess {
                        protocol_major: 1,
                        protocol_minor: 1,
                        worker_identity: "eutheto-ortools-worker".to_owned(),
                        worker_version: "0.1.0".to_owned(),
                        backend_id: "ortools-cp-sat".to_owned(),
                        ortools_version: "9.15.6755".to_owned(),
                        adapter_version: "0.1.0".to_owned(),
                        manifest_sha256: Bytes::from_static(&[0x11; 32]),
                        capabilities: vec![
                            Capability::CpSat as i32,
                            Capability::IntermediateSolutions as i32,
                            Capability::Progress as i32,
                            Capability::SolutionProjection as i32,
                            Capability::ObjectiveBounds as i32,
                            Capability::SolutionStats as i32,
                            Capability::DeterministicTime as i32,
                        ],
                    })),
                })),
            }),
        ),
        (
            "handshake-error",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::HandshakeResponse(HandshakeResponse {
                    outcome: Some(handshake_response::Outcome::Error(HandshakeError {
                        code: HandshakeErrorCode::UnsupportedProtocolMajor as i32,
                        message: "unsupported protocol major 2".to_owned(),
                        supported_protocol_major: Some(1),
                        supported_protocol_minor: Some(1),
                    })),
                })),
            }),
        ),
        (
            "solve-request",
            FixtureFrame::Parent(ParentFrame {
                body: Some(parent_frame::Body::SolveRequest(SolveRequest {
                    request_id: "solve-0001".to_owned(),
                    cp_model_proto: Bytes::from_static(&[0x12, 0x04, 0x12, 0x02, 0x00, 0x00]),
                    parameters: Some(SolveParameters {
                        random_seed: Some(1),
                        stop_after_first_feasible: Some(false),
                        emit_intermediate_solutions: Some(true),
                        log_search_progress: Some(false),
                        deterministic_test_profile: Some(true),
                    }),
                    projections: vec![ProjectionRequest {
                        projection_id: 1,
                        cp_sat_variable_index: 0,
                    }],
                    resource_limits: Some(ResourceLimits {
                        wall_time_millis: 100,
                        memory_bytes: None,
                        worker_threads: 1,
                    }),
                    model_fingerprint: fingerprint.clone(),
                })),
            }),
        ),
        (
            "started",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::Started(Started {
                    request_id: "solve-0001".to_owned(),
                    model_fingerprint: fingerprint.clone(),
                })),
            }),
        ),
        (
            "progress",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::Progress(Progress {
                    request_id: "solve-0001".to_owned(),
                    kind: ProgressKind::Search as i32,
                    objective_values: vec![],
                    best_bound_values: vec![],
                    wall_time_seconds: Some(0.01),
                    deterministic_time: Some(0.001),
                })),
            }),
        ),
        (
            "incumbent",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::Incumbent(Incumbent {
                    request_id: "solve-0001".to_owned(),
                    candidate: Some(ProjectedCandidate {
                        values: vec![ProjectedValue {
                            projection_id: 1,
                            value: 0,
                        }],
                    }),
                    objective_values: vec![0.0],
                    best_bound_values: vec![0.0],
                    wall_time_seconds: Some(0.02),
                    deterministic_time: Some(0.002),
                })),
            }),
        ),
        (
            "finished",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::Finished(Finished {
                    request_id: "solve-0001".to_owned(),
                    raw_cp_sat_status: 4,
                    status: WorkerSolveStatus::Optimal as i32,
                    termination_reason: TerminationReason::Optimal as i32,
                    final_candidate: Some(ProjectedCandidate {
                        values: vec![ProjectedValue {
                            projection_id: 1,
                            value: 0,
                        }],
                    }),
                    objective_values: vec![0.0],
                    best_bound_values: vec![0.0],
                    wall_time_seconds: Some(0.03),
                    user_time_seconds: Some(0.02),
                    deterministic_time: Some(0.003),
                    conflicts: Some(0),
                    branches: Some(0),
                    binary_propagations: Some(0),
                    integer_propagations: Some(0),
                    sufficient_assumptions: vec![],
                    applied_parameters_sha256: Bytes::from_static(&[
                        0x58, 0xa0, 0x2d, 0x86, 0xa3, 0xb5, 0x7c, 0x8a, 0x13, 0x28, 0x65, 0xae,
                        0x6f, 0xb5, 0x26, 0x0c, 0x4c, 0xbe, 0xf5, 0xab, 0x24, 0x3b, 0xcb, 0x93,
                        0x7d, 0x16, 0x76, 0x4b, 0xfa, 0x9e, 0xc4, 0xf1,
                    ]),
                    model_fingerprint: fingerprint.clone(),
                })),
            }),
        ),
        (
            "worker-error",
            FixtureFrame::Worker(WorkerFrame {
                body: Some(worker_frame::Body::Error(WorkerError {
                    request_id: "solve-0001".to_owned(),
                    code: WorkerErrorCode::ResourceLimit as i32,
                    message: "worker resource limit reached".to_owned(),
                    retryable: true,
                })),
            }),
        ),
    ]
}

fn golden_fixtures() -> Result<Vec<GeneratedOutput>> {
    let fixtures = fixture_frames();
    let mut outputs = Vec::with_capacity(fixtures.len() * 2);
    for (name, fixture) in fixtures {
        let payload = fixture.encode_payload();
        let decoded = fixture.decode_payload(Bytes::copy_from_slice(&payload))?;
        ensure!(decoded == fixture, "fixture `{name}` does not round-trip");
        let semantic = render_fixture(&fixture)?;
        ensure_semantic_companion(&decoded, &semantic)?;

        let mut frame = Vec::with_capacity(payload.len() + 4);
        let payload_length = u32::try_from(payload.len()).context("fixture payload exceeds u32")?;
        frame.extend(payload_length.to_be_bytes());
        frame.extend(payload);
        outputs.push((
            format!("protocol/golden/{name}.json"),
            semantic_json_bytes(&semantic)?,
        ));
        outputs.push((
            format!("protocol/golden/{name}.frame.hex"),
            encode_hex(&frame).into_bytes(),
        ));
    }
    Ok(outputs)
}

fn ensure_semantic_companion(frame: &FixtureFrame, companion: &Value) -> Result<()> {
    ensure!(
        render_fixture(frame)? == *companion,
        "fixture semantic companion disagrees with its typed protobuf frame"
    );
    Ok(())
}

fn render_fixture(frame: &FixtureFrame) -> Result<Value> {
    match frame {
        FixtureFrame::Parent(frame) => render_parent_frame(frame),
        FixtureFrame::Worker(frame) => render_worker_frame(frame),
    }
}

fn render_parent_frame(frame: &ParentFrame) -> Result<Value> {
    match frame
        .body
        .as_ref()
        .context("fixture ParentFrame has no body")?
    {
        parent_frame::Body::HandshakeRequest(request) => Ok(json!({
            "handshakeRequest": {
                "coreVersion": request.core_version,
                "expectedBackendId": request.expected_backend_id,
                "expectedManifestSha256": hex_bytes(&request.expected_manifest_sha256),
                "protocolMajor": request.protocol_major,
                "protocolMinor": request.protocol_minor,
                "requiredCapabilities": capability_names(&request.required_capabilities)?
            }
        })),
        parent_frame::Body::SolveRequest(request) => {
            Ok(json!({ "solveRequest": render_solve_request(request) }))
        }
    }
}

fn render_worker_frame(frame: &WorkerFrame) -> Result<Value> {
    match frame
        .body
        .as_ref()
        .context("fixture WorkerFrame has no body")?
    {
        worker_frame::Body::HandshakeResponse(response) => {
            Ok(json!({ "handshakeResponse": render_handshake_response(response)? }))
        }
        worker_frame::Body::Started(started) => Ok(json!({
            "started": {
                "modelFingerprint": hex_bytes(&started.model_fingerprint),
                "requestId": started.request_id
            }
        })),
        worker_frame::Body::Progress(progress) => {
            Ok(json!({ "progress": render_progress(progress)? }))
        }
        worker_frame::Body::Incumbent(incumbent) => {
            Ok(json!({ "incumbent": render_incumbent(incumbent) }))
        }
        worker_frame::Body::Finished(finished) => {
            Ok(json!({ "finished": render_finished(finished)? }))
        }
        worker_frame::Body::Error(error) => Ok(json!({
            "error": {
                "code": worker_error_code_name(error.code)?,
                "message": error.message,
                "requestId": error.request_id,
                "retryable": error.retryable
            }
        })),
    }
}

fn render_handshake_response(response: &HandshakeResponse) -> Result<Value> {
    match response
        .outcome
        .as_ref()
        .context("fixture HandshakeResponse has no outcome")?
    {
        handshake_response::Outcome::Success(success) => Ok(json!({
            "success": {
                "adapterVersion": success.adapter_version,
                "backendId": success.backend_id,
                "capabilities": capability_names(&success.capabilities)?,
                "manifestSha256": hex_bytes(&success.manifest_sha256),
                "ortoolsVersion": success.ortools_version,
                "protocolMajor": success.protocol_major,
                "protocolMinor": success.protocol_minor,
                "workerIdentity": success.worker_identity,
                "workerVersion": success.worker_version
            }
        })),
        handshake_response::Outcome::Error(error) => {
            let mut rendered = serde_json::Map::new();
            rendered.insert(
                "code".to_owned(),
                json!(handshake_error_code_name(error.code)?),
            );
            rendered.insert("message".to_owned(), json!(error.message));
            if let Some(major) = error.supported_protocol_major {
                rendered.insert("supportedProtocolMajor".to_owned(), json!(major));
            }
            if let Some(minor) = error.supported_protocol_minor {
                rendered.insert("supportedProtocolMinor".to_owned(), json!(minor));
            }
            Ok(json!({ "error": rendered }))
        }
    }
}

fn render_solve_request(request: &SolveRequest) -> Value {
    let mut rendered = serde_json::Map::new();
    rendered.insert(
        "cpModelProto".to_owned(),
        json!(hex_bytes(&request.cp_model_proto)),
    );
    rendered.insert(
        "modelFingerprint".to_owned(),
        json!(hex_bytes(&request.model_fingerprint)),
    );
    if let Some(parameters) = &request.parameters {
        rendered.insert("parameters".to_owned(), render_solve_parameters(parameters));
    }
    rendered.insert(
        "projections".to_owned(),
        Value::Array(
            request
                .projections
                .iter()
                .map(|projection| {
                    json!({
                        "cpSatVariableIndex": projection.cp_sat_variable_index,
                        "projectionId": projection.projection_id
                    })
                })
                .collect(),
        ),
    );
    rendered.insert("requestId".to_owned(), json!(request.request_id));
    if let Some(limits) = &request.resource_limits {
        rendered.insert("resourceLimits".to_owned(), render_resource_limits(limits));
    }
    Value::Object(rendered)
}

fn render_solve_parameters(parameters: &SolveParameters) -> Value {
    let mut rendered = serde_json::Map::new();
    if let Some(value) = parameters.deterministic_test_profile {
        rendered.insert("deterministicTestProfile".to_owned(), json!(value));
    }
    if let Some(value) = parameters.emit_intermediate_solutions {
        rendered.insert("emitIntermediateSolutions".to_owned(), json!(value));
    }
    if let Some(value) = parameters.log_search_progress {
        rendered.insert("logSearchProgress".to_owned(), json!(value));
    }
    if let Some(value) = parameters.random_seed {
        rendered.insert("randomSeed".to_owned(), json!(value));
    }
    if let Some(value) = parameters.stop_after_first_feasible {
        rendered.insert("stopAfterFirstFeasible".to_owned(), json!(value));
    }
    Value::Object(rendered)
}

fn render_resource_limits(limits: &ResourceLimits) -> Value {
    let mut rendered = serde_json::Map::new();
    if let Some(value) = limits.memory_bytes {
        rendered.insert("memoryBytes".to_owned(), json!(value));
    }
    rendered.insert("wallTimeMillis".to_owned(), json!(limits.wall_time_millis));
    rendered.insert("workerThreads".to_owned(), json!(limits.worker_threads));
    Value::Object(rendered)
}

fn render_progress(progress: &Progress) -> Result<Value> {
    let mut rendered = serde_json::Map::new();
    if !progress.best_bound_values.is_empty() {
        rendered.insert(
            "bestBoundValues".to_owned(),
            json!(progress.best_bound_values),
        );
    }
    if let Some(value) = progress.deterministic_time {
        rendered.insert("deterministicTime".to_owned(), json!(value));
    }
    rendered.insert("kind".to_owned(), json!(progress_kind_name(progress.kind)?));
    if !progress.objective_values.is_empty() {
        rendered.insert(
            "objectiveValues".to_owned(),
            json!(progress.objective_values),
        );
    }
    rendered.insert("requestId".to_owned(), json!(progress.request_id));
    if let Some(value) = progress.wall_time_seconds {
        rendered.insert("wallTimeSeconds".to_owned(), json!(value));
    }
    Ok(Value::Object(rendered))
}

fn render_incumbent(incumbent: &Incumbent) -> Value {
    let mut rendered = serde_json::Map::new();
    if !incumbent.best_bound_values.is_empty() {
        rendered.insert(
            "bestBoundValues".to_owned(),
            json!(incumbent.best_bound_values),
        );
    }
    if let Some(candidate) = &incumbent.candidate {
        rendered.insert("candidate".to_owned(), render_candidate(candidate));
    }
    if let Some(value) = incumbent.deterministic_time {
        rendered.insert("deterministicTime".to_owned(), json!(value));
    }
    if !incumbent.objective_values.is_empty() {
        rendered.insert(
            "objectiveValues".to_owned(),
            json!(incumbent.objective_values),
        );
    }
    rendered.insert("requestId".to_owned(), json!(incumbent.request_id));
    if let Some(value) = incumbent.wall_time_seconds {
        rendered.insert("wallTimeSeconds".to_owned(), json!(value));
    }
    Value::Object(rendered)
}

fn render_finished(finished: &Finished) -> Result<Value> {
    let mut rendered = serde_json::Map::new();
    rendered.insert(
        "appliedParametersSha256".to_owned(),
        json!(hex_bytes(&finished.applied_parameters_sha256)),
    );
    if !finished.best_bound_values.is_empty() {
        rendered.insert(
            "bestBoundValues".to_owned(),
            json!(finished.best_bound_values),
        );
    }
    if let Some(value) = finished.binary_propagations {
        rendered.insert("binaryPropagations".to_owned(), json!(value));
    }
    if let Some(value) = finished.branches {
        rendered.insert("branches".to_owned(), json!(value));
    }
    if let Some(value) = finished.conflicts {
        rendered.insert("conflicts".to_owned(), json!(value));
    }
    if let Some(value) = finished.deterministic_time {
        rendered.insert("deterministicTime".to_owned(), json!(value));
    }
    if let Some(candidate) = &finished.final_candidate {
        rendered.insert("finalCandidate".to_owned(), render_candidate(candidate));
    }
    if let Some(value) = finished.integer_propagations {
        rendered.insert("integerPropagations".to_owned(), json!(value));
    }
    rendered.insert(
        "modelFingerprint".to_owned(),
        json!(hex_bytes(&finished.model_fingerprint)),
    );
    if !finished.objective_values.is_empty() {
        rendered.insert(
            "objectiveValues".to_owned(),
            json!(finished.objective_values),
        );
    }
    rendered.insert(
        "rawCpSatStatus".to_owned(),
        json!(finished.raw_cp_sat_status),
    );
    rendered.insert("requestId".to_owned(), json!(finished.request_id));
    rendered.insert(
        "status".to_owned(),
        json!(worker_solve_status_name(finished.status)?),
    );
    if !finished.sufficient_assumptions.is_empty() {
        rendered.insert(
            "sufficientAssumptions".to_owned(),
            json!(finished.sufficient_assumptions),
        );
    }
    rendered.insert(
        "terminationReason".to_owned(),
        json!(termination_reason_name(finished.termination_reason)?),
    );
    if let Some(value) = finished.user_time_seconds {
        rendered.insert("userTimeSeconds".to_owned(), json!(value));
    }
    if let Some(value) = finished.wall_time_seconds {
        rendered.insert("wallTimeSeconds".to_owned(), json!(value));
    }
    Ok(Value::Object(rendered))
}

fn render_candidate(candidate: &ProjectedCandidate) -> Value {
    json!({
        "values": candidate.values.iter().map(|value| {
            json!({ "projectionId": value.projection_id, "value": value.value })
        }).collect::<Vec<_>>()
    })
}

fn capability_names(values: &[i32]) -> Result<Vec<&'static str>> {
    values
        .iter()
        .map(|value| {
            Capability::try_from(*value)
                .map(|capability| capability.as_str_name())
                .map_err(|_| anyhow::anyhow!("fixture contains unknown capability {value}"))
        })
        .collect()
}

fn handshake_error_code_name(value: i32) -> Result<&'static str> {
    HandshakeErrorCode::try_from(value)
        .map(|code| code.as_str_name())
        .map_err(|_| anyhow::anyhow!("fixture contains unknown handshake error code {value}"))
}

fn progress_kind_name(value: i32) -> Result<&'static str> {
    ProgressKind::try_from(value)
        .map(|kind| kind.as_str_name())
        .map_err(|_| anyhow::anyhow!("fixture contains unknown progress kind {value}"))
}

fn worker_solve_status_name(value: i32) -> Result<&'static str> {
    WorkerSolveStatus::try_from(value)
        .map(|status| status.as_str_name())
        .map_err(|_| anyhow::anyhow!("fixture contains unknown worker solve status {value}"))
}

fn termination_reason_name(value: i32) -> Result<&'static str> {
    TerminationReason::try_from(value)
        .map(|reason| reason.as_str_name())
        .map_err(|_| anyhow::anyhow!("fixture contains unknown termination reason {value}"))
}

fn worker_error_code_name(value: i32) -> Result<&'static str> {
    WorkerErrorCode::try_from(value)
        .map(|code| code.as_str_name())
        .map_err(|_| anyhow::anyhow!("fixture contains unknown worker error code {value}"))
}

fn semantic_json_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .context("failed to serialize deterministic semantic fixture companion")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2 + 1);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output.push('\n');
    output
}

fn regular_files_recursive(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .with_context(|| {
                format!(
                    "failed to inspect generated directory {}",
                    directory.display()
                )
            })?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::fs;

    use prost::Message;
    use prost_types::FileDescriptorSet;

    use super::{REQUIRED_PROTOC_VERSION, validate_protoc_version_output};

    #[test]
    fn exact_protoc_version_mismatch_is_actionable() -> Result<()> {
        let Err(error) = validate_protoc_version_output("libprotoc 33.0\n") else {
            anyhow::bail!("mismatched protoc version was accepted");
        };
        assert!(error.to_string().contains(REQUIRED_PROTOC_VERSION));
        assert!(error.to_string().contains("libprotoc 33.0"));
        Ok(())
    }

    #[test]
    fn typed_fixtures_round_trip_and_detect_semantic_corruption() -> Result<()> {
        let fixtures = super::fixture_frames();
        assert_eq!(fixtures.len(), 9);
        for (name, fixture) in &fixtures {
            let payload = fixture.encode_payload();
            let decoded = fixture.decode_payload(prost::bytes::Bytes::copy_from_slice(&payload))?;
            assert_eq!(
                &decoded, fixture,
                "typed fixture `{name}` did not round-trip"
            );
            let rendered = super::render_fixture(fixture)?;
            assert_eq!(
                super::render_fixture(&decoded)?,
                rendered,
                "semantic fixture `{name}` changed after decode"
            );
            super::ensure_semantic_companion(&decoded, &rendered)?;
        }

        let Some((_, handshake)) = fixtures.first() else {
            anyhow::bail!("fixture inventory is empty");
        };
        let mut corrupted = super::render_fixture(handshake)?;
        corrupted["handshakeRequest"]["coreVersion"] = serde_json::json!("corrupted");
        assert!(super::ensure_semantic_companion(handshake, &corrupted).is_err());
        Ok(())
    }

    #[test]
    fn descriptor_and_fixture_generation_is_deterministic() -> Result<()> {
        let root = crate::repository_root()?;
        let policy: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("protocol/version.json"))?)?;
        assert_eq!(
            policy["limits"]["max_worker_threads"],
            serde_json::json!(10_000)
        );
        let generated = super::generated_files(&root)?;
        assert_eq!(generated.len(), 24);
        assert!(
            generated
                .iter()
                .any(|(path, _)| path == super::DESCRIPTOR_PATH)
        );
        assert_eq!(
            generated
                .iter()
                .filter(|(path, _)| path.starts_with("protocol/golden/"))
                .count(),
            18
        );
        let policy_header = generated
            .iter()
            .find(|(path, _)| path == super::CPP_POLICY_HEADER_PATH)
            .ok_or_else(|| anyhow::anyhow!("generated C++ policy header is missing"))?;
        let policy_source = std::str::from_utf8(&policy_header.1)?;
        assert!(policy_source.contains("inline constexpr std::uint32_t kProtocolMinor = 1;"));
        assert!(policy_source.contains("kAppliedParametersHashDomainSeparator"));
        assert!(policy_source.contains("inline constexpr std::size_t kMaxWorkerThreads = 10000;"));
        for (field, limit) in policy["field_limits"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("field limit policy is not an object"))?
        {
            let field = field
                .strip_prefix("eutheto.worker.v1.")
                .ok_or_else(|| anyhow::anyhow!("field limit is outside the protocol package"))?;
            let suffix = if limit.get("max_bytes").is_some() {
                "MaxBytes"
            } else {
                "MaxCount"
            };
            let constant = format!("k{}{suffix}", super::cpp_pascal_case(field)?);
            assert!(
                policy_source.contains(&constant),
                "generated C++ policy header is missing `{constant}`"
            );
        }

        let rust_binding = generated
            .iter()
            .find(|(path, _)| path == super::RUST_BINDING_PATH)
            .ok_or_else(|| anyhow::anyhow!("generated Rust binding is missing"))?;
        let rust_source = std::str::from_utf8(&rust_binding.1)?;
        assert!(rust_source.contains("pub cp_model_proto: ::prost::bytes::Bytes"));
        assert!(!rust_source.contains("pub cp_model_proto: ::prost::alloc::vec::Vec<u8>"));
        assert!(rust_source.contains("pub expected_manifest_sha256: ::prost::bytes::Bytes"));

        let handshake_request = generated
            .iter()
            .find(|(path, _)| path == "protocol/golden/handshake-request.json")
            .ok_or_else(|| anyhow::anyhow!("handshake request fixture is missing"))?;
        let request: serde_json::Value = serde_json::from_slice(&handshake_request.1)?;
        assert_eq!(
            request["handshakeRequest"]["requiredCapabilities"],
            serde_json::json!(["CAPABILITY_CP_SAT", "CAPABILITY_SOLUTION_PROJECTION"])
        );
        assert_eq!(
            request["handshakeRequest"]["protocolMinor"],
            serde_json::json!(1)
        );
        assert_eq!(
            request["handshakeRequest"]["expectedManifestSha256"],
            serde_json::json!("1111111111111111111111111111111111111111111111111111111111111111")
        );

        let handshake_response = generated
            .iter()
            .find(|(path, _)| path == "protocol/golden/handshake-response.json")
            .ok_or_else(|| anyhow::anyhow!("handshake response fixture is missing"))?;
        let response: serde_json::Value = serde_json::from_slice(&handshake_response.1)?;
        assert_eq!(
            response["handshakeResponse"]["success"]["capabilities"],
            serde_json::json!([
                "CAPABILITY_CP_SAT",
                "CAPABILITY_INTERMEDIATE_SOLUTIONS",
                "CAPABILITY_PROGRESS",
                "CAPABILITY_SOLUTION_PROJECTION",
                "CAPABILITY_OBJECTIVE_BOUNDS",
                "CAPABILITY_SOLUTION_STATS",
                "CAPABILITY_DETERMINISTIC_TIME"
            ])
        );
        assert_eq!(
            response["handshakeResponse"]["success"]["protocolMinor"],
            serde_json::json!(1)
        );
        assert_eq!(
            response["handshakeResponse"]["success"]["manifestSha256"],
            request["handshakeRequest"]["expectedManifestSha256"]
        );
        Ok(())
    }

    #[test]
    fn checked_in_descriptor_has_expected_package_and_top_level_inventory() -> Result<()> {
        let root = crate::repository_root()?;
        let bytes = fs::read(root.join(crate::protocol_generate::DESCRIPTOR_PATH))?;
        let descriptor = FileDescriptorSet::decode(bytes.as_slice())?;
        let file = descriptor
            .file
            .first()
            .ok_or_else(|| anyhow::anyhow!("protocol descriptor has no file"))?;
        assert_eq!(file.package.as_deref(), Some("eutheto.worker.v1"));
        let messages = file
            .message_type
            .iter()
            .filter_map(|message| message.name.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            [
                "ParentFrame",
                "WorkerFrame",
                "HandshakeRequest",
                "HandshakeResponse",
                "HandshakeSuccess",
                "HandshakeError",
                "SolveRequest",
                "SolveParameters",
                "ProjectionRequest",
                "ResourceLimits",
                "Started",
                "Progress",
                "Incumbent",
                "ProjectedCandidate",
                "ProjectedValue",
                "Finished",
                "WorkerError"
            ]
        );
        Ok(())
    }
}
