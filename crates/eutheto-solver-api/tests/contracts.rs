use eutheto_planning_ir::{
    BoolVariable, BoolVariableId, CandidateValues, Capability, CompilerId, InclusiveRange,
    IntDomain, IntVariable, IntVariableId, LexicographicStrategy, ObjectiveLevel, ObjectiveLevelId,
    ObjectivePlan, PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningMetadata,
    PlanningProblem, PlanningProblemSummary, ProvenanceId, ProvenanceRecord, ProvenanceSourceKind,
    Variable, summarize,
};
use eutheto_solver_api::*;
use eutheto_types::{
    BackendId, BackendSelection, CancellationToken, DurationMillis, ExplanationMode,
    FixedMonotonicClock, MonotonicClock, PackId, ParentSolveBudget, PreservationPolicy,
    ReproducibilityMode, ResourceLimits, ScenarioId, SolveMode, SolveOptions, WorkerThreadPolicy,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const FAKE_BACKEND_ID: &str = "tests.fake-exact";
type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct ValidationCrossingClock {
    reads: AtomicUsize,
}

impl MonotonicClock for ValidationCrossingClock {
    fn now(&self) -> Duration {
        if self.reads.fetch_add(1, Ordering::Relaxed) < 3 {
            Duration::ZERO
        } else {
            Duration::from_millis(900)
        }
    }
}

fn support_features() -> Result<Vec<SupportFeature>, SupportMatrixError> {
    SUPPORT_FEATURES
        .iter()
        .map(|(id, category, gate)| {
            let category = match *category {
                "primitive" => SupportFeatureCategory::Primitive,
                "objective" => SupportFeatureCategory::Objective,
                "projection" => SupportFeatureCategory::Projection,
                "solve" => SupportFeatureCategory::Solve,
                other => {
                    return Err(SupportMatrixError::UnknownGeneratedCategory(
                        other.to_owned(),
                    ));
                }
            };
            Ok(SupportFeature {
                id: SupportFeatureId::new(*id)?,
                category,
                gate: if *gate == "unconditional" {
                    SupportFeatureGate::Unconditional
                } else {
                    SupportFeatureGate::Enabled((*gate).to_owned())
                },
            })
        })
        .collect()
}

fn matrix_and_descriptor(
    overrides: &BTreeMap<&str, SupportCell>,
) -> Result<(CapabilityMatrix, SolverDescriptor), Box<dyn Error>> {
    let backend_id = BackendId::new(FAKE_BACKEND_ID)?;
    let mut supported = BTreeSet::new();
    let mut degraded = BTreeSet::new();
    let mut cells = Vec::new();
    for feature in support_features()? {
        let cell = overrides
            .get(feature.id.as_str())
            .cloned()
            .unwrap_or_else(|| SupportCell::Supported {
                fixture_id: format!("fixture.{}", feature.id.as_str().replace('.', "-")),
            });
        match cell {
            SupportCell::Supported { .. } => {
                supported.insert(feature.id.clone());
            }
            SupportCell::Degraded { .. } => {
                degraded.insert(feature.id.clone());
            }
            SupportCell::Unsupported { .. } => {}
        }
        cells.push((feature.id, cell));
    }
    let descriptor = SolverDescriptor {
        id: backend_id.clone(),
        display_name: "Deterministic test backend".to_owned(),
        version: "1.0.0".to_owned(),
        adapter_version: "1.0.0".to_owned(),
        distribution: SolverDistribution::BuiltIn,
        license: LicenseMetadata {
            spdx_expression: "Apache-2.0".to_owned(),
            license_name: "Apache License 2.0".to_owned(),
            source_url: Some("https://example.invalid/test-backend".to_owned()),
        },
        stability: BackendStability::Experimental,
        capabilities: SolverCapabilities {
            supported,
            degraded,
        },
    };
    let matrix = CapabilityMatrix::new(
        SUPPORT_MATRIX_SCHEMA_VERSION,
        SUPPORT_MATRIX_IR_SCHEMA_VERSION,
        support_features()?,
        vec![BackendSupportColumn {
            backend_id,
            backend_version: descriptor.version.clone(),
            adapter_version: descriptor.adapter_version.clone(),
            cells,
        }],
        Vec::new(),
    )?;
    Ok((matrix, descriptor))
}

fn runtime_identity(
    descriptor: &SolverDescriptor,
) -> Result<BackendRuntimeIdentity, BackendRuntimeIdentityError> {
    BackendRuntimeIdentity::new(
        descriptor.id.clone(),
        descriptor.version.clone(),
        descriptor.adapter_version.clone(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        1,
        0,
    )
}

fn problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let provenance = ProvenanceId::new("tests.provenance")?;
    let mut value = PlanningProblem {
        schema_version: PLANNING_IR_SCHEMA_VERSION,
        variables: vec![
            Variable::Boolean(BoolVariable {
                id: BoolVariableId::new("tests.enabled")?,
                provenance: provenance.clone(),
            }),
            Variable::Integer(IntVariable {
                id: IntVariableId::new("tests.count")?,
                domain: IntDomain::new(vec![InclusiveRange { start: 1, end: 3 }])?,
                provenance: provenance.clone(),
            }),
        ],
        constraints: Vec::new(),
        objectives: ObjectivePlan::default(),
        assumptions: Vec::new(),
        projections: Vec::new(),
        provenance: vec![ProvenanceRecord {
            id: provenance,
            source_kind: ProvenanceSourceKind::Fact,
            source_id: "tests.source".to_owned(),
            entity_refs: Vec::new(),
            message_key: "tests.message".to_owned(),
            parameters: BTreeMap::new(),
            parent: None,
        }],
        metadata: PlanningMetadata {
            pack_id: PackId::new("tests.synthetic")?,
            scenario_id: "01890a5d-ac96-7b64-9f74-bbfcf30f9f80".parse::<ScenarioId>()?,
            scenario_revision: 4,
            projection_version: 1,
            compiler_id: CompilerId::new("tests.compiler")?,
            compiler_version: "1.0.0".to_owned(),
            compile_metadata: BTreeMap::new(),
            display_text: BTreeMap::new(),
        },
        declared_capabilities: BTreeSet::new(),
        split_authorization: None,
    };
    value.canonicalize()?;
    Ok(value)
}

fn solve_options(backend: BackendId) -> Result<SolveOptions, Box<dyn Error>> {
    Ok(SolveOptions {
        backend: BackendSelection::Specific(backend),
        mode: SolveMode::Balanced,
        time_limit_milliseconds: DurationMillis::new(1_000)?,
        memory_limit_bytes: Some(64 * 1024 * 1024),
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 7,
        solution_limit: Some(4),
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

fn parent_budget(
    milliseconds: u64,
) -> Result<(ParentSolveBudget, FixedMonotonicClock, CancellationToken), Box<dyn Error>> {
    let clock = FixedMonotonicClock::default();
    let cancellation = CancellationToken::new();
    let parent = ParentSolveBudget::new(
        DurationMillis::new(milliseconds)?,
        Arc::new(clock.clone()),
        cancellation.clone(),
    )?;
    Ok((parent, clock, cancellation))
}

fn request(
    backend: BackendId,
) -> Result<(SolveRequest, FixedMonotonicClock, CancellationToken), Box<dyn Error>> {
    let problem = Arc::new(problem()?);
    let options = solve_options(backend.clone())?;
    let summary = summarize(
        &problem,
        PlanningIrLimitsV1::DEFAULT.tightened_by(options.resource_limits),
    )?;
    let (parent, clock, cancellation) = parent_budget(1_000)?;
    Ok((
        SolveRequest::new(
            backend,
            "1.0.0",
            "1.0.0",
            problem,
            summary,
            options,
            &parent,
            Some(DurationMillis::new(800)?),
            DurationMillis::ZERO,
        )?,
        clock,
        cancellation,
    ))
}

fn valid_candidate() -> Result<CandidateValues, Box<dyn Error>> {
    Ok(CandidateValues {
        booleans: BTreeMap::from([(BoolVariableId::new("tests.enabled")?, true)]),
        integers: BTreeMap::from([(IntVariableId::new("tests.count")?, 2)]),
    })
}

struct VecProgress(Vec<SolveProgressEvent>);

impl ProgressSink for VecProgress {
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        self.0.push(event);
        Ok(())
    }
}

struct SequenceBackend {
    descriptor: SolverDescriptor,
    runtime_identity: BackendRuntimeIdentity,
    matrix: CapabilityMatrix,
    candidates: Vec<CandidateSubmission>,
}

impl SolverBackend for SequenceBackend {
    fn descriptor(&self) -> &SolverDescriptor {
        &self.descriptor
    }

    fn runtime_identity(&self) -> &BackendRuntimeIdentity {
        &self.runtime_identity
    }

    fn compatibility(
        &self,
        problem: &PlanningProblemSummary,
        options: &SolveOptions,
    ) -> CompatibilityReport {
        match compatibility_for(&self.matrix, &self.descriptor.id, problem, options) {
            Ok(report) => report,
            Err(_) => CompatibilityReport {
                level: CompatibilityLevel::Unsupported,
                unsupported_features: Vec::new(),
                warnings: Vec::new(),
                estimated_translation_cost: None,
            },
        }
    }

    fn solve<'a>(
        &'a self,
        request: &'a SolveRequest,
        output: &'a mut dyn BackendOutputSink,
    ) -> BackendSolveFuture<'a> {
        Box::pin(async move {
            output.emit_progress(SolveProgressEvent::BackendStarted {
                backend: self.descriptor.id.clone(),
            })?;
            for candidate in &self.candidates {
                let accepted = output.submit_candidate(candidate.clone())?;
                output.emit_progress(SolveProgressEvent::IncumbentFound(IncumbentSummary {
                    sequence: accepted.sequence,
                    observed_after_milliseconds: accepted.observed_after_milliseconds,
                    objective: accepted.objective,
                }))?;
            }
            Ok(BackendSolveOutcome {
                backend_id: self.descriptor.id.clone(),
                model_hash: request.model_hash().to_owned(),
                solve_fingerprint: request.solve_fingerprint().to_owned(),
                termination: BackendTerminationReason::OptimalityClaimed,
                evidence: BackendTerminationEvidence {
                    remaining_at_dispatch_milliseconds: request
                        .dispatch_budget()
                        .remaining_at_dispatch(),
                    backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
                    elapsed_milliseconds: DurationMillis::new(10)
                        .map_err(|_| BackendError::from(OutputError::UnsafeDiagnosticLine))?,
                    first_incumbent_milliseconds: Some(
                        DurationMillis::new(2)
                            .map_err(|_| BackendError::from(OutputError::UnsafeDiagnosticLine))?,
                    ),
                    objective: None,
                    evidence_refs: Vec::new(),
                    execution: None,
                },
            })
        })
    }
}

#[test]
fn duplicate_registry_rejection_is_deterministic() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let runtime_identity = runtime_identity(&descriptor)?;
    let first: Arc<dyn SolverBackend> = Arc::new(SequenceBackend {
        descriptor: descriptor.clone(),
        runtime_identity: runtime_identity.clone(),
        matrix: matrix.clone(),
        candidates: Vec::new(),
    });
    let second: Arc<dyn SolverBackend> = Arc::new(SequenceBackend {
        descriptor,
        runtime_identity,
        matrix: matrix.clone(),
        candidates: Vec::new(),
    });
    let result = SolverRegistry::new(matrix, vec![first, second]);
    assert!(matches!(result, Err(RegistryError::DuplicateBackend(_))));
    Ok(())
}

