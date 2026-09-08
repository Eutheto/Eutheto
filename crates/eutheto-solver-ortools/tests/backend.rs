#![forbid(unsafe_code)]
#![cfg(any(
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"),
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
))]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use eutheto_domain_ir::{DomainAssignmentId, DomainEntityId, DomainEntityKindId, DomainEntityRef};
use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, Capability, CompilerId, ObjectivePlan,
    PLANNING_IR_SCHEMA_VERSION, PROJECTION_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningMetadata,
    PlanningProblem, ProjectionExpression, ProjectionId, ProvenanceId, ProvenanceRecord,
    ProvenanceSourceKind, SolutionProjection, Variable, summarize,
};
use eutheto_solver_api::{
    BackendOutputSink, BackendTerminationReason, BoundedBackendOutput, ProgressSink,
    SolveProgressEvent, SolveRequest, SolverApiLimits, validate_outcome,
};
use eutheto_solver_ortools::{
    BundledWorkerArtifactError, ExecutableIdentityError, ORTOOLS_ADAPTER_VERSION,
    ORTOOLS_BACKEND_ID, ORTOOLS_VERSION, VerifiedWorkerArtifact, registry_with_ortools,
};
use eutheto_types::{
    BackendSelection, CancellationToken, DurationMillis, ExplanationMode, PackId,
    ParentSolveBudget, PreservationPolicy, ReproducibilityMode, ResourceLimits, ScenarioId,
    SolveMode, SolveOptions, SystemMonotonicClock, WorkerThreadPolicy,
};
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

struct Progress(Vec<SolveProgressEvent>);

impl ProgressSink for Progress {
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), eutheto_solver_api::OutputError> {
        self.0.push(event);
        Ok(())
    }
}

async fn verified_artifact()
-> Result<(VerifiedWorkerArtifact, Option<tempfile::TempDir>), Box<dyn Error>> {
    if let Some(root) = std::env::var_os("EUTHETO_TEST_ORTOOLS_ARTIFACT") {
        let root = std::path::PathBuf::from(root);
        let manifest = tokio::fs::read(root.join("solver-manifest.json")).await?;
        let manifest_sha256: [u8; 32] = Sha256::digest(manifest).into();
        return Ok((
            VerifiedWorkerArtifact::verify(root, manifest_sha256).await?,
            None,
        ));
    }

    let (artifact, directory, _) = fixture_artifact().await?;
    Ok((artifact, Some(directory)))
}

async fn fixture_artifact()
-> Result<(VerifiedWorkerArtifact, tempfile::TempDir, [u8; 32]), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let bin = directory.path().join("bin");
    tokio::fs::create_dir(&bin).await?;
    let worker = bin.join(expected_worker_name());
    tokio::fs::copy(Path::new(env!("CARGO_BIN_EXE_worker-helper")), &worker).await?;
    let worker_sha256: [u8; 32] = Sha256::digest(tokio::fs::read(&worker).await?).into();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "approval": null,
        "backend_source": {
            "kind": "ortools",
            "sha256": "fixture",
            "source_url": "https://example.invalid/fixture",
            "version": ORTOOLS_VERSION
        },
        "build": {
            "architecture": current_architecture(),
            "cmake": {},
            "compiler": {},
            "linkage": "fixture",
            "target_triple": current_target_triple()
        },
        "capabilities": [
            "cp-sat",
            "deterministic-time",
            "intermediate-solutions",
            "objective-bounds",
            "progress",
            "solution-projection",
            "solution-stats"
        ],
        "licenses": null,
        "manifest": {"generation_contract_version": 1, "schema_version": 1},
        "protobuf": null,
        "protocol": {
            "major": 1,
            "minor": 1,
            "schema_sha256": "fixture",
            "wire_version": 1
        },
        "runtime_libraries": [],
        "sbom": null,
        "worker": {
            "adapter_version": ORTOOLS_ADAPTER_VERSION,
            "backend_id": "ortools-cp-sat",
            "distribution": "bundled-worker",
            "executable": {
                "path": format!("bin/{}", expected_worker_name()),
                "sha256": hex_sha256(&worker_sha256)?
            },
            "identity": "eutheto-ortools-worker",
            "stability": "beta",
            "version": "0.1.0"
        }
    }))?;
    tokio::fs::write(directory.path().join("solver-manifest.json"), &manifest).await?;
    let manifest_sha256: [u8; 32] = Sha256::digest(manifest).into();
    let artifact =
        VerifiedWorkerArtifact::verify(directory.path().to_owned(), manifest_sha256).await?;
    Ok((artifact, directory, manifest_sha256))
}