#[test]
fn registry_exposes_valid_runtime_identity_without_downcasting() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let backend_id = descriptor.id.clone();
    let runtime_identity = runtime_identity(&descriptor)?;
    let backend: Arc<dyn SolverBackend> = Arc::new(SequenceBackend {
        descriptor,
        runtime_identity: runtime_identity.clone(),
        matrix: matrix.clone(),
        candidates: Vec::new(),
    });

    let registry = SolverRegistry::new(matrix, [backend])?;
    let registered = registry
        .get(&backend_id)
        .ok_or("backend was not registered")?;
    assert_eq!(registered.runtime_identity(), &runtime_identity);
    assert_eq!(registered.runtime_identity().backend_id(), &backend_id);
    assert_eq!(registered.runtime_identity().backend_version(), "1.0.0");
    assert_eq!(registered.runtime_identity().adapter_version(), "1.0.0");
    assert_eq!(registered.runtime_identity().worker_version(), "1.0.0");
    assert_eq!(registered.runtime_identity().solver_version(), "1.0.0");
    assert_eq!(registered.runtime_identity().protocol_major(), 1);
    assert_eq!(registered.runtime_identity().protocol_minor(), 0);
    Ok(())
}

#[test]
fn runtime_identity_rejects_each_invalid_version_and_zero_protocol_major() -> TestResult {
    let (_, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let identity = |backend_version: &str,
                    adapter_version: &str,
                    worker_version: &str,
                    solver_version: &str,
                    protocol_major| {
        BackendRuntimeIdentity::new(
            descriptor.id.clone(),
            backend_version.to_owned(),
            adapter_version.to_owned(),
            worker_version.to_owned(),
            solver_version.to_owned(),
            protocol_major,
            0,
        )
    };
    let oversized_version = "x".repeat(65);

    assert_eq!(
        identity("invalid version", "1.0.0", "1.0.0", "1.0.0", 1),
        Err(BackendRuntimeIdentityError::InvalidBackendVersion)
    );
    assert_eq!(
        identity("1.0.0", "invalid version", "1.0.0", "1.0.0", 1),
        Err(BackendRuntimeIdentityError::InvalidAdapterVersion)
    );
    assert_eq!(
        identity("1.0.0", "1.0.0", &oversized_version, "1.0.0", 1),
        Err(BackendRuntimeIdentityError::InvalidWorkerVersion)
    );
    assert_eq!(
        identity("1.0.0", "1.0.0", "1.0.0", "invalid version", 1),
        Err(BackendRuntimeIdentityError::InvalidSolverVersion)
    );
    assert_eq!(
        identity("1.0.0", "1.0.0", "1.0.0", "1.0.0", 0),
        Err(BackendRuntimeIdentityError::ZeroProtocolMajor)
    );
    Ok(())
}

#[test]
fn registry_revalidates_deserialized_runtime_identity() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let invalid_identity = serde_json::from_value(serde_json::json!({
        "backendId": descriptor.id.as_str(),
        "backendVersion": descriptor.version.clone(),
        "adapterVersion": descriptor.adapter_version.clone(),
        "workerVersion": "invalid version",
        "solverVersion": "1.0.0",
        "protocolMajor": 1,
        "protocolMinor": 0
    }))?;
    let backend: Arc<dyn SolverBackend> = Arc::new(SequenceBackend {
        descriptor,
        runtime_identity: invalid_identity,
        matrix: matrix.clone(),
        candidates: Vec::new(),
    });

    assert!(matches!(
        SolverRegistry::new(matrix, [backend]),
        Err(RegistryError::InvalidRuntimeIdentity {
            error: BackendRuntimeIdentityError::InvalidWorkerVersion,
            ..
        })
    ));
    Ok(())
}

#[test]
fn registry_rejects_each_runtime_identity_descriptor_mismatch() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let backend = |runtime_identity| -> Arc<dyn SolverBackend> {
        Arc::new(SequenceBackend {
            descriptor: descriptor.clone(),
            runtime_identity,
            matrix: matrix.clone(),
            candidates: Vec::new(),
        })
    };

    let other_backend_id = BackendId::new("tests.other-exact")?;
    let identity = BackendRuntimeIdentity::new(
        other_backend_id.clone(),
        descriptor.version.clone(),
        descriptor.adapter_version.clone(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        1,
        0,
    )?;
    assert!(matches!(
        SolverRegistry::new(matrix.clone(), [backend(identity)]),
        Err(RegistryError::RuntimeIdentityBackendIdMismatch {
            descriptor_backend_id,
            identity_backend_id,
        }) if descriptor_backend_id == descriptor.id && identity_backend_id == other_backend_id
    ));

    let identity = BackendRuntimeIdentity::new(
        descriptor.id.clone(),
        "2.0.0".to_owned(),
        descriptor.adapter_version.clone(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        1,
        0,
    )?;
    assert!(matches!(
        SolverRegistry::new(matrix.clone(), [backend(identity)]),
        Err(RegistryError::RuntimeIdentityBackendVersionMismatch(backend_id))
            if backend_id == descriptor.id
    ));

    let identity = BackendRuntimeIdentity::new(
        descriptor.id.clone(),
        descriptor.version.clone(),
        "2.0.0".to_owned(),
        "1.0.0".to_owned(),
        "1.0.0".to_owned(),
        1,
        0,
    )?;
    assert!(matches!(
        SolverRegistry::new(matrix.clone(), [backend(identity)]),
        Err(RegistryError::RuntimeIdentityAdapterVersionMismatch(backend_id))
            if backend_id == descriptor.id
    ));
    Ok(())
}

#[test]
fn compatibility_preserves_exact_degraded_and_unsupported_reasons() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::from([
        (
            "ir.bool-or",
            SupportCell::Degraded {
                restriction_id: "restriction.bool-count".to_owned(),
                reason: "Boolean clauses are restricted to 32 literals.".to_owned(),
                remediation: "Reduce the clause or choose another backend.".to_owned(),
                fixture_id: "fixture.bool-count".to_owned(),
            },
        ),
        (
            "ir.integer-linear",
            SupportCell::Unsupported {
                reason: "Integer linear comparisons are unavailable.".to_owned(),
                remediation: "Choose a backend with integer-linear support.".to_owned(),
                fixture_id: "fixture.integer-unsupported".to_owned(),
            },
        ),
    ]))?;
    let mut summary = summarize(&problem()?, PlanningIrLimitsV1::DEFAULT)?;
    summary.manifest.capability_counts =
        BTreeMap::from([(Capability::BoolOr, 2), (Capability::LinearComparison, 1)]);
    let options = solve_options(descriptor.id.clone())?;
    let report = compatibility_for(&matrix, &descriptor.id, &summary, &options)?;
    assert_eq!(report.level, CompatibilityLevel::Unsupported);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].restriction_id, "restriction.bool-count");
    assert_eq!(report.unsupported_features.len(), 1);
    assert_eq!(
        report.unsupported_features[0].reason,
        "Integer linear comparisons are unavailable."
    );
    summary.manifest.capability_counts = BTreeMap::from([(Capability::BoolOr, 2)]);
    let degraded = compatibility_for(&matrix, &descriptor.id, &summary, &options)?;
    assert_eq!(degraded.level, CompatibilityLevel::Degraded);
    assert!(degraded.unsupported_features.is_empty());
    summary.manifest.capability_counts.clear();
    let supported = compatibility_for(&matrix, &descriptor.id, &summary, &options)?;
    assert_eq!(supported.level, CompatibilityLevel::Supported);
    assert!(supported.warnings.is_empty());
    Ok(())
}