fn hex_sha256(digest: &[u8; 32]) -> Result<String, std::string::FromUtf8Error> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = Vec::with_capacity(64);
    for byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)]);
        encoded.push(HEX[usize::from(byte & 0x0f)]);
    }
    String::from_utf8(encoded)
}

#[cfg(windows)]
const fn expected_worker_name() -> &'static str {
    "ortools-worker.exe"
}

#[cfg(not(windows))]
const fn expected_worker_name() -> &'static str {
    "ortools-worker"
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

#[cfg(target_arch = "x86_64")]
const fn current_architecture() -> &'static str {
    "x86_64"
}
#[cfg(target_arch = "aarch64")]
const fn current_architecture() -> &'static str {
    "aarch64"
}

fn problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let provenance = ProvenanceId::new("backend.provenance")?;
    let variable = BoolVariableId::new("backend.selected")?;
    let latent = BoolVariableId::new("backend.latent")?;
    Ok(PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: vec![
            Variable::Boolean(BoolVariable {
                id: latent,
                provenance: provenance.clone(),
            }),
            Variable::Boolean(BoolVariable {
                id: variable.clone(),
                provenance: provenance.clone(),
            }),
        ],
        constraints: Vec::new(),
        objectives: ObjectivePlan { levels: Vec::new() },
        assumptions: Vec::new(),
        projections: vec![SolutionProjection {
            id: ProjectionId::new("backend.projection")?,
            assignment_id: DomainAssignmentId::new("backend.assignment")?,
            entity: DomainEntityRef {
                kind: DomainEntityKindId::new("backend.entity")?,
                id: DomainEntityId::new("backend.entity.one")?,
            },
            required: true,
            expression: ProjectionExpression::Boolean(variable),
            provenance: provenance.clone(),
        }],
        provenance: vec![ProvenanceRecord {
            id: provenance,
            source_kind: ProvenanceSourceKind::Fact,
            source_id: "backend.fixture".to_owned(),
            entity_refs: Vec::new(),
            message_key: "backend.fixture".to_owned(),
            parameters: BTreeMap::new(),
            parent: None,
        }],
        metadata: PlanningMetadata {
            pack_id: PackId::new("official.synthetic")?,
            scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
            scenario_revision: 1,
            projection_version: PROJECTION_SCHEMA_VERSION,
            compiler_id: CompilerId::new("compiler.backend")?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: BTreeMap::new(),
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::from([Capability::BooleanProjection]),
        split_authorization: None,
    })
}