#[test]
fn preflight_rejects_request_option_backend_version_and_adapter_mismatch() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let (original, _, _) = request(descriptor.id.clone())?;
    let mut options = original.options().clone();
    options.backend = BackendSelection::Specific(BackendId::new("tests.other")?);
    let (parent, _, _) = parent_budget(1_000)?;
    let mismatched = SolveRequest::new(
        descriptor.id.clone(),
        &descriptor.version,
        &descriptor.adapter_version,
        Arc::clone(original.problem()),
        original.summary().clone(),
        options,
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert!(matches!(
        preflight(&matrix, &descriptor, &mismatched),
        Err(PreflightError::OptionBackendMismatch)
    ));
    let other = BackendId::new("tests.other")?;
    let (other_parent, _, _) = parent_budget(1_000)?;
    let wrong_request = SolveRequest::new(
        other.clone(),
        &descriptor.version,
        &descriptor.adapter_version,
        Arc::clone(original.problem()),
        original.summary().clone(),
        solve_options(other)?,
        &other_parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert!(matches!(
        preflight(&matrix, &descriptor, &wrong_request),
        Err(PreflightError::RequestBackendMismatch)
    ));
    let (version_parent, _, _) = parent_budget(1_000)?;
    let wrong_version = SolveRequest::new(
        descriptor.id.clone(),
        "9.9.9",
        &descriptor.adapter_version,
        Arc::clone(original.problem()),
        original.summary().clone(),
        solve_options(descriptor.id.clone())?,
        &version_parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert!(matches!(
        preflight(&matrix, &descriptor, &wrong_version),
        Err(PreflightError::BackendVersionMismatch)
    ));
    let (adapter_parent, _, _) = parent_budget(1_000)?;
    let wrong_adapter = SolveRequest::new(
        descriptor.id.clone(),
        &descriptor.version,
        "9.9.9",
        Arc::clone(original.problem()),
        original.summary().clone(),
        solve_options(descriptor.id.clone())?,
        &adapter_parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert!(matches!(
        preflight(&matrix, &descriptor, &wrong_adapter),
        Err(PreflightError::AdapterVersionMismatch)
    ));
    Ok(())
}

#[test]
fn preflight_maps_shared_invalid_solve_options() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let problem = Arc::new(problem()?);
    let summary = summarize(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let base = solve_options(descriptor.id.clone())?;
    let invalid_options = [
        {
            let mut value = base.clone();
            value.memory_limit_bytes = Some(0);
            value
        },
        {
            let mut value = base.clone();
            value.worker_threads = WorkerThreadPolicy::Exact(0);
            value
        },
        {
            let mut value = base;
            value.solution_limit = Some(0);
            value
        },
    ];
    for options in invalid_options {
        let (parent, _, _) = parent_budget(1_000)?;
        let request = SolveRequest::new(
            descriptor.id.clone(),
            &descriptor.version,
            &descriptor.adapter_version,
            Arc::clone(&problem),
            summary.clone(),
            options,
            &parent,
            None,
            DurationMillis::ZERO,
        )?;
        assert!(matches!(
            preflight(&matrix, &descriptor, &request),
            Err(PreflightError::InvalidSolveOptions)
        ));
    }
    Ok(())
}

#[test]
fn cancellation_and_timeout_remain_distinct() -> TestResult {
    let (cancel_parent, _, cancellation) = parent_budget(100)?;
    let problem = Arc::new(problem()?);
    let backend = BackendId::new(FAKE_BACKEND_ID)?;
    let options = solve_options(backend.clone())?;
    let summary = summarize(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let cancelled = SolveRequest::new(
        backend.clone(),
        "1.0.0",
        "1.0.0",
        Arc::clone(&problem),
        summary.clone(),
        options.clone(),
        &cancel_parent,
        None,
        DurationMillis::ZERO,
    )?;
    cancellation.cancel();
    assert_eq!(
        cancelled.dispatch_budget().stop_reason(),
        Some(BackendStopReason::Cancelled)
    );

    let (timeout_parent, timeout_clock, _) = parent_budget(100)?;
    let timed_out = SolveRequest::new(
        backend,
        "1.0.0",
        "1.0.0",
        problem,
        summary,
        options,
        &timeout_parent,
        None,
        DurationMillis::ZERO,
    )?;
    timeout_clock.advance(Duration::from_millis(100))?;
    assert_eq!(
        timed_out.dispatch_budget().stop_reason(),
        Some(BackendStopReason::DeadlineExceeded)
    );
    Ok(())
}

#[test]
fn child_budget_inherits_the_absolute_parent_deadline() -> TestResult {
    let (parent, clock, _) = parent_budget(1_000)?;
    clock.advance(Duration::from_millis(400))?;
    let problem = Arc::new(problem()?);
    let backend = BackendId::new(FAKE_BACKEND_ID)?;
    let options = solve_options(backend.clone())?;
    let summary = summarize(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let request = SolveRequest::new(
        backend,
        "1.0.0",
        "1.0.0",
        problem,
        summary,
        options,
        &parent,
        Some(DurationMillis::new(500)?),
        DurationMillis::ZERO,
    )?;
    assert_eq!(
        request.dispatch_budget().remaining_at_dispatch().value(),
        600
    );
    assert_eq!(request.dispatch_budget().backend_limit().value(), 500);
    clock.advance(Duration::from_millis(200))?;
    assert_eq!(
        request
            .dispatch_budget()
            .child_view()
            .snapshot()
            .remaining_milliseconds
            .value(),
        400
    );
    Ok(())
}

#[test]
fn backend_output_rejects_live_output_after_every_authoritative_stop() -> TestResult {
    let submission = CandidateSubmission {
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(1)?,
        objective: None,
        evidence_refs: Vec::new(),
    };

    let (cap_request, cap_clock, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let mut cap_progress = VecProgress(Vec::new());
    let mut cap_output = BoundedBackendOutput::new(
        cap_request.problem(),
        &mut cap_progress,
        cap_request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    cap_clock.advance(Duration::from_millis(800))?;
    assert_eq!(
        cap_output.emit_progress(SolveProgressEvent::Queued),
        Err(OutputError::BackendLimitExceeded)
    );
    assert_eq!(
        cap_output.submit_candidate(submission.clone()),
        Err(OutputError::BackendLimitExceeded)
    );
    assert!(cap_output.candidates().is_empty());
    drop(cap_output);
    assert!(cap_progress.0.is_empty());

    let (deadline_request, deadline_clock, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let mut deadline_progress = VecProgress(Vec::new());
    let mut deadline_output = BoundedBackendOutput::new(
        deadline_request.problem(),
        &mut deadline_progress,
        deadline_request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    deadline_clock.advance(Duration::from_secs(1))?;
    assert_eq!(
        deadline_output.emit_progress(SolveProgressEvent::Queued),
        Err(OutputError::ParentDeadlineExceeded)
    );

    let (cancel_request, _, cancellation) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let mut cancel_progress = VecProgress(Vec::new());
    let mut cancel_output = BoundedBackendOutput::new(
        cancel_request.problem(),
        &mut cancel_progress,
        cancel_request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    cancellation.cancel();
    assert_eq!(
        cancel_output.submit_candidate(submission),
        Err(OutputError::Cancelled)
    );
    Ok(())
}

#[test]
fn candidate_crossing_backend_cap_during_validation_is_not_retained() -> TestResult {
    let clock = Arc::new(ValidationCrossingClock::default());
    let parent = ParentSolveBudget::new(
        DurationMillis::new(1_000)?,
        clock.clone(),
        CancellationToken::new(),
    )?;
    let problem = Arc::new(problem()?);
    let options = solve_options(BackendId::new(FAKE_BACKEND_ID)?)?;
    let summary = summarize(
        &problem,
        PlanningIrLimitsV1::DEFAULT.tightened_by(options.resource_limits),
    )?;
    let request = SolveRequest::new(
        BackendId::new(FAKE_BACKEND_ID)?,
        "1.0.0",
        "1.0.0",
        problem,
        summary,
        options,
        &parent,
        Some(DurationMillis::new(800)?),
        DurationMillis::ZERO,
    )?;
    let mut progress = VecProgress(Vec::new());
    let mut output = BoundedBackendOutput::new(
        request.problem(),
        &mut progress,
        request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    assert_eq!(
        output.submit_candidate(CandidateSubmission {
            values: valid_candidate()?,
            observed_after_milliseconds: DurationMillis::new(1)?,
            objective: None,
            evidence_refs: Vec::new(),
        }),
        Err(OutputError::BackendLimitExceeded)
    );
    assert!(output.candidates().is_empty());
    assert_eq!(output.candidate_assignment_count(), 0);
    Ok(())
}

#[test]
fn event_and_candidate_limits_are_hard() -> TestResult {
    let (budget_request, _, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let problem = budget_request.problem();
    let mut progress = VecProgress(Vec::new());
    let limits = SolverApiLimits {
        max_candidates: 1,
        max_candidate_assignments: 2,
        max_progress_events: 1,
        max_diagnostic_lines: 1,
        max_evidence_refs_per_candidate: 1,
    };
    let mut output = BoundedBackendOutput::new(
        problem,
        &mut progress,
        budget_request.dispatch_budget(),
        limits,
    )?;
    output.emit_progress(SolveProgressEvent::Queued)?;
    assert_eq!(
        output.emit_progress(SolveProgressEvent::Queued),
        Err(OutputError::ProgressLimitExceeded)
    );
    let submission = CandidateSubmission {
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(1)?,
        objective: None,
        evidence_refs: Vec::new(),
    };
    output.submit_candidate(submission.clone())?;
    assert_eq!(
        output.submit_candidate(submission),
        Err(OutputError::CandidateLimitExceeded)
    );
    Ok(())
}

#[test]
fn candidate_assignment_limit_is_cumulative_at_boundary() -> TestResult {
    let (budget_request, _, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let problem = budget_request.problem();
    let submission = CandidateSubmission {
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(1)?,
        objective: None,
        evidence_refs: Vec::new(),
    };
    let limits = SolverApiLimits {
        max_candidates: 3,
        max_candidate_assignments: 4,
        max_progress_events: 1,
        max_diagnostic_lines: 1,
        max_evidence_refs_per_candidate: 1,
    };
    let mut progress = VecProgress(Vec::new());
    let mut boundary = BoundedBackendOutput::new(
        problem,
        &mut progress,
        budget_request.dispatch_budget(),
        limits,
    )?;
    boundary.submit_candidate(submission.clone())?;
    boundary.submit_candidate(submission.clone())?;
    assert_eq!(boundary.candidate_assignment_count(), 4);
    drop(boundary);

    let mut progress = VecProgress(Vec::new());
    let mut one_over = BoundedBackendOutput::new(
        problem,
        &mut progress,
        budget_request.dispatch_budget(),
        SolverApiLimits {
            max_candidate_assignments: 3,
            ..limits
        },
    )?;
    one_over.submit_candidate(submission.clone())?;
    assert_eq!(
        one_over.submit_candidate(submission),
        Err(OutputError::CandidateAssignmentLimitExceeded)
    );
    assert_eq!(one_over.candidate_assignment_count(), 2);
    assert_eq!(
        one_over.contract_rejection(),
        Some(&OutputError::CandidateAssignmentLimitExceeded)
    );
    Ok(())
}

#[test]
fn malformed_unknown_and_out_of_domain_candidates_are_rejected() -> TestResult {
    let problem = problem()?;
    let missing = CandidateValues {
        booleans: BTreeMap::new(),
        integers: BTreeMap::new(),
    };
    assert!(matches!(
        validate_candidate_values(&problem, &missing),
        Err(OutputError::MissingBoolean(_))
    ));
    let mut unknown = valid_candidate()?;
    unknown
        .integers
        .insert(IntVariableId::new("tests.unknown")?, 1);
    assert!(matches!(
        validate_candidate_values(&problem, &unknown),
        Err(OutputError::UnknownInteger(_))
    ));
    let mut outside = valid_candidate()?;
    outside
        .integers
        .insert(IntVariableId::new("tests.count")?, 9);
    assert!(matches!(
        validate_candidate_values(&problem, &outside),
        Err(OutputError::OutOfDomain(_))
    ));
    assert!(SafeDiagnosticLine::new("solver.line", "unsafe\nsecond line").is_err());
    Ok(())
}

#[tokio::test]
async fn deterministic_fake_backend_sequence_stays_confined_to_test_contracts() -> TestResult {
    let (matrix, descriptor) = matrix_and_descriptor(&BTreeMap::new())?;
    let runtime_identity = runtime_identity(&descriptor)?;
    let backend = SequenceBackend {
        descriptor: descriptor.clone(),
        runtime_identity,
        matrix,
        candidates: vec![
            CandidateSubmission {
                values: valid_candidate()?,
                observed_after_milliseconds: DurationMillis::new(2)?,
                objective: None,
                evidence_refs: Vec::new(),
            },
            CandidateSubmission {
                values: valid_candidate()?,
                observed_after_milliseconds: DurationMillis::new(4)?,
                objective: None,
                evidence_refs: Vec::new(),
            },
        ],
    };
    let (request, _, _) = request(descriptor.id)?;
    let mut progress = VecProgress(Vec::new());
    let mut output = BoundedBackendOutput::new(
        request.problem(),
        &mut progress,
        request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    let outcome = backend.solve(&request, &mut output).await?;
    assert_eq!(output.candidates().len(), 2);
    assert_eq!(output.candidates()[0].sequence, 1);
    assert_eq!(output.candidates()[1].sequence, 2);
    validate_outcome(
        &request,
        &outcome,
        output.candidates(),
        SolverApiLimits::DEFAULT,
    )?;
    assert_eq!(output.progress_event_count(), 3);
    Ok(())
}

#[test]
fn matrix_completeness_rejects_a_missing_cell() -> TestResult {
    let backend = BackendId::new(FAKE_BACKEND_ID)?;
    let result = CapabilityMatrix::new(
        SUPPORT_MATRIX_SCHEMA_VERSION,
        SUPPORT_MATRIX_IR_SCHEMA_VERSION,
        support_features()?,
        vec![BackendSupportColumn {
            backend_id: backend.clone(),
            backend_version: "1.0.0".to_owned(),
            adapter_version: "1.0.0".to_owned(),
            cells: Vec::new(),
        }],
        Vec::new(),
    );
    assert!(matches!(
        result,
        Err(SupportMatrixError::IncompleteColumn(id)) if id == backend
    ));
    Ok(())
}

#[test]
fn generated_matrix_has_complete_ortools_column_and_only_pumpkin_deferred() -> TestResult {
    let matrix = CapabilityMatrix::generated()?;
    assert_eq!(matrix.features().len(), SUPPORT_FEATURES.len());
    assert_eq!(
        matrix
            .production_backend_ids()
            .map(BackendId::as_str)
            .collect::<Vec<_>>(),
        vec!["solver.ortools-cp-sat"]
    );
    assert_eq!(
        matrix
            .backend_columns()
            .next()
            .map(|column| column.cells.len()),
        Some(SUPPORT_FEATURES.len())
    );
    assert_eq!(matrix.deferred_candidates().len(), 1);
    assert_eq!(
        matrix.deferred_candidates()[0].backend_id.as_str(),
        "solver.pumpkin"
    );
    Ok(())
}

fn two_level_problem() -> Result<PlanningProblem, Box<dyn Error>> {
    let mut value = problem()?;
    let provenance = value.provenance[0].id.clone();
    value.objectives.levels = ["tests.priority", "tests.preference"]
        .into_iter()
        .map(|id| -> Result<ObjectiveLevel, Box<dyn Error>> {
            Ok(ObjectiveLevel {
                id: ObjectiveLevelId::new(id)?,
                direction: serde_json::from_str("\"minimize\"")?,
                lower_bound: 0,
                upper_bound: 0,
                terms: Vec::new(),
                provenance: provenance.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    value.canonicalize()?;
    Ok(value)
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "keeping the fingerprint dimensions together makes this contract matrix auditable"
)]
fn routed_fingerprint_separates_model_route_backend_adapter_and_options() -> TestResult {
    let backend = BackendId::new(FAKE_BACKEND_ID)?;
    let base_problem = Arc::new(problem()?);
    let base_summary = summarize(&base_problem, PlanningIrLimitsV1::DEFAULT)?;
    let base_options = solve_options(backend.clone())?;
    let (parent, _, _) = parent_budget(1_000)?;
    let base = SolveRequest::new(
        backend.clone(),
        "backend-1",
        "adapter-1",
        Arc::clone(&base_problem),
        base_summary.clone(),
        base_options.clone(),
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    let repeated = SolveRequest::new(
        backend.clone(),
        "backend-1",
        "adapter-1",
        Arc::clone(&base_problem),
        base_summary.clone(),
        base_options.clone(),
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert_eq!(base.model_hash(), repeated.model_hash());
    assert_eq!(base.solve_fingerprint(), repeated.solve_fingerprint());

    let backend_version_changed = SolveRequest::new(
        backend.clone(),
        "backend-2",
        "adapter-1",
        Arc::clone(&base_problem),
        base_summary.clone(),
        base_options.clone(),
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert_eq!(base.model_hash(), backend_version_changed.model_hash());
    assert_ne!(
        base.solve_fingerprint(),
        backend_version_changed.solve_fingerprint()
    );

    let adapter_changed = SolveRequest::new(
        backend.clone(),
        "backend-1",
        "adapter-2",
        Arc::clone(&base_problem),
        base_summary.clone(),
        base_options.clone(),
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert_eq!(base.model_hash(), adapter_changed.model_hash());
    assert_ne!(
        base.solve_fingerprint(),
        adapter_changed.solve_fingerprint()
    );

    let mut changed_options = base_options.clone();
    changed_options.random_seed += 1;
    let options_changed = SolveRequest::new(
        backend.clone(),
        "backend-1",
        "adapter-1",
        Arc::clone(&base_problem),
        base_summary.clone(),
        changed_options,
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert_eq!(base.model_hash(), options_changed.model_hash());
    assert_ne!(
        base.solve_fingerprint(),
        options_changed.solve_fingerprint()
    );

    let other_backend = BackendId::new("tests.other-exact")?;
    let mut other_options = base_options.clone();
    other_options.backend = BackendSelection::Specific(other_backend.clone());
    let route_changed = SolveRequest::new(
        other_backend,
        "backend-1",
        "adapter-1",
        Arc::clone(&base_problem),
        base_summary,
        other_options,
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert_eq!(base.model_hash(), route_changed.model_hash());
    assert_ne!(base.solve_fingerprint(), route_changed.solve_fingerprint());

    let mut recompiled = (*base_problem).clone();
    recompiled.metadata.compiler_version = "2.0.0".to_owned();
    let recompiled = Arc::new(recompiled);
    let recompiled_summary = summarize(&recompiled, PlanningIrLimitsV1::DEFAULT)?;
    let compiler_changed = SolveRequest::new(
        backend,
        "backend-1",
        "adapter-1",
        recompiled,
        recompiled_summary,
        base_options,
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    assert_ne!(base.model_hash(), compiler_changed.model_hash());
    assert_ne!(
        base.solve_fingerprint(),
        compiler_changed.solve_fingerprint()
    );
    Ok(())
}

#[test]
fn multi_level_compatibility_uses_the_checked_summary_strategy() -> TestResult {
    let (scalar_matrix, descriptor) = matrix_and_descriptor(&BTreeMap::from([(
        "ir.multipass-objectives",
        SupportCell::Unsupported {
            reason: "multipass unavailable".to_owned(),
            remediation: "use exact scalarization".to_owned(),
            fixture_id: "fixture.no-multipass".to_owned(),
        },
    )]))?;
    let mut summary = summarize(&two_level_problem()?, PlanningIrLimitsV1::DEFAULT)?;
    assert!(matches!(
        summary.lexicographic_strategy,
        LexicographicStrategy::ExactScalarization { .. }
    ));
    let options = solve_options(descriptor.id.clone())?;
    assert!(compatibility_for(&scalar_matrix, &descriptor.id, &summary, &options)?.compatible());

    let (multipass_matrix, multipass_descriptor) = matrix_and_descriptor(&BTreeMap::from([(
        "ir.scalarized-objectives",
        SupportCell::Unsupported {
            reason: "scalarization unavailable".to_owned(),
            remediation: "use multipass".to_owned(),
            fixture_id: "fixture.no-scalarization".to_owned(),
        },
    )]))?;
    summary.lexicographic_strategy = LexicographicStrategy::Multipass;
    let multipass_options = solve_options(multipass_descriptor.id.clone())?;
    assert!(
        compatibility_for(
            &multipass_matrix,
            &multipass_descriptor.id,
            &summary,
            &multipass_options,
        )?
        .compatible()
    );
    Ok(())
}

#[test]
fn backend_output_rejects_application_progress_and_bad_vector_or_time_shapes() -> TestResult {
    let problem = two_level_problem()?;
    let (budget_request, _, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    for event in [
        SolveProgressEvent::Verifying,
        SolveProgressEvent::Explaining,
        SolveProgressEvent::Completed(SolveCompletionSummary {
            status: eutheto_types::SolveStatus::Feasible,
            accepted_candidate_count: 1,
        }),
    ] {
        let mut progress = VecProgress(Vec::new());
        let mut output = BoundedBackendOutput::new(
            &problem,
            &mut progress,
            budget_request.dispatch_budget(),
            SolverApiLimits::DEFAULT,
        )?;
        assert_eq!(
            output.emit_progress(event),
            Err(OutputError::BackendProgressAuthorityViolation)
        );
    }

    let mut progress = VecProgress(Vec::new());
    let mut output = BoundedBackendOutput::new(
        &problem,
        &mut progress,
        budget_request.dispatch_budget(),
        SolverApiLimits::DEFAULT,
    )?;
    assert_eq!(
        output.emit_progress(SolveProgressEvent::BoundImproved(BoundSummary {
            observed_after_milliseconds: DurationMillis::new(1)?,
            bound_values: vec![0],
        })),
        Err(OutputError::InvalidObjectiveDimension)
    );
    let malformed = CandidateSubmission {
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(1)?,
        objective: Some(BackendObjectiveEvidence {
            objective_values: vec![0],
            best_bound_values: Some(vec![0, 0]),
        }),
        evidence_refs: Vec::new(),
    };
    assert_eq!(
        output.submit_candidate(malformed),
        Err(OutputError::InvalidObjectiveDimension)
    );
    output.submit_candidate(CandidateSubmission {
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(2)?,
        objective: Some(BackendObjectiveEvidence {
            objective_values: vec![0, 0],
            best_bound_values: Some(vec![0, 0]),
        }),
        evidence_refs: Vec::new(),
    })?;
    assert_eq!(
        output.submit_candidate(CandidateSubmission {
            values: valid_candidate()?,
            observed_after_milliseconds: DurationMillis::new(1)?,
            objective: Some(BackendObjectiveEvidence {
                objective_values: vec![0, 0],
                best_bound_values: Some(vec![0, 0]),
            }),
            evidence_refs: Vec::new(),
        }),
        Err(OutputError::NonMonotonicCandidateTime)
    );
    Ok(())
}

#[test]
fn outcome_requires_both_fingerprints_and_exact_first_incumbent() -> TestResult {
    let (request, _, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let candidate = BackendCandidate {
        sequence: 1,
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(2)?,
        objective: None,
        evidence_refs: Vec::new(),
    };
    let mut outcome = BackendSolveOutcome {
        backend_id: request.backend_id().clone(),
        model_hash: request.model_hash().to_owned(),
        solve_fingerprint: request.solve_fingerprint().to_owned(),
        termination: BackendTerminationReason::CandidateFound,
        evidence: BackendTerminationEvidence {
            remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
            backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
            elapsed_milliseconds: DurationMillis::new(3)?,
            first_incumbent_milliseconds: Some(DurationMillis::new(1)?),
            objective: None,
            evidence_refs: Vec::new(),
            execution: None,
        },
    };
    assert_eq!(
        validate_outcome(
            &request,
            &outcome,
            std::slice::from_ref(&candidate),
            SolverApiLimits::DEFAULT,
        ),
        Err(OutcomeError::FirstIncumbentMismatch)
    );
    outcome.evidence.first_incumbent_milliseconds = Some(DurationMillis::new(2)?);
    outcome.solve_fingerprint = "0".repeat(64);
    assert_eq!(
        validate_outcome(
            &request,
            &outcome,
            std::slice::from_ref(&candidate),
            SolverApiLimits::DEFAULT,
        ),
        Err(OutcomeError::SolveFingerprintMismatch)
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn execution_evidence_round_trips_and_is_bound_to_the_request() -> TestResult {
    let (request, _, _) = request(BackendId::new(FAKE_BACKEND_ID)?)?;
    let candidate = BackendCandidate {
        sequence: 1,
        values: valid_candidate()?,
        observed_after_milliseconds: DurationMillis::new(2)?,
        objective: None,
        evidence_refs: Vec::new(),
    };
    let mut outcome = BackendSolveOutcome {
        backend_id: request.backend_id().clone(),
        model_hash: request.model_hash().to_owned(),
        solve_fingerprint: request.solve_fingerprint().to_owned(),
        termination: BackendTerminationReason::CandidateFound,
        evidence: BackendTerminationEvidence {
            remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
            backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
            elapsed_milliseconds: DurationMillis::new(10)?,
            first_incumbent_milliseconds: Some(DurationMillis::new(2)?),
            objective: None,
            evidence_refs: Vec::new(),
            execution: Some(BackendExecutionEvidence {
                timings: BackendTimingEvidence {
                    translation_serialization_milliseconds: DurationMillis::new(1)?,
                    worker_startup_milliseconds: Some(DurationMillis::new(2)?),
                    handshake_milliseconds: Some(DurationMillis::new(1)?),
                    solver_milliseconds: Some(DurationMillis::new(5)?),
                    protocol_decode_milliseconds: Some(DurationMillis::new(1)?),
                },
                model_counts: BackendModelCountEvidence {
                    planning_variable_count: request.summary().variable_count,
                    planning_constraint_count: request.summary().constraint_count,
                    translated_variable_count: 2,
                    translated_constraint_count: 1,
                },
                worker_statistics: Some(BackendWorkerStatistics {
                    wall_time_milliseconds: Some(DurationMillis::new(4)?),
                    user_time_milliseconds: Some(DurationMillis::new(3)?),
                    deterministic_time_milliseconds: Some(DurationMillis::new(2)?),
                    conflicts: Some(3),
                    branches: Some(4),
                    binary_propagations: Some(5),
                    integer_propagations: Some(6),
                }),
                reproducibility: BackendReproducibilityEvidence {
                    backend_version: request.backend_version().to_owned(),
                    adapter_version: request.adapter_version().to_owned(),
                    worker_version: "1.0.0".to_owned(),
                    engine_version: "1.0.0".to_owned(),
                    protocol_major: 1,
                    protocol_minor: 0,
                    applied_options: request.options().clone(),
                    applied_parameters: BackendAppliedParameterEvidence {
                        wall_time_milliseconds: Some(DurationMillis::new(700)?),
                        memory_limit_bytes: None,
                        worker_threads: 1,
                        random_seed: 7,
                        stop_after_first_feasible: false,
                        emit_intermediate_solutions: false,
                        log_search_progress: false,
                        deterministic_test_profile: true,
                    },
                    model_fingerprint_sha256: "a".repeat(64),
                    applied_parameters_sha256: Some("b".repeat(64)),
                },
            }),
        },
    };
    assert_eq!(
        validate_outcome(
            &request,
            &outcome,
            std::slice::from_ref(&candidate),
            SolverApiLimits::DEFAULT,
        ),
        Ok(())
    );
    outcome
        .evidence
        .execution
        .as_mut()
        .ok_or("fixture missing execution evidence")?
        .timings
        .solver_milliseconds = Some(DurationMillis::new(11)?);
    assert_eq!(
        validate_outcome(
            &request,
            &outcome,
            std::slice::from_ref(&candidate),
            SolverApiLimits::DEFAULT,
        ),
        Err(OutcomeError::InvalidExecutionEvidence)
    );
    outcome
        .evidence
        .execution
        .as_mut()
        .ok_or("fixture missing execution evidence")?
        .timings
        .solver_milliseconds = Some(DurationMillis::new(5)?);
    let encoded = serde_json::to_vec(&outcome)?;
    assert_eq!(
        serde_json::from_slice::<BackendSolveOutcome>(&encoded)?,
        outcome
    );

    outcome
        .evidence
        .execution
        .as_mut()
        .ok_or("fixture missing execution evidence")?
        .reproducibility
        .applied_options
        .random_seed = 8;
    assert_eq!(
        validate_outcome(
            &request,
            &outcome,
            std::slice::from_ref(&candidate),
            SolverApiLimits::DEFAULT,
        ),
        Err(OutcomeError::ExecutionOptionsMismatch)
    );
    let reproducibility = &mut outcome
        .evidence
        .execution
        .as_mut()
        .ok_or("fixture missing execution evidence")?
        .reproducibility;
    reproducibility.applied_options.random_seed = 7;
    reproducibility.applied_parameters.worker_threads = 2;
    assert_eq!(
        validate_outcome(
            &request,
            &outcome,
            std::slice::from_ref(&candidate),
            SolverApiLimits::DEFAULT,
        ),
        Err(OutcomeError::InvalidExecutionEvidence)
    );
    Ok(())
}

#[test]
fn final_backend_evidence_matches_objective_dimension() -> TestResult {
    let backend = BackendId::new(FAKE_BACKEND_ID)?;
    let problem = Arc::new(two_level_problem()?);
    let summary = summarize(&problem, PlanningIrLimitsV1::DEFAULT)?;
    let options = solve_options(backend.clone())?;
    let (parent, _, _) = parent_budget(1_000)?;
    let request = SolveRequest::new(
        backend.clone(),
        "1.0.0",
        "1.0.0",
        problem,
        summary,
        options,
        &parent,
        None,
        DurationMillis::ZERO,
    )?;
    let outcome = BackendSolveOutcome {
        backend_id: backend,
        model_hash: request.model_hash().to_owned(),
        solve_fingerprint: request.solve_fingerprint().to_owned(),
        termination: BackendTerminationReason::InfeasibilityClaimed,
        evidence: BackendTerminationEvidence {
            remaining_at_dispatch_milliseconds: request.dispatch_budget().remaining_at_dispatch(),
            backend_limit_milliseconds: request.dispatch_budget().backend_limit(),
            elapsed_milliseconds: DurationMillis::new(1)?,
            first_incumbent_milliseconds: None,
            objective: Some(BackendObjectiveEvidence {
                objective_values: vec![0],
                best_bound_values: Some(vec![0, 0]),
            }),
            evidence_refs: Vec::new(),
            execution: None,
        },
    };
    assert_eq!(
        validate_outcome(&request, &outcome, &[], SolverApiLimits::DEFAULT),
        Err(OutcomeError::InvalidObjectiveDimension)
    );
    Ok(())
}
#[test]
fn production_registry_contains_no_fake_or_test_identifier() -> TestResult {
    let registry = SolverRegistry::production()?;
    assert!(registry.is_empty());
    assert!(PRODUCTION_BACKENDS.iter().all(|(id, _, _)| {
        !id.contains("fake") && !id.starts_with("test.") && !id.starts_with("tests.")
    }));
    Ok(())
}