fn options() -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend: BackendSelection::Specific(ORTOOLS_BACKEND_ID.parse()?),
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(3_000)?,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 1,
        solution_limit: None,
        stop_after_first_feasible: false,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::None,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 100,
            max_rules: 100,
            max_variables: 100,
            max_constraints: 100,
        },
    })
}
#[tokio::test]
async fn artifact_verification_rejects_untrusted_manifest_and_tampered_worker() -> TestResult {
    let (_artifact, directory, manifest_sha256) = fixture_artifact().await?;
    assert!(matches!(
        VerifiedWorkerArtifact::verify(directory.path().to_owned(), [0_u8; 32]).await,
        Err(BundledWorkerArtifactError::ManifestDigestMismatch)
    ));
    assert!(matches!(
        VerifiedWorkerArtifact::verify_packaged(
            directory.path().to_owned(),
            expected_worker_name(),
            manifest_sha256,
        )
        .await,
        Err(BundledWorkerArtifactError::UnexpectedExecutablePath)
    ));
    let worker_path = directory.path().join("bin").join(expected_worker_name());
    let mut worker = tokio::fs::read(&worker_path).await?;
    let packaged_directory = tempfile::tempdir()?;
    let packaged_worker = packaged_directory.path().join(expected_worker_name());
    tokio::fs::copy(&worker_path, &packaged_worker).await?;
    tokio::fs::remove_file(&worker_path).await?;
    VerifiedWorkerArtifact::verify_packaged(
        directory.path().to_owned(),
        packaged_worker,
        manifest_sha256,
    )
    .await?;

    worker[0] ^= 0xff;
    tokio::fs::write(&worker_path, worker).await?;
    assert!(matches!(
        VerifiedWorkerArtifact::verify(directory.path().to_owned(), manifest_sha256).await,
        Err(BundledWorkerArtifactError::Executable(
            ExecutableIdentityError::DigestMismatch
        ))
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn production_backend_translates_supervises_and_submits_candidate() -> TestResult {
    let (artifact, _artifact_guard) = verified_artifact().await?;
    let registry = registry_with_ortools(artifact)?;
    assert_eq!(registry.len(), 1);
    let backend_id = ORTOOLS_BACKEND_ID.parse()?;
    let backend = registry
        .get(&backend_id)
        .ok_or("missing OR-Tools backend")?;
    assert_eq!(backend.descriptor().version, ORTOOLS_VERSION);
    assert_eq!(
        backend.descriptor().adapter_version,
        ORTOOLS_ADAPTER_VERSION
    );

    let problem = Arc::new(problem()?);
    let summary = summarize(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let options = options()?;
    assert!(backend.compatibility(&summary, &options).compatible());
    let parent_budget = ParentSolveBudget::new(
        options.time_limit_milliseconds,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    let request = SolveRequest::new(
        backend_id,
        ORTOOLS_VERSION,
        ORTOOLS_ADAPTER_VERSION,
        Arc::clone(&problem),
        summary,
        options,
        &parent_budget,
        None,
        DurationMillis::ZERO,
    )?;
    let mut progress = Progress(Vec::new());
    let mut output = BoundedBackendOutput::new(
        &problem,
        &mut progress,
        request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;

    let outcome = backend
        .solve(&request, &mut output as &mut dyn BackendOutputSink)
        .await?;
    let result = output.into_result(outcome);
    validate_outcome(
        &request,
        &result.outcome,
        &result.candidates,
        SolverApiLimits::DEFAULT,
    )?;
    assert_eq!(
        result.outcome.termination,
        BackendTerminationReason::OptimalityClaimed
    );
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].values.booleans.len(), 2);
    assert!(!result.candidates[0].values.booleans[&BoolVariableId::new("backend.selected")?]);
    assert!(matches!(
        progress.0.first(),
        Some(SolveProgressEvent::BackendStarted { .. })
    ));
    assert!(matches!(
        progress.0.last(),
        Some(SolveProgressEvent::IncumbentFound(_))
    ));
    let execution = result
        .outcome
        .evidence
        .execution
        .as_ref()
        .ok_or("missing OR-Tools execution evidence")?;
    assert_eq!(
        execution.model_counts.planning_variable_count,
        request.summary().variable_count
    );
    assert_eq!(
        execution.model_counts.planning_constraint_count,
        request.summary().constraint_count
    );
    assert_eq!(execution.model_counts.translated_variable_count, 2);
    assert_eq!(execution.model_counts.translated_constraint_count, 0);
    assert_eq!(execution.reproducibility.backend_version, ORTOOLS_VERSION);
    assert_eq!(
        execution.reproducibility.adapter_version,
        ORTOOLS_ADAPTER_VERSION
    );
    assert_eq!(execution.reproducibility.worker_version, "0.1.0");
    assert_eq!(execution.reproducibility.engine_version, ORTOOLS_VERSION);
    assert_eq!(
        execution.reproducibility.applied_parameters.random_seed,
        i32::try_from(request.options().random_seed)?
    );
    assert_eq!(
        execution.reproducibility.applied_parameters.worker_threads,
        1
    );
    assert!(
        !execution
            .reproducibility
            .applied_parameters
            .emit_intermediate_solutions
    );
    assert!(
        execution
            .reproducibility
            .applied_parameters
            .deterministic_test_profile
    );
    assert_eq!(
        execution.reproducibility.applied_options,
        *request.options()
    );
    assert_eq!(execution.reproducibility.model_fingerprint_sha256.len(), 64);
    assert_eq!(
        execution
            .reproducibility
            .applied_parameters_sha256
            .as_deref()
            .map(str::len),
        Some(64)
    );
    assert!(
        execution
            .reproducibility
            .applied_parameters
            .wall_time_milliseconds
            .is_some()
    );
    assert!(execution.timings.worker_startup_milliseconds.is_some());
    assert!(execution.timings.handshake_milliseconds.is_some());
    assert!(execution.timings.solver_milliseconds.is_some());
    assert!(execution.timings.protocol_decode_milliseconds.is_some());
    assert!(execution.worker_statistics.is_some());
    if std::env::var_os("EUTHETO_TEST_ORTOOLS_ARTIFACT").is_some() {
        let statistics = execution
            .worker_statistics
            .as_ref()
            .ok_or("missing production worker statistics")?;
        assert!(statistics.wall_time_milliseconds.is_some());
        assert!(statistics.deterministic_time_milliseconds.is_some());
        assert!(statistics.conflicts.is_some());
        assert!(statistics.branches.is_some());
        assert!(statistics.binary_propagations.is_some());
        assert!(statistics.integer_propagations.is_some());
    }
    Ok(())
}
